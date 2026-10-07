//! Omen pipe proofs over `tethers.authority/2` host-execution bundles.
//!
//! Requires a canonical Gate workspace (`OMEN_H2_GATE_EXE`,
//! `OMEN_H2_ENGINE_EXE`, `OMEN_H2_TETHERS_SRC`, `OMEN_H2_TETHERS_SHA` —
//! the same wiring as the H2 live suite); skips loudly otherwise.
//! The checkout SHA must equal [`omen_authority::TETHERS_SOURCE_SHA`].
//! Pin provenance: merged Phase A main 4dd85883 plus the stdio authority/2
//! wiring branch (pending Tethers-main merge; re-pin to the new main then).
//!
//! Architecture under proof (canonical):
//! ordinary Tethers Actions → sequential PREPARE → atomic `commit_bundle`
//! → Omen spawns `producer | consumer` over direct OS pipes (and
//! `producer | consumer > file` with the redirect inside the authorised
//! composition). Omen owns composition + raw bytes; Tethers owns
//! authority/replay/outcome truth. `together` is never used for pipes.
//!
//! Laws (each a test below):
//! - every required Action is PREPAREd before physical execution;
//! - bundle COMMIT succeeds before the first spawn;
//! - PREPARE/approval/scope/replay/composition failure → ZERO spawns;
//! - the actual composition is bound to the authorised identity (drift
//!   fails closed);
//! - raw bytes are byte-preserving through the pipe (and redirect);
//! - redirect authority is included before execution;
//! - partial start reports started members truthfully and unstarted
//!   members via the bounded `not_attempted` outcome;
//! - no authority/1 regression (the /1 suites keep passing unmodified).

use omen_authority::{
    AdmitExecute, AuthorisedPipeReport, AuthorityIntent, BundleCommitOutcome, ExpectedGate,
    GateProcess, GateSpawnConfig, GateTransport, OutcomeDelivered, PipeRedirect, PipeStage,
    StageAttempt, TETHERS_SOURCE_SHA, authorise_and_spawn_pipe, authorise_pipe, deliver_outcome_v2,
    empty_approvals, member_outcome_payload, spawn_authorised,
};
use omen_engine::ProcessSupervisor;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

const BUNDLE_TETHER: &str = r#"tether "Omen pipe bundle scenario"

anchor
    coding.task_completed

when
    project.type is "software"
    and task.changed_files greater_than 0

do
    fixture.bundle
        message: anchor.task
        path: anchor.path
        composition_digest: anchor.composition
"#;

const PRODUCER_RS: &str = r#"
fn main() {
    let marker = std::env::args().nth(1).expect("marker path");
    std::fs::write(&marker, std::process::id().to_string()).expect("marker");
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    out.write_all(b"omen-pipe-proof-v1\n").unwrap();
    let row: Vec<u8> = (0u8..=255u8).collect();
    out.write_all(&row).unwrap();
    out.write_all(b"END-NO-NEWLINE").unwrap();
    out.flush().unwrap();
}
"#;

const CONSUMER_RS: &str = r#"
fn main() {
    let marker = std::env::args().nth(1).expect("marker path");
    std::fs::write(&marker, std::process::id().to_string()).expect("marker");
    use std::io::{Read, Write};
    let mut input = Vec::new();
    std::io::stdin().lock().read_to_end(&mut input).unwrap();
    std::io::stdout().lock().write_all(&input).unwrap();
}
"#;

fn fixed_bytes() -> Vec<u8> {
    let mut v = b"omen-pipe-proof-v1\n".to_vec();
    v.extend(0u8..=255u8);
    v.extend_from_slice(b"END-NO-NEWLINE");
    v
}

struct BundleEnv {
    gate_exe: PathBuf,
    engine_exe: PathBuf,
    manifest_text: String,
}

