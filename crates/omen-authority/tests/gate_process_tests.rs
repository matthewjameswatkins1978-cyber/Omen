//! Real-process Gate supervision proofs: missing binary, startup
//! failure, crash before COMMIT, wrong protocol, malformed frames,
//! response timeout, and secret isolation.
//!
//! A minimal scripted stub (compiled with rustc, no dependencies,
//! hand-rolled frame scanning — a fixture, not a protocol
//! implementation) stands in for the Gate child. Every case drives the
//! full `run_once` with a counting executor where a session exists and
//! asserts marker absence + zero calls.

use omen_authority::{
    AdmitExecute, AskPolicy, CountingExecutor, ExpectedGate, GateProcess, GateSpawnConfig,
    GateTransport, OutcomeJournal, protocol::HelloResult, run::run_once,
};
use std::sync::OnceLock;
use std::time::Duration;

const STUB_RS: &str = r#"
use std::io::{BufRead, Write};
fn extract<'a>(line: &'a str, key: &str) -> String {
    let pat = format!("\"{key}\"");
    let mut search: &str = line;
    while let Some(i) = search.find(&pat) {
        let mut rest = &search[i + pat.len()..];
        rest = rest.trim_start_matches([' ', ':']);
        if rest.starts_with('"') {
            let mut out = String::new();
            let mut esc = false;
            for c in rest[1..].chars() {
                if esc { out.push(c); esc = false; }
                else if c == '\\' { esc = true; }
                else if c == '"' { break; }
                else { out.push(c); }
            }
            return out;
        }
        search = &search[i + 1..];
    }
    String::new()
}
fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "exit-now" { std::process::exit(1); }
    if mode == "env-dump" {
        let out = std::env::args().nth(2).expect("env-dump needs path");
        let mut text = String::new();
        for (k, v) in std::env::vars() {
            text.push_str(&format!("{k}={v}\n"));
        }
        std::fs::write(out, text).unwrap();
    }
    if mode == "wrong-protocol" {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let line = line.unwrap();
            let rid = extract(&line, "request_id");
            println!("{{\"schema\":\"evil/9\",\"request_id\":\"{rid}\",\"status\":\"ok\",\"result\":{{}}}}");
        }
        return;
    }
    let stdin = std::io::stdin();
    let mut seen = 0u64;
    for line in stdin.lock().lines() {
        let line = line.unwrap();
        seen += 1;
        if mode == "garbage-first" && seen == 1 {
            println!("THIS IS NOT NDJSON{{{{");
            continue;
        }
        let rid = extract(&line, "request_id");
        let op = extract(&line, "operation");
        if mode == "crash-after-prepare" && op == "prepare" {
            let eval = extract(&line, "evaluation_id");
            let act = extract(&line, "action_id");
            println!("{{\"schema\":\"tethers.authority/1\",\"request_id\":\"{rid}\",\"status\":\"ok\",\"result\":{{\"prepared_id\":\"prep_stub_1\",\"decision\":\"allow_prepared\",\"reason\":\"current_policy_allow\",\"authorizes_dispatch\":false,\"evaluation_id\":\"{eval}\",\"action_id\":\"{act}\",\"tether_id\":\"t\",\"tether_version\":\"1\",\"core_planned\":true,\"provider_invocations\":0,\"execution\":{{\"performed\":false,\"provider_invocations\":0,\"replay_mutated\":false}}}}}}");
            let _ = std::io::stdout().flush();
            std::process::exit(1);
        }
        if mode == "hang" {
            std::thread::sleep(std::time::Duration::from_secs(300));
        }
        let result = match op.as_str() {
            "hello" => "{\"protocol\":\"tethers.authority/1\",\"protocol_versions\":[\"tethers.authority/1\"],\"product_version\":\"0.8.0\",\"git_sha\":null,\"features\":[\"prepare\",\"approval_decision\",\"commit\",\"outcome\",\"status\",\"shutdown\"],\"gate_instance_id\":\"gate_stub\",\"authority_granted\":false,\"provider_invocations\":0}".to_string(),
            "status" => "{\"protocol\":\"tethers.authority/1\",\"product_version\":\"0.8.0\",\"gate_instance_id\":\"gate_stub\",\"healthy\":true,\"durable_reconciliation\":{\"state\":\"healthy\"},\"shutdown_requested\":false,\"provider_invocations\":0,\"pending_approvals\":[],\"unresolved_commits\":[],\"prepared\":[],\"terminal_outcomes\":[],\"recovery_required\":[],\"truncated\":false,\"prepared_count\":0,\"committed_count\":0}".to_string(),
            _ => "{\"unexpected\":true}".to_string(),
        };
        println!("{{\"schema\":\"tethers.authority/1\",\"request_id\":\"{rid}\",\"status\":\"ok\",\"result\":{result}}}");
    }
}
"#;

