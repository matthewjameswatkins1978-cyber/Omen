//! `omen authority` — human CLI over the Tethers admission seam.
//!
//! Thin projection only: all authority truth lives in `omen-authority`.
//! Machine output is the [`AuthorityProjection`] JSON (agents consume
//! that, never prose); humans get one closed-vocabulary line.

use clap::{Args, Subcommand};
use omen_authority::{
    AdmitExecute, AuthorityIntent, ExpectedGate, FixtureProvision, GateProcess, GateSpawnConfig,
    GateTransport, OutcomeJournal, SupervisorExecutor, parse_ask_policy, report_json,
    run::run_once,
};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Args, Debug)]
pub struct AuthorityArgs {
    #[command(subcommand)]
    pub subcommand: AuthoritySubcommands,
}

#[derive(Subcommand, Debug)]
pub enum AuthoritySubcommands {
    /// Admit one consequential step under current Tethers truth and
    /// execute it exactly once on success.
    Exec(ExecArgs),
    /// Show the Gate's bounded durable-reconciliation view.
    Status(StatusArgs),
    /// Fetch + verify + install the release-pinned Gate companion.
    FetchCompanion(FetchCompanionArgs),
}

#[derive(Args, Debug)]
pub struct ExecArgs {
    /// Intent spec JSON (see AuthorityIntent fields).
    #[arg(long)]
    pub spec: PathBuf,
    /// ASK policy: approve|deny|cancel|defer (default: defer).
    #[arg(long, default_value = "defer")]
    pub on_ask: String,
    /// Managed companion dir (default: install-root/gate-companion).
    #[arg(long)]
    pub companion_dir: Option<PathBuf>,
    /// Gate runtime dir holding runtime.json (or OMEN_H2_GATE_RUNTIME).
    #[arg(long)]
    pub gate_runtime: Option<PathBuf>,
    /// Outcome journal path (default: temp/omen-authority/journal.jsonl).
    #[arg(long)]
    pub journal: Option<PathBuf>,
    /// Trusted fixture-provider executable (installation truth for the
    /// Omen-owned resolver; the spec carries NO argv).
    #[arg(long)]
    pub fixture_exe: PathBuf,
    /// Trusted fixture sandbox dir: resolver-derived marker scope AND
    /// physical cwd.
    #[arg(long)]
    pub fixture_workdir: PathBuf,
}

#[derive(Args, Debug)]
pub struct StatusArgs {
    #[arg(long)]
    pub companion_dir: Option<PathBuf>,
    #[arg(long)]
    pub gate_runtime: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct FetchCompanionArgs {
    /// Exact release tag (e.g. v0.9.0-preview.22). Explicit, never latest.
    #[arg(long)]
    pub tag: String,
    /// Companion dir to install (default: install-root/gate-companion).
    #[arg(long)]
    pub companion_dir: Option<PathBuf>,
}

/// Intent spec: SEMANTIC ONLY. There is deliberately no `argv`/`cwd`
/// field: the exact physical command is derived after COMMIT by the
/// Omen-owned trusted resolver from these semantics plus the
/// `--fixture-exe`/`--fixture-workdir` installation truth. Unknown
/// fields (including stale `argv`/`cwd`) are rejected loudly so a
/// caller can never pair authorised semantics with arbitrary argv.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SpecFile {
    tether_id: String,
    tether_version: String,
    evaluation_id: String,
    action_id: String,
    event_id: String,
    event_name: String,
    event_data: serde_json::Value,
    facts: serde_json::Value,
    expected_arguments: serde_json::Value,
    expected_capability: String,
    expected_capability_version: u32,
    expected_manifest_digest: String,
    expected_provider: String,
    timeout_ms: Option<u64>,
    success_result: Option<serde_json::Value>,
}

fn default_companion_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OMEN_H2_COMPANION_DIR") {
        return PathBuf::from(dir);
    }
    // Install layout: <root>/bin/omen[.exe] -> <root>/gate-companion.
    if let Ok(exe) = std::env::current_exe()
        && let Some(bin) = exe.parent()
        && let Some(root) = bin.parent()
    {
        return root.join("gate-companion");
    }
    PathBuf::from("gate-companion")
}

fn default_journal() -> PathBuf {
    std::env::temp_dir()
        .join("omen-authority")
        .join("journal.jsonl")
}

fn gate_runtime_dir(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(d) = explicit {
        return Ok(d);
    }
    std::env::var("OMEN_H2_GATE_RUNTIME")
        .map(PathBuf::from)
        .map_err(|_| {
            "gate runtime required: --gate-runtime or OMEN_H2_GATE_RUNTIME (dir with runtime.json)"
                .to_string()
        })
}