fn bundle_env() -> Option<BundleEnv> {
    let gate_exe = std::env::var_os("OMEN_H2_GATE_EXE").map(PathBuf::from)?;
    let engine_exe = std::env::var_os("OMEN_H2_ENGINE_EXE").map(PathBuf::from)?;
    let src = std::env::var_os("OMEN_H2_TETHERS_SRC").map(PathBuf::from)?;
    if !gate_exe.is_file() || !engine_exe.is_file() {
        return None;
    }
    let sha = std::env::var("OMEN_H2_TETHERS_SHA").unwrap_or_default();
    assert_eq!(
        sha.to_lowercase(),
        TETHERS_SOURCE_SHA,
        "tethers checkout SHA must equal the pinned canonical SHA"
    );
    let manifest_text = std::fs::read_to_string(src.join(
        "tethers-0.1/host-rust/fixtures/capability-manifests/fixture-bundle-standing-allow.json",
    ))
    .ok()?;
    Some(BundleEnv {
        gate_exe,
        engine_exe,
        manifest_text,
    })
}

fn skip(name: &str) {
    eprintln!(
        "SKIP-PIPE ({name}): set OMEN_H2_GATE_EXE + OMEN_H2_ENGINE_EXE + OMEN_H2_TETHERS_SRC"
    );
}

struct Workspace {
    config: PathBuf,
    trail: PathBuf,
    host_data: PathBuf,
}

impl Workspace {
    fn create(policy_decision: &str, manifest_text: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "omen-pipe-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(root.join("tethers")).unwrap();
        std::fs::create_dir_all(root.join("manifests")).unwrap();
        std::fs::create_dir_all(root.join("host-data")).unwrap();
        std::fs::write(root.join("tethers/bundle.tether"), BUNDLE_TETHER).unwrap();
        std::fs::write(
            root.join("manifests/fixture-bundle-standing-allow.json"),
            manifest_text,
        )
        .unwrap();
        let ws = Self {
            config: root.join("runtime.json"),
            trail: root.join("trail.jsonl"),
            host_data: root.join("host-data"),
        };
        ws.write_config(policy_decision, manifest_text);
        #[cfg(windows)]
        harden_acl(&ws.host_data);
        ws
    }

    fn write_config(&self, decision: &str, manifest_text: &str) {
        let manifest: Value = serde_json::from_str(manifest_text).unwrap();
        let digest = manifest["digest"].as_str().unwrap();
        let config = json!({
            "format_version": "0.1",
            "tether_set": {
                "id": "bundle.authority-gate",
                "version": "1",
                "tethers": [{
                    "id": "bundle-complete", "version": "1",
                    "source_path": "tethers/bundle.tether",
                    "core_environment": {
                        "program_id": "program.bundle.complete", "core_version": "1",
                        "capabilities": [{
                            "source_name": "fixture.bundle",
                            "capability_id": "cap.semantic.fixture-bundle",
                            "contract_digest": "CORE-CONTRACT-BUNDLE",
                            "runtime_name": "fixture.bundle"
                        }],
                        "input_facts": [
                            {"source_name": "project.type", "fact_id": "fact.project_type",
                             "host_snapshot_key": "project.type", "scalar_type": "string",
                             "schema_description": "project type"},
                            {"source_name": "task.changed_files", "fact_id": "fact.task_changed_files",
                             "host_snapshot_key": "task.changed_files", "scalar_type": "integer",
                             "schema_description": "number of changed files"}
                        ]
                    }
                }],
                "capability_requirements": [
                    {"name": "fixture.bundle", "version": 1, "reason": "Omen pipe proofs"}
                ]
            },
            "providers": [{
                "id": "tethers-host-fixture", "display_name": "Tethers Host Fixture",
                "transport": {"kind": "stdio", "command": "pwsh.exe",
                              "args": ["-NoProfile", "-File", "providers/fixture.ps1"],
                              "protocol_version": "2025-11-25"},
                "capabilities": [{
                    "name": "fixture.bundle", "version": 1,
                    "manifest_path": "manifests/fixture-bundle-standing-allow.json",
                    "pinned_digest": digest,
                    "scope_binding": {"kind": "path_prefix", "argument_json_pointer": "/path"}
                }]
            }],
            "policy": {"default": "deny", "rules": [
                {"name": "fixture.bundle", "version": 1, "decision": decision}
            ]}
        });
        std::fs::write(&self.config, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    }

    fn spawn_gate(&self, env: &BundleEnv) -> GateProcess {
        let cfg = GateSpawnConfig {
            exe: env.gate_exe.clone(),
            args: vec![
                "gate".to_string(),
                "--stdio".to_string(),
                "--config".to_string(),
                self.config.to_string_lossy().into_owned(),
                "--engine".to_string(),
                env.engine_exe.to_string_lossy().into_owned(),
                "--trail".to_string(),
                self.trail.to_string_lossy().into_owned(),
                "--host-data-root".to_string(),
                self.host_data.to_string_lossy().into_owned(),
            ],
            expected_exe_sha256: None,
            env_extra: Vec::new(),
            startup_timeout: Duration::from_secs(30),
            request_timeout: Duration::from_secs(120),
            stderr_cap: 32768,
        };
        let mut gate = GateProcess::spawn(&cfg).expect("gate spawns");
        let hello = gate.hello_v2(Duration::from_secs(30)).unwrap_or_else(|e| {
            panic!(
                "hello_v2 works: {e:?} :: gate stderr: {}",
                String::from_utf8_lossy(&gate.stderr_tail())
            )
        });
        ExpectedGate::pinned_v2("canonical".to_string(), None)
            .verify_hello(&hello)
            .expect("canonical /2 gate identity verifies");
        gate
    }

    /// TEMPORARY exact-seam probe (Windows only): true when the replay
    /// root owner differs from the invoking user, i.e. the canonical
    /// Gate will deterministically refuse hello with exactly
    /// GATE_REPLAY_UNAVAILABLE. Harness-only; changes no semantics.
    #[cfg(windows)]
    fn windows_owner_seam_active(&self) -> bool {
        let hd = self.host_data.to_string_lossy().into_owned();
        let owner = std::process::Command::new("pwsh.exe")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Acl -LiteralPath '{hd}').Owner"),
            ])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let user = std::process::Command::new("whoami")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        !owner.is_empty() && !user.is_empty() && !owner.eq_ignore_ascii_case(&user)
    }

    fn committed_count(&self) -> usize {
        let dir = self.host_data.join("bundles");
        match std::fs::read_dir(&dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().ends_with(".committed.json"))
                .count(),
            Err(_) => 0,
        }
    }
}