fn stub_exe() -> std::path::PathBuf {
    static COMPILED: OnceLock<std::path::PathBuf> = OnceLock::new();
    COMPILED
        .get_or_init(|| {
            let dir = tempfile::tempdir().expect("tempdir");
            let src = dir.path().join("stub.rs");
            std::fs::write(&src, STUB_RS).unwrap();
            let exe = dir.path().join("gate-stub.exe");
            let out = std::process::Command::new("rustc")
                .arg("--edition=2021")
                .arg(&src)
                .arg("-o")
                .arg(&exe)
                .output()
                .expect("rustc builds the stub");
            assert!(
                out.status.success(),
                "stub build failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            // Leak the tempdir: the stub must outlive the test session.
            let leaked = dir.keep();
            leaked.join("gate-stub.exe")
        })
        .clone()
}

fn spawn_config(mode: &str, extra: Vec<String>) -> GateSpawnConfig {
    let mut args = vec![mode.to_string()];
    args.extend(extra);
    GateSpawnConfig {
        exe: stub_exe(),
        args,
        expected_exe_sha256: None,
        env_extra: Vec::new(),
        startup_timeout: Duration::from_secs(5),
        request_timeout: Duration::from_secs(5),
        stderr_cap: 8192,
    }
}

fn harness_bits() -> (tempfile::TempDir, std::path::PathBuf, OutcomeJournal) {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("spawn.marker");
    let journal = OutcomeJournal::open(dir.path().join("j.jsonl"));
    (dir, marker, journal)
}

fn test_intent(dir: &std::path::Path) -> omen_authority::AuthorityIntent {
    omen_authority::AuthorityIntent {
        tether_id: "t".to_string(),
        tether_version: "1".to_string(),
        evaluation_id: "e".to_string(),
        action_id: "a".to_string(),
        event_id: "ev".to_string(),
        event_name: "n".to_string(),
        event_data: serde_json::json!({}),
        facts: serde_json::json!({}),
        expected_arguments: serde_json::json!({}),
        expected_capability: "c".to_string(),
        expected_capability_version: 1,
        expected_manifest_digest: "sha256:0".to_string(),
        expected_provider: "p".to_string(),
        argv: vec!["x".to_string()],
        cwd: dir.to_path_buf(),
        timeout_ms: 1000,
        success_result: serde_json::json!({"echo": "x"}),
    }
}

// 8. Missing Gate: zero spawn (no session can exist).
#[test]
fn g08_missing_gate_zero_spawn() {
    let (_dir, marker, _journal) = harness_bits();
    let mut cfg = spawn_config("ok", vec![]);
    cfg.exe = std::path::PathBuf::from("C:\\nonexistent\\omen-h2-no-gate.exe");
    let err = GateProcess::spawn(&cfg).expect_err("missing binary must fail");
    assert!(
        format!("{err:?}").contains("gate.spawn.failed"),
        "got: {err:?}"
    );
    assert!(!marker.exists());
}

// 9. Gate startup failure: child exits at once; hello fails; zero spawn.
#[tokio::test]
async fn g09_startup_failure_zero_spawn() {
    let (dir, marker, journal) = harness_bits();
    let cfg = spawn_config("exit-now", vec![]);
    let mut gate = GateProcess::spawn(&cfg).expect("spawn of exit-now works");
    // The child exits at once: hello fails either on write (broken pipe
    // won the race) or on read (EOF). Both are handshake failure —
    // the phase is timing, the invariant (zero spawn) is not.
    let err = gate
        .hello(Duration::from_secs(3))
        .expect_err("hello must fail on instant exit");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("eof")
            || msg.contains("closed")
            || msg.contains("write.failed")
            || msg.contains("Broken pipe"),
        "got: {msg}"
    );
    let intent = test_intent(dir.path());
    let mut driver = AdmitExecute::new(gate);
    let mut exec = CountingExecutor::succeeding(Some(marker.clone()));
    let err = run_once(
        &mut driver,
        None,
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect_err("dead session must fail closed");
    assert_eq!(exec.calls, 0);
    assert!(!marker.exists(), "startup failure executed");
    let _ = err;
}

// 10. Gate crashes before COMMIT: zero spawn.
#[tokio::test]
async fn g10_crash_before_commit_zero_spawn() {
    let (dir, marker, journal) = harness_bits();
    let cfg = spawn_config("crash-after-prepare", vec![]);
    let mut gate = GateProcess::spawn(&cfg).expect("spawn works");
    gate.hello(Duration::from_secs(5)).expect("hello works");
    let intent = test_intent(dir.path());
    let mut driver = AdmitExecute::new(gate);
    let mut exec = CountingExecutor::succeeding(Some(marker.clone()));
    let err = run_once(
        &mut driver,
        Some("gate_stub".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect_err("crashed gate must fail closed at commit");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("closed")
            || msg.contains("broken")
            || msg.contains("failed")
            || msg.contains("eof"),
        "got: {msg}"
    );
    assert_eq!(exec.calls, 0);
    assert!(!marker.exists(), "crashed gate executed");
}

// 11. Wrong protocol: handshake refuses; zero spawn.
#[tokio::test]
async fn g11_wrong_protocol_zero_spawn() {
    let (_dir, marker, _journal) = harness_bits();
    let cfg = spawn_config("wrong-protocol", vec![]);
    let mut gate = GateProcess::spawn(&cfg).expect("spawn works");
    let err = gate
        .hello(Duration::from_secs(5))
        .expect_err("wrong schema must refuse");
    assert!(
        format!("{err:?}").contains("unsupported_schema"),
        "got: {err:?}"
    );
    assert!(!marker.exists());
}

// 11b. Handshake identity pins (unit): product, features, premature grant.
#[test]
fn g11b_hello_identity_pins() {
    let pin = ExpectedGate::pinned("abc123".to_string(), None);
    let base = || HelloResult {
        protocol: "tethers.authority/1".to_string(),
        protocol_versions: vec!["tethers.authority/1".to_string()],
        product_version: "0.8.0".to_string(),
        git_sha: None,
        features: vec![
            "prepare".to_string(),
            "approval_decision".to_string(),
            "commit".to_string(),
            "outcome".to_string(),
            "status".to_string(),
            "shutdown".to_string(),
        ],
        gate_instance_id: "gate_x".to_string(),
        authority_granted: false,
        provider_invocations: 0,
    };
    assert!(pin.verify_hello(&base()).is_ok());
    let mut bad = base();
    bad.protocol = "tethers.authority/2".to_string();
    assert!(pin.verify_hello(&bad).is_err());
    let mut bad = base();
    bad.product_version = "0.7.1".to_string();
    assert!(pin.verify_hello(&bad).is_err());
    let mut bad = base();
    bad.features.retain(|f| f != "commit");
    assert!(pin.verify_hello(&bad).is_err());
    let mut bad = base();
    bad.authority_granted = true;
    assert!(pin.verify_hello(&bad).is_err());
}

// 12. Malformed NDJSON: handshake refuses; zero spawn.
#[tokio::test]
async fn g12_malformed_frame_zero_spawn() {
    let (_dir, marker, _journal) = harness_bits();
    let cfg = spawn_config("garbage-first", vec![]);
    let mut gate = GateProcess::spawn(&cfg).expect("spawn works");
    let err = gate
        .hello(Duration::from_secs(5))
        .expect_err("garbage must refuse");
    assert!(format!("{err:?}").contains("invalid_json"), "got: {err:?}");
    assert!(!marker.exists());
}

// 16b. Response timeout against a real hung child.
#[tokio::test]
async fn g16b_hung_gate_timeout_zero_spawn() {
    let (dir, marker, journal) = harness_bits();
    let mut cfg = spawn_config("hang", vec![]);
    cfg.startup_timeout = Duration::from_secs(1);
    cfg.request_timeout = Duration::from_secs(1);
    let mut gate = GateProcess::spawn(&cfg).expect("spawn works");
    let err = gate
        .hello(Duration::from_secs(1))
        .expect_err("hung gate must time out");
    assert!(format!("{err:?}").contains("timeout"), "got: {err:?}");
    let intent = test_intent(dir.path());
    let mut driver = AdmitExecute::new(gate);
    let mut exec = CountingExecutor::succeeding(Some(marker.clone()));
    let _ = run_once(
        &mut driver,
        None,
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await;
    assert_eq!(exec.calls, 0);
    assert!(!marker.exists());
}

// 22. Unrelated parent secret is absent from the Gate environment.
#[test]
fn g22_gate_child_secret_isolation() {
    // Single-var process-env mutation: unique sentinel name, restored
    // before return; no other test reads this variable.
    unsafe {
        std::env::set_var("OMEN_H2_SECRET_SENTINEL", "top-secret-h2-probe-value");
    }
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("env.txt");
    let cfg = spawn_config("env-dump", vec![dump.to_string_lossy().into_owned()]);
    let mut gate = GateProcess::spawn(&cfg).expect("spawn works");
    gate.hello(Duration::from_secs(5)).expect("hello works");
    gate.shutdown();
    let text = std::fs::read_to_string(&dump).expect("env dump written");
    assert!(
        !text.contains("OMEN_H2_SECRET_SENTINEL") && !text.contains("top-secret-h2-probe-value"),
        "parent secret leaked into gate environment"
    );
    assert!(text.contains("PATH="), "minimal env must carry PATH");
    unsafe {
        std::env::remove_var("OMEN_H2_SECRET_SENTINEL");
    }
}
