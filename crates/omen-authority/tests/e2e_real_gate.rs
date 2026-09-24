//! Real Omen + real Tethers Gate end-to-end proof.
//!
//! Requires a canonical Gate workspace (`OMEN_H2_GATE_EXE`,
//! `OMEN_H2_ENGINE_EXE`, `OMEN_H2_TETHERS_SRC`). Without all three the
//! suite skips loudly (matrix covers the logic; this file proves the
//! live integration).
//!
//! Fixture capability `fixture.ping` (standing-allow manifest): action
//! args are `{message, path}` projected from Omen's event data, so Omen
//! recomputes the exact `argument_digest`. The physical command is a
//! marker fixture Omen owns: marker presence == physical execution.

use omen_authority::{
    AdmitExecute, AskPolicy, AuthorityIntent, CommitOutcome, ExpectedGate, GateProcess,
    GateSpawnConfig, GateTransport, HumanOutcome, OutcomeJournal, PrepareOutcome,
    SupervisorExecutor, run::run_once,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

const CORE_TETHER: &str = r#"tether "J14 complete local scenario"

anchor
    coding.task_completed

when
    project.type is "software"
    and task.changed_files greater_than 0

do
    fixture.ping
        message: anchor.task
        path: anchor.path
"#;

const MARKER_RS: &str = r#"
fn main() {
    let marker = std::env::args().nth(1).expect("marker path");
    std::fs::write(&marker, std::process::id().to_string()).expect("marker");
    println!("marker-ok");
}
"#;

struct GateEnv {
    gate_exe: PathBuf,
    engine_exe: PathBuf,
    manifest_text: String,
}

fn gate_env() -> Option<GateEnv> {
    let gate_exe = std::env::var_os("OMEN_H2_GATE_EXE").map(PathBuf::from)?;
    let engine_exe = std::env::var_os("OMEN_H2_ENGINE_EXE").map(PathBuf::from)?;
    let src = std::env::var_os("OMEN_H2_TETHERS_SRC").map(PathBuf::from)?;
    if !gate_exe.is_file() || !engine_exe.is_file() {
        return None;
    }
    // Pin proof: the checkout must be exactly the canonical SHA.
    let sha = std::env::var("OMEN_H2_TETHERS_SHA").unwrap_or_default();
    assert_eq!(
        sha.to_lowercase(),
        omen_authority::TETHERS_SOURCE_SHA,
        "tethers checkout SHA must equal the pinned canonical SHA"
    );
    let manifest_text = std::fs::read_to_string(
        src.join("tethers-0.1/protocol/capability-manifests/fixture-ping-standing-allow.json"),
    )
    .ok()?;
    Some(GateEnv {
        gate_exe,
        engine_exe,
        manifest_text,
    })
}

struct Workspace {
    root: PathBuf,
    config: PathBuf,
    trail: PathBuf,
    host_data: PathBuf,
}

impl Workspace {
    fn create(decision: &str, manifest_text: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "omen-h2-e2e-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(root.join("tethers")).unwrap();
        std::fs::create_dir_all(root.join("manifests")).unwrap();
        std::fs::create_dir_all(root.join("host-data")).unwrap();
        std::fs::write(root.join("tethers/complete.tether"), CORE_TETHER).unwrap();
        std::fs::write(
            root.join("manifests/fixture-ping-standing-allow.json"),
            manifest_text,
        )
        .unwrap();
        let ws = Self {
            config: root.join("runtime.json"),
            trail: root.join("trail.jsonl"),
            host_data: root.join("host-data"),
            root,
        };
        ws.write_config(decision);
        // Replay hardening mirrors the Gate's own requirement (Windows ACL).
        #[cfg(windows)]
        {
            let script = format!(
                "$p='{}'; $i=[System.Security.Principal.WindowsIdentity]::GetCurrent().Name; $a=[System.Security.AccessControl.DirectorySecurity]::new(); $a.SetAccessRuleProtection($true,$false); $h=[System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [System.Security.AccessControl.InheritanceFlags]::ObjectInherit; foreach($t in @($i,'NT AUTHORITY\\SYSTEM','BUILTIN\\Administrators')) {{ $a.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new($t,'FullControl',$h,'None','Allow')) }}; Set-Acl -LiteralPath $p -AclObject $a",
                ws.host_data.display()
            );
            let st = std::process::Command::new("pwsh.exe")
                .args(["-NoProfile", "-Command", &script])
                .status()
                .expect("acl harden");
            assert!(st.success());
        }
        ws
    }

    fn write_config(&self, decision: &str) {
        let manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(self.root.join("manifests/fixture-ping-standing-allow.json"))
                .unwrap(),
        )
        .unwrap();
        let digest = manifest["digest"].as_str().unwrap();
        let config = json!({
            "format_version": "0.1",
            "tether_set": {
                "id": "r2.authority-gate", "version": "1",
                "tethers": [{
                    "id": "r2-complete", "version": "1",
                    "source_path": "tethers/complete.tether",
                    "core_environment": {
                        "program_id": "program.j14.complete", "core_version": "1",
                        "capabilities": [{
                            "source_name": "fixture.ping",
                            "capability_id": "cap.semantic.fixture-ping",
                            "contract_digest": "CORE-CONTRACT-J14",
                            "runtime_name": "fixture.ping"
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
                    {"name": "fixture.ping", "version": 1, "reason": "H2 e2e fixture"}
                ]
            },
            "providers": [{
                "id": "tethers-stdio-fixture", "display_name": "Tethers Stdio Fixture",
                "transport": {"kind": "stdio", "command": "pwsh.exe",
                              "args": ["-NoProfile", "-File", "providers/fixture.ps1"],
                              "protocol_version": "2025-11-25"},
                "capabilities": [{
                    "name": "fixture.ping", "version": 1,
                    "manifest_path": "manifests/fixture-ping-standing-allow.json",
                    "pinned_digest": digest,
                    "scope_binding": {"kind": "path_prefix", "argument_json_pointer": "/path"}
                }]
            }],
            "policy": {"default": "deny", "rules": [{"name": "fixture.ping", "version": 1, "decision": decision}]}
        });
        std::fs::write(&self.config, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    }

    /// H2 forensics: the canonical Gate redacts every Windows
    /// replay-provision failure to `PersistenceUnavailable` by design, so a
    /// hello refusal carries no failing-condition detail. This harness-only
    /// probe dumps the Gate-side validation inputs (path spelling, volume,
    /// owner, DACL, emptiness) on failure. It changes no production
    /// semantics: the caller still fails closed after printing.
    fn diagnose(&self) -> String {
        const CAP: usize = 2000;
        fn cap(s: &str) -> String {
            let out: String = s.chars().take(CAP).collect();
            if out.len() < s.len() {
                format!("{out}…[truncated]")
            } else {
                out
            }
        }
        #[cfg(windows)]
        fn run(prog: &str, args: &[&str]) -> String {
            match std::process::Command::new(prog).args(args).output() {
                Ok(o) => {
                    let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
                    let e = String::from_utf8_lossy(&o.stderr);
                    if !e.trim().is_empty() {
                        s.push_str("\n[stderr] ");
                        s.push_str(&e);
                    }
                    s
                }
                Err(e) => format!("<spawn failed: {e}>"),
            }
        }
        let mut d = String::new();
        d.push_str(&format!("host_data={}\n", self.host_data.display()));
        d.push_str(&format!("config={}\n", self.config.display()));
        d.push_str(&format!("trail={}\n", self.trail.display()));
        d.push_str(&format!(
            "temp_dir={} TEMP={:?} TMP={:?}\n",
            std::env::temp_dir().display(),
            std::env::var("TEMP"),
            std::env::var("TMP"),
        ));
        let spelling = self.host_data.to_string_lossy().into_owned();
        d.push_str(&format!(
            "absolute={} contains_slash={} contains_ext_prefix={}\n",
            self.host_data.is_absolute(),
            spelling.contains('/'),
            spelling.contains("\\\\."),
        ));
        match std::fs::read_dir(&self.host_data) {
            Ok(rd) => {
                let mut names: Vec<String> = rd
                    .map(|e| {
                        e.map(|x| x.file_name().to_string_lossy().into_owned())
                            .unwrap_or_else(|e| format!("<entry-err: {e}>"))
                    })
                    .collect();
                names.sort();
                d.push_str(&format!("host_data_entries={names:?}\n"));
            }
            Err(e) => d.push_str(&format!("host_data_readdir_err={e}\n")),
        }
        #[cfg(windows)]
        {
            let hd = self.host_data.to_string_lossy().into_owned();
            let drive = hd.chars().next().unwrap_or('?').to_string();
            d.push_str(&format!("whoami={}\n", cap(&run("whoami", &[]))));
            d.push_str(&format!(
                "whoami_groups={}\n",
                cap(&run("whoami", &["/groups"]))
            ));
            d.push_str(&format!("icacls={}\n", cap(&run("icacls", &[hd.as_str()]))));
            // Owner + inheritance detail: icacls shows only the DACL, while
            // the Gate requires the directory owner to equal the process
            // token user. Capture both explicitly (pwsh is already required
            // by the Workspace ACL hardening above).
            let acl_probe = format!(
                "$a=Get-Acl -LiteralPath '{}'; 'OWNER='+$a.Owner; $a.Access | Format-Table IdentityReference,FileSystemRights,AccessControlType,IsInherited -AutoSize | Out-String -Width 200",
                hd
            );
            d.push_str(&format!(
                "get_acl={}\n",
                cap(&run(
                    "pwsh.exe",
                    &["-NoProfile", "-Command", acl_probe.as_str()]
                ))
            ));
            d.push_str(&format!(
                "fsinfo_volume={}\n",
                cap(&run("fsutil", &["fsinfo", "volumeinfo", drive.as_str()]))
            ));
            d.push_str(&format!(
                "fsinfo_drivetype={}\n",
                cap(&run("fsutil", &["fsinfo", "drivetype", drive.as_str()]))
            ));
        }
        cap(&d)
    }

    fn spawn_gate(&self, env: &GateEnv) -> GateProcess {
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
            request_timeout: Duration::from_secs(90),
            stderr_cap: 32768,
        };
        // Fixture paths resolve relative to the config dir (Gate-side).
        let mut gate = GateProcess::spawn(&cfg).expect("gate spawns");
        let hello = gate.hello(Duration::from_secs(30)).unwrap_or_else(|e| {
            panic!(
                "hello works: {e:?} :: gate stderr: {} :: h2_forensics:\n{}",
                String::from_utf8_lossy(&gate.stderr_tail()),
                self.diagnose()
            )
        });
        ExpectedGate::pinned("canonical".to_string(), None)
            .verify_hello(&hello)
            .expect("canonical gate identity verifies");
        gate
    }
}

fn marker_fixture(dir: &Path) -> PathBuf {
    // Compiled ONCE per test binary (leaked tempdir): six parallel
    // rustc invocations would otherwise spike CI runner load and
    // starve timing-sensitive suites running alongside. The exe is
    // shared; only marker PATHS differ per test.
    let _ = dir;
    static COMPILED: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    COMPILED
        .get_or_init(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let src = dir.path().join("marker_fixture.rs");
            std::fs::write(&src, MARKER_RS).unwrap();
            let exe = dir.path().join("marker-fixture.exe");
            let out = std::process::Command::new("rustc")
                .arg("--edition=2021")
                .arg(&src)
                .arg("-o")
                .arg(&exe)
                .output()
                .expect("rustc builds marker fixture");
            assert!(out.status.success(), "marker build failed");
            let kept = dir.keep();
            kept.join("marker-fixture.exe")
        })
        .clone()
}

fn intent_for(
    fixture_exe: &Path,
    marker: &Path,
    eval: &str,
    manifest_digest: &str,
) -> AuthorityIntent {
    AuthorityIntent {
        tether_id: "r2-complete".to_string(),
        tether_version: "1".to_string(),
        evaluation_id: eval.to_string(),
        action_id: "action_1".to_string(),
        event_id: "evt_h2_001".to_string(),
        event_name: "coding.task_completed".to_string(),
        event_data: json!({"project": "lantern-keeper", "task": "LK-39", "path": "projects/r2-stdio"}),
        facts: json!({"project.type": "software", "task.changed_files": 3}),
        expected_arguments: json!({"message": "LK-39", "path": "projects/r2-stdio"}),
        expected_capability: "fixture.ping".to_string(),
        expected_capability_version: 1,
        expected_manifest_digest: manifest_digest.to_string(),
        expected_provider: "tethers-stdio-fixture".to_string(),
        argv: vec![
            fixture_exe.to_string_lossy().into_owned(),
            marker.to_string_lossy().into_owned(),
        ],
        cwd: fixture_exe.parent().unwrap().to_path_buf(),
        timeout_ms: 30_000,
        success_result: json!({"echo": "marker-ok"}),
    }
}

fn manifest_digest(manifest_text: &str) -> String {
    let v: Value = serde_json::from_str(manifest_text).unwrap();
    v["digest"].as_str().unwrap().to_string()
}

fn skip(name: &str) {
    eprintln!("SKIP-E2E ({name}): set OMEN_H2_GATE_EXE + OMEN_H2_ENGINE_EXE + OMEN_H2_TETHERS_SRC");
}

// ALLOW: gate admits, Omen executes once, marker once, outcome reaches Tethers.
#[tokio::test]
async fn e2e_allow_executes_once_and_reports() {
    let Some(env) = gate_env() else {
        skip("allow");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create("allow", &env.manifest_text);
    let fixture = marker_fixture(tmp.path());
    let marker = tmp.path().join("allow.marker");
    let digest = manifest_digest(&env.manifest_text);
    let intent = intent_for(&fixture, &marker, "eval_e2e_allow", &digest);
    let journal = OutcomeJournal::open(tmp.path().join("j.jsonl"));

    let gate = ws.spawn_gate(&env);
    let mut driver = AdmitExecute::new(gate);
    let mut exec = SupervisorExecutor::new();
    let report = run_once(
        &mut driver,
        Some("gate_e2e".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("allow e2e drives cleanly");
    drop(driver);

    assert!(
        matches!(report.human, HumanOutcome::Admitted),
        "got {:?}",
        report.human
    );
    assert_eq!(report.spawn_count, 1);
    assert!(
        marker.is_file(),
        "ALLOW marker missing: Omen did not execute"
    );
    assert_eq!(
        report.projection.outcome_state.as_deref(),
        Some("succeeded")
    );

    // Outcome reached Tethers: fresh session status shows terminal truth.
    let gate2 = ws.spawn_gate(&env);
    let mut driver2 = AdmitExecute::new(gate2);
    let view = driver2.status().expect("status works");
    let exec_id = report.projection.execution_id.clone().unwrap();
    assert!(
        view.terminal_known(&exec_id),
        "gate does not show terminal outcome for {exec_id}"
    );
    driver2.transport_mut().shutdown();
}

// DENY: gate denies, marker absent.
#[tokio::test]
async fn e2e_deny_zero_spawn() {
    let Some(env) = gate_env() else {
        skip("deny");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create("deny", &env.manifest_text);
    let fixture = marker_fixture(tmp.path());
    let marker = tmp.path().join("deny.marker");
    let digest = manifest_digest(&env.manifest_text);
    let intent = intent_for(&fixture, &marker, "eval_e2e_deny", &digest);
    let journal = OutcomeJournal::open(tmp.path().join("j.jsonl"));

    let gate = ws.spawn_gate(&env);
    let mut driver = AdmitExecute::new(gate);
    let mut exec = SupervisorExecutor::new();
    let report = run_once(
        &mut driver,
        Some("gate_e2e".to_string()),
        &intent,
        AskPolicy::Approve,
        &mut exec,
        &journal,
    )
    .await
    .expect("deny surfaces");
    assert!(matches!(report.human, HumanOutcome::Denied(_)));
    assert!(!marker.exists(), "DENY executed: marker exists");
    driver.transport_mut().shutdown();
}

// ASK: asks with marker absent; approval + fresh COMMIT; marker exactly once.
#[tokio::test]
async fn e2e_ask_approve_executes_once() {
    let Some(env) = gate_env() else {
        skip("ask");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create("ask", &env.manifest_text);
    let fixture = marker_fixture(tmp.path());
    let marker = tmp.path().join("ask.marker");
    let digest = manifest_digest(&env.manifest_text);
    let journal = OutcomeJournal::open(tmp.path().join("j.jsonl"));

    let gate = ws.spawn_gate(&env);
    let mut driver = AdmitExecute::new(gate);
    let mut exec = SupervisorExecutor::new();
    let intent = intent_for(&fixture, &marker, "eval_e2e_ask", &digest);
    let ask = run_once(
        &mut driver,
        Some("gate_e2e".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("ask surfaces");
    assert!(matches!(ask.human, HumanOutcome::ApprovalRequired(_)));
    assert!(!marker.exists(), "ASK executed before approval");

    let intent2 = intent_for(&fixture, &marker, "eval_e2e_ask2", &digest);
    let done = run_once(
        &mut driver,
        Some("gate_e2e".to_string()),
        &intent2,
        AskPolicy::Approve,
        &mut exec,
        &journal,
    )
    .await
    .expect("approved ask executes");
    assert!(matches!(done.human, HumanOutcome::Admitted));
    assert!(marker.is_file(), "approved ASK never executed");
    assert!(done.projection.approval_consumed);
    driver.transport_mut().shutdown();
}

// REVOKE: prepare succeeds, config flips to deny, commit refuses, marker absent.
#[tokio::test]
async fn e2e_revoke_before_commit_zero_spawn() {
    let Some(env) = gate_env() else {
        skip("revoke");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create("allow", &env.manifest_text);
    let fixture = marker_fixture(tmp.path());
    let marker = tmp.path().join("revoke.marker");
    let digest = manifest_digest(&env.manifest_text);
    let intent = intent_for(&fixture, &marker, "eval_e2e_revoke", &digest);

    let gate = ws.spawn_gate(&env);
    let mut driver = AdmitExecute::new(gate);
    let prepared_id = match driver.prepare(&intent).expect("prepare works") {
        PrepareOutcome::AllowPrepared { prepared_id, .. } => prepared_id,
        other => panic!("want allow, got {other:?}"),
    };
    // Authority revoked between PREPARE and COMMIT (config file flip —
    // the Gate re-reads fresh truth at COMMIT).
    ws.write_config("deny");
    let commit = driver
        .commit(&prepared_id, None)
        .expect("commit roundtrips");
    assert!(
        matches!(commit, CommitOutcome::Refused { .. }),
        "revoked commit must refuse, got {commit:?}"
    );
    assert!(!marker.exists(), "REVOKED executed: marker exists");

    // Re-admission on fresh truth executes exactly once.
    ws.write_config("allow");
    let journal = OutcomeJournal::open(tmp.path().join("j.jsonl"));
    let intent2 = intent_for(&fixture, &marker, "eval_e2e_revoke2", &digest);
    let mut exec = SupervisorExecutor::new();
    let done = run_once(
        &mut driver,
        Some("gate_e2e".to_string()),
        &intent2,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("re-admission executes");
    assert!(matches!(done.human, HumanOutcome::Admitted));
    assert!(marker.is_file());
    driver.transport_mut().shutdown();
}

// GATE DOWN: dead session fails closed, marker absent.
#[tokio::test]
async fn e2e_gate_down_zero_spawn() {
    let Some(env) = gate_env() else {
        skip("down");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create("allow", &env.manifest_text);
    let fixture = marker_fixture(tmp.path());
    let marker = tmp.path().join("down.marker");
    let digest = manifest_digest(&env.manifest_text);
    let intent = intent_for(&fixture, &marker, "eval_e2e_down", &digest);
    let journal = OutcomeJournal::open(tmp.path().join("j.jsonl"));

    let gate = ws.spawn_gate(&env);
    let mut driver = AdmitExecute::new(gate);
    // Kill the Gate mid-session (drop = kill): COMMIT can never complete.
    driver.transport_mut().shutdown();
    let mut exec = SupervisorExecutor::new();
    let err = run_once(
        &mut driver,
        Some("gate_e2e".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect_err("dead gate must fail closed");
    assert!(!marker.exists(), "dead gate executed");
    let _ = err;
}

// MULTI-STEP: step 1 executes; revoke; step 2 denied with zero new spawn.
#[tokio::test]
async fn e2e_multistep_readmission() {
    let Some(env) = gate_env() else {
        skip("multistep");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create("allow", &env.manifest_text);
    let fixture = marker_fixture(tmp.path());
    let digest = manifest_digest(&env.manifest_text);
    let journal = OutcomeJournal::open(tmp.path().join("j.jsonl"));

    let gate = ws.spawn_gate(&env);
    let mut driver = AdmitExecute::new(gate);
    let mut exec = SupervisorExecutor::new();
    let marker1 = tmp.path().join("step1.marker");
    let intent1 = intent_for(&fixture, &marker1, "eval_e2e_s1", &digest);
    // Step 1 marker path must be inside argv: rebuild with its own marker.
    let mut intent1 = intent1;
    intent1.argv[1] = marker1.to_string_lossy().into_owned();
    let s1 = run_once(
        &mut driver,
        Some("gate_e2e".to_string()),
        &intent1,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("step 1 executes");
    assert!(matches!(s1.human, HumanOutcome::Admitted));
    assert!(marker1.is_file());

    ws.write_config("deny");
    let marker2 = tmp.path().join("step2.marker");
    let mut intent2 = intent_for(&fixture, &marker2, "eval_e2e_s2", &digest);
    intent2.argv[1] = marker2.to_string_lossy().into_owned();
    let s2 = run_once(
        &mut driver,
        Some("gate_e2e".to_string()),
        &intent2,
        AskPolicy::Approve,
        &mut exec,
        &journal,
    )
    .await
    .expect("step 2 refusal surfaces");
    assert!(matches!(s2.human, HumanOutcome::Denied(_)));
    assert!(!marker2.exists(), "step 2 inherited step 1 permission");
    driver.transport_mut().shutdown();
}