#[cfg(windows)]
fn harden_acl(dir: &Path) {
    let script = format!(
        "$p='{}'; $i=[System.Security.Principal.WindowsIdentity]::GetCurrent().Name; $a=[System.Security.AccessControl.DirectorySecurity]::new(); $a.SetAccessRuleProtection($true,$false); $h=[System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [System.Security.AccessControl.InheritanceFlags]::ObjectInherit; foreach($t in @($i,'NT AUTHORITY\\SYSTEM','BUILTIN\\Administrators')) {{ $a.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new($t,'FullControl',$h,'None','Allow')) }}; Set-Acl -LiteralPath $p -AclObject $a",
        dir.display()
    );
    let st = std::process::Command::new("pwsh.exe")
        .args(["-NoProfile", "-Command", &script])
        .status()
        .expect("acl harden");
    assert!(st.success());
}

fn compile_fixture(dir: &Path, name: &str, source: &str) -> PathBuf {
    // Build-once cache plus a PRIVATE per-test copy: sabotage tests
    // (drift/unstarted) rewrite or delete their own executables, so no
    // test may observe another test's physical material.
    static COMPILED: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    static BUILD_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _build_guard = BUILD_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    let root = COMPILED
        .get_or_init(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            dir.keep()
        })
        .clone();
    let src = root.join(format!("{name}.rs"));
    let exe = root.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    if !exe.is_file() {
        std::fs::write(&src, source).unwrap();
        let out = std::process::Command::new("rustc")
            .arg("--edition=2021")
            .arg(&src)
            .arg("-o")
            .arg(&exe)
            .output()
            .expect("rustc builds pipe fixture");
        assert!(out.status.success(), "fixture build failed: {name}");
    }
    let private = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(&exe, &private).expect("private fixture copy");
    private
}

