//! Child-process hermetic proofs: the real `omen` executable, spawned
//! with synthetic configured credentials, stays denied for live AI
//! egress.
//!
//! The child `main` admits the process (unchanged production behavior),
//! but the inherited `OMEN_HERMETIC_TESTS=1` denial takes precedence at
//! the shared HTTPS choke point. `doctor --json` reports the structural
//! evidence: `ai_egress.admitted=true` (the child really admitted),
//! `test_isolation=true` (denial overrode), and `configured` naming the
//! providers the child resolved from its synthetic ambient credentials
//! (proving the test really placed the child in the hostile-configured
//! state — the denial is not vacuous).
//!
//! No live attempt is possible from this surface today (no
//! non-interactive AI verb exists; `doctor` never queries a provider),
//! so the AI-attempt half of the proof lives in-process in
//! `omen-agent/tests/live_gate_tests.rs` (fully-configured providers
//! over the real transport refuse with the exact signal, fail-closed,
//! never substituted). Together they cover the full execution tree:
//! in-process emission and spawned-child emission share one gate.
//!
//! Keys below are synthetic fixtures (`…-not-real`), never real
//! credentials. Serial env guard: these tests mutate process env.

use serde_json::Value;
use std::process::Command;
use std::sync::Mutex;
use tempfile::tempdir;

static ENV_MUTEX: Mutex<()> = Mutex::new(());

struct EnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, prev) in &self.saved {
            unsafe {
                match prev {
                    Some(v) => std::env::set_var(k, v.clone()),
                    None => std::env::remove_var(k),
                }
            }
        }
    }
}

fn set_var(key: &'static str, value: &str, saved: &mut Vec<(&'static str, Option<String>)>) {
    saved.push((key, std::env::var(key).ok()));
    unsafe { std::env::set_var(key, value) };
}

/// Spawns the real `omen` binary with an isolated state root and returns
/// parsed `doctor --json` output.
fn doctor_json() -> Value {
    let workspace = tempdir().expect("temporary workspace");
    let state = tempdir().expect("temporary state root");
    let output = Command::new(env!("CARGO_BIN_EXE_omen"))
        .args(["doctor", "--json"])
        .arg("--workspace")
        .arg(workspace.path())
        .env("OMEN_STATE_HOME", state.path())
        .output()
        .expect("run real omen doctor");
    assert!(
        output.status.success(),
        "doctor must succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("doctor output is JSON")
}

#[test]
fn spawned_omen_reports_test_isolation_over_admission() {
    let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let mut saved = Vec::new();
    set_var("OMEN_HERMETIC_TESTS", "1", &mut saved);
    set_var(
        "OPENAI_API_KEY",
        "sk-luna-child-fixture-not-real",
        &mut saved,
    );
    set_var(
        "ANTHROPIC_API_KEY",
        "sk-ant-child-fixture-not-real",
        &mut saved,
    );
    let _restore = EnvGuard { saved };

    let doctor = doctor_json();
    let egress = &doctor["ai_egress"];
    assert_eq!(
        egress["admitted"], true,
        "the real binary admits at startup (production behavior preserved)"
    );
    assert_eq!(
        egress["test_isolation"], true,
        "inherited denial must be active in the spawned child"
    );
    let configured = egress["configured"]
        .as_array()
        .expect("configured provider id list");
    let ids: Vec<&str> = configured.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        ids.contains(&"openai-luna"),
        "child must have resolved Luna from its synthetic ambient credential (non-vacuous denial), got: {ids:?}"
    );
    assert!(
        ids.contains(&"anthropic-sonnet"),
        "child must have resolved Sonnet from its synthetic ambient credential, got: {ids:?}"
    );

    // No credential material and no live-attempt markers anywhere.
    let raw = serde_json::to_string(&doctor).unwrap();
    assert!(!raw.contains("sk-luna-child-fixture-not-real"));
    assert!(!raw.contains("sk-ant-child-fixture-not-real"));
    assert!(!raw.to_lowercase().contains("bearer"));
}

#[test]
fn human_doctor_names_denial_only_when_active() {
    let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let mut saved = Vec::new();
    set_var("OMEN_HERMETIC_TESTS", "1", &mut saved);
    let _restore = EnvGuard { saved };

    let workspace = tempdir().expect("temporary workspace");
    let state = tempdir().expect("temporary state root");
    let output = Command::new(env!("CARGO_BIN_EXE_omen"))
        .args(["doctor"])
        .arg("--workspace")
        .arg(workspace.path())
        .env("OMEN_STATE_HOME", state.path())
        .output()
        .expect("run real omen doctor");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("test isolation"),
        "human doctor must name the denial when active, got: {text}"
    );
}