fn spawn_session(
    companion_dir: Option<PathBuf>,
    runtime: &std::path::Path,
) -> Result<(GateProcess, String, omen_authority::GateCompanion), String> {
    let companion =
        omen_authority::resolve_companion(&companion_dir.unwrap_or_else(default_companion_dir))
            .map_err(|e| {
                format!(
                    "companion resolve failed: {e:?}; run `omen authority fetch-companion --tag <release>` first"
                )
            })?;
    let config = runtime.join("runtime.json");
    if !config.is_file() {
        return Err(format!(
            "gate runtime has no runtime.json: {}",
            runtime.display()
        ));
    }
    let trail = runtime.join("trail.jsonl");
    let host_data = runtime.join("host-data");
    let gate_cfg = GateSpawnConfig {
        exe: companion.paths.gate_exe.clone(),
        args: vec![
            "gate".to_string(),
            "--stdio".to_string(),
            "--config".to_string(),
            config.to_string_lossy().into_owned(),
            "--engine".to_string(),
            companion.paths.engine_exe.to_string_lossy().into_owned(),
            "--trail".to_string(),
            trail.to_string_lossy().into_owned(),
            "--host-data-root".to_string(),
            host_data.to_string_lossy().into_owned(),
        ],
        expected_exe_sha256: Some(companion.provenance.gate_exe_sha256.clone()),
        env_extra: Vec::new(),
        startup_timeout: Duration::from_secs(30),
        request_timeout: Duration::from_secs(90),
        stderr_cap: 32768,
    };
    let mut gate =
        GateProcess::spawn(&gate_cfg).map_err(|e| format!("gate start failed: {e:?}"))?;
    let hello = gate
        .hello(Duration::from_secs(30))
        .map_err(|e| format!("gate handshake failed: {e:?}"))?;
    ExpectedGate::pinned(
        companion.provenance.tethers_source_sha.clone(),
        Some(companion.provenance.gate_exe_sha256.clone()),
    )
    .verify_hello(&hello)
    .map_err(|e| format!("gate identity refused: {e:?}"))?;
    let instance = hello.gate_instance_id.clone();
    Ok((gate, instance, companion))
}

pub async fn run_exec(args: &ExecArgs, machine: bool) -> Result<(), Box<dyn std::error::Error>> {
    let ask = parse_ask_policy(&args.on_ask).ok_or_else(|| {
        format!(
            "--on-ask must be approve|deny|cancel|defer, got {}",
            args.on_ask
        )
    })?;
    let text = std::fs::read_to_string(&args.spec).map_err(|e| format!("spec unreadable: {e}"))?;
    let spec: SpecFile = serde_json::from_str(&text).map_err(|e| format!("spec malformed: {e}"))?;
    let intent = AuthorityIntent {
        tether_id: spec.tether_id,
        tether_version: spec.tether_version,
        evaluation_id: spec.evaluation_id,
        action_id: spec.action_id,
        event_id: spec.event_id,
        event_name: spec.event_name,
        event_data: spec.event_data,
        facts: spec.facts,
        expected_arguments: spec.expected_arguments,
        expected_capability: spec.expected_capability,
        expected_capability_version: spec.expected_capability_version,
        expected_manifest_digest: spec.expected_manifest_digest,
        expected_provider: spec.expected_provider,
        timeout_ms: spec.timeout_ms.unwrap_or(30_000),
        success_result: spec
            .success_result
            .unwrap_or(serde_json::json!({"echo": ""})),
    };
    let provision = FixtureProvision {
        exe: args.fixture_exe.clone(),
        workdir: args.fixture_workdir.clone(),
    };
    let runtime = gate_runtime_dir(args.gate_runtime.clone())?;
    let (gate, instance, _companion) = spawn_session(args.companion_dir.clone(), &runtime)?;
    let journal = OutcomeJournal::open(args.journal.clone().unwrap_or_else(default_journal));
    let mut driver = AdmitExecute::new(gate);
    let mut exec = SupervisorExecutor::new();
    let report = run_once(
        &mut driver,
        Some(instance),
        &intent,
        &provision,
        ask,
        &mut exec,
        &journal,
    )
    .await;
    driver.into_transport().shutdown();
    match report {
        Ok(rep) => {
            if machine {
                println!("{}", serde_json::to_string_pretty(&report_json(&rep))?);
            } else {
                println!("{}", rep.human.line());
            }
            if rep.spawn_count > 1 {
                return Err("invariant violated: spawn_count > 1".into());
            }
            Ok(())
        }
        Err(e) => {
            if machine {
                println!(
                    "{}",
                    serde_json::json!({"ok": false, "authority_state": "malformed_authority", "detail": format!("{e:?}")})
                );
            } else {
                eprintln!("malformed authority: {e:?} (zero spawn)");
            }
            Err(format!("{e:?}").into())
        }
    }
}

pub async fn run_status(args: &StatusArgs) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = gate_runtime_dir(args.gate_runtime.clone())?;
    let (gate, _instance, _companion) = spawn_session(args.companion_dir.clone(), &runtime)?;
    let mut driver = AdmitExecute::new(gate);
    let view = driver
        .status()
        .map_err(|e| format!("status failed: {e:?}"))?;
    driver.into_transport().shutdown();
    println!("{}", serde_json::to_string_pretty(&view.raw)?);
    Ok(())
}

pub async fn run_fetch_companion(
    args: &FetchCompanionArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    use omen_lifecycle::transport::GithubTransport;
    let dir = args
        .companion_dir
        .clone()
        .unwrap_or_else(default_companion_dir);
    let work = std::env::temp_dir().join("omen-authority-fetch");
    let fetched = omen_authority::fetch_companion(&GithubTransport, &args.tag, &dir, &work)
        .map_err(|e| format!("fetch failed: {e:?}"))?;
    println!(
        "{}",
        serde_json::json!({
            "ok": true,
            "dir": fetched.dir.to_string_lossy(),
            "tethers_source_sha": fetched.provenance.tethers_source_sha,
            "gate_exe_sha256": fetched.provenance.gate_exe_sha256,
            "engine_exe_sha256": fetched.provenance.engine_exe_sha256,
            "protocol": fetched.provenance.protocol,
        })
    );
    Ok(())
}