fn stage_intent(
    evaluation_id: &str,
    action_id: &str,
    event_id: &str,
    task: &str,
    path: &str,
    composition: &str,
    manifest_digest: &str,
) -> AuthorityIntent {
    AuthorityIntent {
        tether_id: "bundle-complete".to_string(),
        tether_version: "1".to_string(),
        evaluation_id: evaluation_id.to_string(),
        action_id: action_id.to_string(),
        event_id: event_id.to_string(),
        event_name: "coding.task_completed".to_string(),
        event_data: json!({
            "project": "omen-shell", "task": task, "path": path,
            "composition": composition
        }),
        facts: json!({"project.type": "software", "task.changed_files": 3}),
        expected_arguments: json!({
            "message": task, "path": path, "composition_digest": composition
        }),
        expected_capability: "fixture.bundle".to_string(),
        expected_capability_version: 1,
        expected_manifest_digest: manifest_digest.to_string(),
        expected_provider: "tethers-host-fixture".to_string(),
        timeout_ms: 30_000,
        success_result: json!({"echo": "pipe-ok"}),
    }
}

fn manifest_digest(manifest_text: &str) -> String {
    let v: Value = serde_json::from_str(manifest_text).unwrap();
    v["digest"].as_str().unwrap().to_string()
}

/// Build both stages, compute the live composition digest over physical
/// material, then bind the intents to it. The digest covers exe
/// bytes/argv/cwd/redirect only — rebinding intents afterwards cannot
/// change it.
fn build_stages(
    dir: &Path,
    tag: &str,
    path: &str,
    manifest_digest: &str,
    redirect: Option<&Path>,
) -> (Vec<PipeStage>, PathBuf, PathBuf) {
    let producer = compile_fixture(dir, "omen-pipe-producer", PRODUCER_RS);
    let consumer = compile_fixture(dir, "omen-pipe-consumer", CONSUMER_RS);
    let producer_marker = dir.join(format!("producer-{tag}.marker"));
    let consumer_marker = dir.join(format!("consumer-{tag}.marker"));
    let _ = std::fs::remove_file(&producer_marker);
    let _ = std::fs::remove_file(&consumer_marker);
    let redir = redirect.map(|p| PipeRedirect {
        path: p.to_path_buf(),
    });
    let mut stages = vec![
        PipeStage {
            exe: producer.clone(),
            argv: vec![
                producer.to_string_lossy().into_owned(),
                producer_marker.to_string_lossy().into_owned(),
            ],
            cwd: dir.to_path_buf(),
            intent: stage_intent(
                "eval-x",
                "action_x",
                "evt-x",
                "pipe.x",
                path,
                "",
                manifest_digest,
            ),
        },
        PipeStage {
            exe: consumer.clone(),
            argv: vec![
                consumer.to_string_lossy().into_owned(),
                consumer_marker.to_string_lossy().into_owned(),
            ],
            cwd: dir.to_path_buf(),
            intent: stage_intent(
                "eval-y",
                "action_y",
                "evt-y",
                "pipe.y",
                path,
                "",
                manifest_digest,
            ),
        },
    ];
    let (_, composition) =
        omen_authority::compose_identity(&stages, redir.as_ref()).expect("compose works");
    for (stage, task) in stages.iter_mut().zip(["pipe.producer", "pipe.consumer"]) {
        let (evaluation_id, action_id, event_id) = if task == "pipe.producer" {
            (
                format!("eval_pipe_producer_{tag}"),
                "action_1".to_string(),
                format!("evt_pipe_producer_{tag}"),
            )
        } else {
            (
                format!("eval_pipe_consumer_{tag}"),
                "action_1".to_string(),
                format!("evt_pipe_consumer_{tag}"),
            )
        };
        stage.intent = stage_intent(
            &evaluation_id,
            &action_id,
            &event_id,
            task,
            path,
            &composition,
            manifest_digest,
        );
    }
    (stages, producer_marker, consumer_marker)
}

fn unattempted(member_execution_id: &str) -> StageAttempt {
    StageAttempt {
        stage_index: 0,
        execution_id: member_execution_id.to_string(),
        attempted: false,
        classification: "not_attempted",
        error: None,
    }
}

fn report_outcomes(driver: &mut AdmitExecute<GateProcess>, report: &AuthorisedPipeReport) {
    for attempt in &report.attempts {
        let payload = member_outcome_payload(
            &attempt.execution_id,
            attempt,
            Some(json!({"echo": "pipe-ok"})),
        );
        match deliver_outcome_v2(driver.transport_mut(), &payload).expect("outcome delivers") {
            OutcomeDelivered::Recorded(_) => {}
            OutcomeDelivered::Refused { code, message } => {
                panic!("outcome refused: {code}: {message}")
            }
        }
    }
}

fn report_not_attempted(driver: &mut AdmitExecute<GateProcess>, execution_ids: &[String]) {
    for execution_id in execution_ids {
        let payload = member_outcome_payload(execution_id, &unattempted(execution_id), None);
        match deliver_outcome_v2(driver.transport_mut(), &payload).expect("outcome delivers") {
            OutcomeDelivered::Recorded(r) => assert_eq!(r.status, "not_attempted"),
            OutcomeDelivered::Refused { code, message } => {
                panic!("not_attempted refused: {code}: {message}")
            }
        }
    }
}

fn assert_terminals(driver: &mut AdmitExecute<GateProcess>, report: &AuthorisedPipeReport) {
    let view = driver.status().expect("status works");
    for attempt in &report.attempts {
        assert!(
            view.terminal_known(&attempt.execution_id),
            "gate lacks terminal outcome for {}",
            attempt.execution_id
        );
    }
}

// producer | consumer: byte-preserving pipe over direct OS pipes.
#[tokio::test]
async fn pipe_two_stage_bytes_preserved() {
    let Some(env) = bundle_env() else {
        skip("bytes");
        return;
    };
    let digest = manifest_digest(&env.manifest_text);
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create("allow", &env.manifest_text);
    #[cfg(windows)]
    if ws.windows_owner_seam_active() {
        characterise_seam(&env, &ws, &digest, "bytes");
        return;
    }
    let (stages, producer_marker, consumer_marker) =
        build_stages(tmp.path(), "bytes", "projects/pipe", &digest, None);
    let supervisor = ProcessSupervisor::new();
    let gate = ws.spawn_gate(&env);
    let mut driver = AdmitExecute::new(gate);
    let report = authorise_and_spawn_pipe(&mut driver, &supervisor, stages, None)
        .await
        .expect("pipe authorises and spawns");
    assert_eq!(report.spawn_count, 2, "both stages spawned");
    assert_eq!(report.bundle.members.len(), 2);
    assert!(!report.bundle.bundle_id.is_empty());
    assert_ne!(
        report.bundle.members[0].execution_id, report.bundle.members[1].execution_id,
        "distinct member execution identities"
    );
    let output = report.pipeline.as_ref().expect("pipeline output");
    assert_eq!(
        output.execution.stdout_all,
        fixed_bytes(),
        "consumer stdout is byte-identical to producer bytes"
    );
    assert!(producer_marker.is_file(), "producer spawned");
    assert!(consumer_marker.is_file(), "consumer spawned");
    assert_eq!(ws.committed_count(), 1, "exactly one committed marker");
    report_outcomes(&mut driver, &report);
    assert_terminals(&mut driver, &report);
    driver.transport_mut().shutdown();
}

// producer | consumer > file: redirect target inside the authorised composition.
#[tokio::test]
async fn pipe_redirect_to_file() {
    let Some(env) = bundle_env() else {
        skip("redirect");
        return;
    };
    let digest = manifest_digest(&env.manifest_text);
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create("allow", &env.manifest_text);
    #[cfg(windows)]
    if ws.windows_owner_seam_active() {
        characterise_seam(&env, &ws, &digest, "redirect");
        return;
    }
    let out_file = tmp.path().join("pipe-out.bin");
    let (stages, _, _) = build_stages(
        tmp.path(),
        "redirect",
        "projects/pipe",
        &digest,
        Some(&out_file),
    );
    let redirect = PipeRedirect {
        path: out_file.clone(),
    };
    let supervisor = ProcessSupervisor::new();
    let gate = ws.spawn_gate(&env);
    let mut driver = AdmitExecute::new(gate);
    let report = authorise_and_spawn_pipe(&mut driver, &supervisor, stages, Some(redirect))
        .await
        .expect("redirect pipe authorises and spawns");
    assert_eq!(report.spawn_count, 2);
    let file_bytes = std::fs::read(&out_file).expect("redirect file exists");
    assert_eq!(
        file_bytes,
        fixed_bytes(),
        "redirect file is byte-identical to producer bytes"
    );
    // The redirect target was inside the authorised composition: the
    // committed identity still matches live material including the file.
    let (rebuilt, _, _) = build_stages(
        tmp.path(),
        "redirect",
        "projects/pipe",
        &digest,
        Some(&out_file),
    );
    let redir = PipeRedirect {
        path: out_file.clone(),
    };
    let recomputed = omen_authority::compose_identity(&rebuilt, Some(&redir))
        .expect("recompose works")
        .1;
    assert_eq!(
        recomputed, report.bundle.composition_digest,
        "actual composition is bound to the authorised identity"
    );
    report_outcomes(&mut driver, &report);
    assert_terminals(&mut driver, &report);
    driver.transport_mut().shutdown();
}

// Deny at PREPARE: zero spawns, zero committed markers.
#[tokio::test]
async fn pipe_deny_zero_spawn() {
    let Some(env) = bundle_env() else {
        skip("deny");
        return;
    };
    let digest = manifest_digest(&env.manifest_text);
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create("deny", &env.manifest_text);
    #[cfg(windows)]
    if ws.windows_owner_seam_active() {
        characterise_seam(&env, &ws, &digest, "deny");
        return;
    }
    let (stages, producer_marker, consumer_marker) =
        build_stages(tmp.path(), "deny", "projects/pipe", &digest, None);
    let supervisor = ProcessSupervisor::new();
    let gate = ws.spawn_gate(&env);
    let mut driver = AdmitExecute::new(gate);
    let err = authorise_and_spawn_pipe(&mut driver, &supervisor, stages, None)
        .await
        .expect_err("deny must fail closed");
    let _ = err;
    assert!(!producer_marker.exists(), "DENY spawned producer");
    assert!(!consumer_marker.exists(), "DENY spawned consumer");
    assert_eq!(ws.committed_count(), 0, "DENY committed a bundle");
    driver.transport_mut().shutdown();
}

// Composition mismatch across members: commit refuses, zero spawns.
#[tokio::test]
async fn pipe_composition_mismatch_zero_spawn() {
    let Some(env) = bundle_env() else {
        skip("mismatch");
        return;
    };
    let digest = manifest_digest(&env.manifest_text);
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create("allow", &env.manifest_text);
    #[cfg(windows)]
    if ws.windows_owner_seam_active() {
        characterise_seam(&env, &ws, &digest, "mismatch");
        return;
    }
    let (mut stages, producer_marker, consumer_marker) =
        build_stages(tmp.path(), "mismatch", "projects/pipe", &digest, None);
    // Force member divergence: the consumer's authorised composition no
    // longer matches the producer's (simulates two unrelated plans).
    stages[1].intent = stage_intent(
        "eval_pipe_consumer_mismatch",
        "action_1",
        "evt_pipe_consumer_mismatch",
        "pipe.consumer",
        "projects/pipe",
        "forged-composition-identity",
        &digest,
    );
    let supervisor = ProcessSupervisor::new();
    let gate = ws.spawn_gate(&env);
    let mut driver = AdmitExecute::new(gate);
    let err = authorise_and_spawn_pipe(&mut driver, &supervisor, stages, None)
        .await
        .expect_err("composition mismatch must refuse");
    let s = format!("{err:?}");
    assert!(
        s.contains("composition"),
        "refusal names composition, got {s}"
    );
    assert!(!producer_marker.exists(), "MISMATCH spawned producer");
    assert!(!consumer_marker.exists(), "MISMATCH spawned consumer");
    assert_eq!(ws.committed_count(), 0, "MISMATCH committed a bundle");
    driver.transport_mut().shutdown();
}

// Scope violation (path outside the manifest prefix): zero spawns.
#[tokio::test]
async fn pipe_scope_violation_zero_spawn() {
    let Some(env) = bundle_env() else {
        skip("scope");
        return;
    };
    let digest = manifest_digest(&env.manifest_text);
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create("allow", &env.manifest_text);
    #[cfg(windows)]
    if ws.windows_owner_seam_active() {
        characterise_seam(&env, &ws, &digest, "scope");
        return;
    }
    let (stages, producer_marker, consumer_marker) =
        build_stages(tmp.path(), "scope", "elsewhere/pipe", &digest, None);
    let supervisor = ProcessSupervisor::new();
    let gate = ws.spawn_gate(&env);
    let mut driver = AdmitExecute::new(gate);
    let err = authorise_and_spawn_pipe(&mut driver, &supervisor, stages, None)
        .await
        .expect_err("scope violation must fail closed");
    let _ = err;
    assert!(!producer_marker.exists(), "SCOPE spawned producer");
    assert!(!consumer_marker.exists(), "SCOPE spawned consumer");
    assert_eq!(ws.committed_count(), 0, "SCOPE committed a bundle");
    driver.transport_mut().shutdown();
}

// Executable drift after COMMIT: spawn refuses, members report
// `not_attempted` (accepted on /2), zero spawns.
#[tokio::test]
async fn pipe_drift_zero_spawn_not_attempted_accepted() {
    let Some(env) = bundle_env() else {
        skip("drift");
        return;
    };
    let digest = manifest_digest(&env.manifest_text);
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create("allow", &env.manifest_text);
    #[cfg(windows)]
    if ws.windows_owner_seam_active() {
        characterise_seam(&env, &ws, &digest, "drift");
        return;
    }
    let (stages, producer_marker, consumer_marker) =
        build_stages(tmp.path(), "drift", "projects/pipe", &digest, None);
    let supervisor = ProcessSupervisor::new();
    let gate = ws.spawn_gate(&env);
    let mut driver = AdmitExecute::new(gate);
    let authorised = authorise_pipe(&mut driver, stages.clone(), None).expect("authorise works");
    assert_eq!(ws.committed_count(), 1, "bundle committed before drift");
    // Drift: rewrite the consumer executable after authority.
    std::fs::write(&stages[1].exe, b"tampered-bytes").unwrap();
    let drifted = omen_authority::AuthorisedBundle {
        bundle: authorised.bundle.clone(),
        composition_digest: authorised.composition_digest.clone(),
        stages,
        redirect: None,
    };
    let err = spawn_authorised(&supervisor, drifted)
        .await
        .expect_err("drift must fail closed");
    let s = format!("{err:?}");
    assert!(s.contains("drift"), "refusal names drift, got {s}");
    assert!(!producer_marker.exists(), "DRIFT spawned producer");
    assert!(!consumer_marker.exists(), "DRIFT spawned consumer");
    // Truthful outcomes: nothing started → both `not_attempted`, and the
    // /2 Gate accepts them for the committed bundle.
    report_not_attempted(
        &mut driver,
        &authorised
            .bundle
            .members
            .iter()
            .map(|m| m.execution_id.clone())
            .collect::<Vec<_>>(),
    );
    driver.transport_mut().shutdown();
}

// Unstarted members after local spawn failure: `not_attempted` accepted.
#[tokio::test]
async fn pipe_unstarted_members_not_attempted_accepted() {
    let Some(env) = bundle_env() else {
        skip("unstarted");
        return;
    };
    let digest = manifest_digest(&env.manifest_text);
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create("allow", &env.manifest_text);
    #[cfg(windows)]
    if ws.windows_owner_seam_active() {
        characterise_seam(&env, &ws, &digest, "unstarted");
        return;
    }
    let (stages, producer_marker, consumer_marker) =
        build_stages(tmp.path(), "unstarted", "projects/pipe", &digest, None);
    let supervisor = ProcessSupervisor::new();
    let gate = ws.spawn_gate(&env);
    let mut driver = AdmitExecute::new(gate);
    let authorised = authorise_pipe(&mut driver, stages.clone(), None).expect("authorise works");
    // Local spawn failure: the consumer executable vanishes after COMMIT.
    let stages = stages;
    std::fs::remove_file(&stages[1].exe).unwrap();
    let missing = omen_authority::AuthorisedBundle {
        bundle: authorised.bundle.clone(),
        composition_digest: authorised.composition_digest.clone(),
        stages,
        redirect: None,
    };
    let err = spawn_authorised(&supervisor, missing)
        .await
        .expect_err("missing executable must fail closed");
    let _ = err;
    assert!(!producer_marker.exists(), "UNSTARTED spawned producer");
    assert!(!consumer_marker.exists(), "UNSTARTED spawned consumer");
    report_not_attempted(
        &mut driver,
        &authorised
            .bundle
            .members
            .iter()
            .map(|m| m.execution_id.clone())
            .collect::<Vec<_>>(),
    );
    driver.transport_mut().shutdown();
}

// Second commit of the same bundle: replay refuses, zero new dispatch.
#[tokio::test]
async fn pipe_replay_second_commit_refused() {
    let Some(env) = bundle_env() else {
        skip("replay");
        return;
    };
    let digest = manifest_digest(&env.manifest_text);
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create("allow", &env.manifest_text);
    #[cfg(windows)]
    if ws.windows_owner_seam_active() {
        characterise_seam(&env, &ws, &digest, "replay");
        return;
    }
    let (stages, producer_marker, consumer_marker) =
        build_stages(tmp.path(), "replay", "projects/pipe", &digest, None);
    let gate = ws.spawn_gate(&env);
    let mut driver = AdmitExecute::new(gate);
    let authorised = authorise_pipe(&mut driver, stages, None).expect("first commit works");
    assert_eq!(ws.committed_count(), 1);
    let ids: Vec<String> = authorised
        .bundle
        .members
        .iter()
        .map(|m| m.prepared_id.clone())
        .collect();
    match driver
        .commit_bundle(&ids, &empty_approvals())
        .expect("second commit roundtrips")
    {
        BundleCommitOutcome::Refused { code, .. } => {
            assert!(
                code.contains("replay_blocked") || code.contains("already_committed"),
                "replay refusal names replay, got {code}"
            );
        }
        BundleCommitOutcome::Admitted { .. } => {
            panic!("second commit of the same bundle admitted a second dispatch")
        }
    }
    assert!(!producer_marker.exists(), "REPLAY spawned producer");
    assert!(!consumer_marker.exists(), "REPLAY spawned consumer");
    assert_eq!(ws.committed_count(), 1, "replay forked a second bundle");
    driver.transport_mut().shutdown();
}

#[cfg(windows)]
fn characterise_seam(env: &BundleEnv, ws: &Workspace, digest: &str, name: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let (_stages, producer_marker, consumer_marker) =
        build_stages(tmp.path(), name, "projects/pipe", digest, None);
    let cfg = GateSpawnConfig {
        exe: env.gate_exe.clone(),
        args: vec![
            "gate".to_string(),
            "--stdio".to_string(),
            "--config".to_string(),
            ws.config.to_string_lossy().into_owned(),
            "--engine".to_string(),
            env.engine_exe.to_string_lossy().into_owned(),
            "--trail".to_string(),
            ws.trail.to_string_lossy().into_owned(),
            "--host-data-root".to_string(),
            ws.host_data.to_string_lossy().into_owned(),
        ],
        expected_exe_sha256: None,
        env_extra: Vec::new(),
        startup_timeout: Duration::from_secs(30),
        request_timeout: Duration::from_secs(120),
        stderr_cap: 32768,
    };
    let mut gate = GateProcess::spawn(&cfg).expect("gate spawns");
    match gate.hello_v2(Duration::from_secs(30)) {
        Ok(_) => panic!("UPSTREAM-SEAM-REVIEW-REQUIRED ({name}): /2 hello unexpectedly succeeded"),
        Err(e) => {
            let s = format!("{e:?}");
            assert!(
                s.contains("GATE_REPLAY_UNAVAILABLE"),
                "seam drift ({name}): expected exactly GATE_REPLAY_UNAVAILABLE, got {s}"
            );
        }
    }
    assert!(!producer_marker.exists(), "seam executed ({name})");
    assert!(!consumer_marker.exists(), "seam executed ({name})");
    assert_eq!(ws.committed_count(), 0, "seam committed ({name})");
    eprintln!(
        "PIPE-SEAM-CHARACTERISED ({name}): exact GATE_REPLAY_UNAVAILABLE, zero spawn — fail-closed, NOT live proof"
    );
}
