//! Workstream A (0.9-D.1): stdin topology must never produce a plausible
//! wrong result in the child.
//!
//! A `Closed` stdin with no payload must not present an empty *readable
//! pipe*: stdin-sensitive tools (ripgrep with no explicit path) treat that
//! as "search this empty stream" instead of their normal no-stdin behavior.
//! These proofs pin the topology per `StdioMode` using the gremlin fixture.

use omen_core::{RequiredAssurance, RuntimeStatus, StdioMode};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use std::path::PathBuf;

fn gremlin_exe() -> PathBuf {
    let mut path = std::env::current_exe().expect("failed to get current_exe");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    let name = if cfg!(windows) {
        "omen-gremlin.exe"
    } else {
        "omen-gremlin"
    };
    let exe = path.join(name);
    if exe.exists() {
        return exe;
    }

    let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target")
        .join("debug")
        .join(name);
    if fallback.exists() {
        return fallback;
    }

    // If binary not found, build it on-demand
    let _ = std::process::Command::new("cargo")
        .args(["build", "--bin", "omen-gremlin"])
        .status();

    if exe.exists() {
        return exe;
    }
    fallback
}

fn request(mode: StdioMode, payload: Option<Vec<u8>>) -> ExecutionRequest {
    ExecutionRequest {
        argv: vec![
            gremlin_exe().to_string_lossy().to_string(),
            "--report-stdin-topology".into(),
        ],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: mode,
        stdin_payload: payload,
        timeout_ms: 10000,
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: vec![],
    }
}

fn topology(output: &omen_engine::ExecutionOutput) -> serde_json::Value {
    let text = String::from_utf8_lossy(&output.stdout_all);
    let line = text
        .lines()
        .find(|line| line.starts_with("STDIN_TOPOLOGY:"))
        .unwrap_or_else(|| panic!("missing STDIN_TOPOLOGY in {text:?}"));
    serde_json::from_str(line.trim_start_matches("STDIN_TOPOLOGY:")).unwrap()
}

#[tokio::test]
async fn closed_stdin_without_payload_is_not_a_readable_pipe() {
    let supervisor = ProcessSupervisor::new();
    let output = supervisor
        .execute(request(StdioMode::Closed, None))
        .await
        .unwrap();
    assert_eq!(output.runtime_status, RuntimeStatus::Completed);
    let topology = topology(&output);
    assert_eq!(topology["is_tty"], false);
    assert_ne!(
        topology["kind"].as_str().unwrap(),
        "pipe",
        "Closed stdin must not present an empty readable pipe"
    );
    assert_eq!(topology["stdin_bytes"], 0);
}

#[tokio::test]
async fn inline_stdin_with_payload_uses_a_pipe() {
    let supervisor = ProcessSupervisor::new();
    let payload = b"explicit-stdin-bytes".to_vec();
    let output = supervisor
        .execute(request(StdioMode::Inline, Some(payload.clone())))
        .await
        .unwrap();
    assert_eq!(output.runtime_status, RuntimeStatus::Completed);
    let topology = topology(&output);
    assert_eq!(topology["kind"].as_str().unwrap(), "pipe");
    assert_eq!(topology["stdin_bytes"], payload.len());
}

#[tokio::test]
async fn closed_stdin_with_explicit_payload_keeps_a_pipe() {
    // Explicit bytes (payload or stdin secrets) still travel over a pipe;
    // only the no-payload Closed case attaches null.
    let supervisor = ProcessSupervisor::new();
    let payload = b"secret-path-bytes".to_vec();
    let output = supervisor
        .execute(request(StdioMode::Closed, Some(payload.clone())))
        .await
        .unwrap();
    assert_eq!(output.runtime_status, RuntimeStatus::Completed);
    let topology = topology(&output);
    assert_eq!(topology["kind"].as_str().unwrap(), "pipe");
    assert_eq!(topology["stdin_bytes"], payload.len());
}

#[tokio::test]
async fn inherit_stdin_completes_without_consuming_the_harness() {
    let supervisor = ProcessSupervisor::new();
    let output = supervisor
        .execute(request(StdioMode::Inherit, None))
        .await
        .unwrap();
    assert_eq!(output.runtime_status, RuntimeStatus::Completed);
    // Topology itself depends on the test harness; the contract is only
    // that Omen does not hang, fail, or rewrite the child's stdin.
    assert!(String::from_utf8_lossy(&output.stdout_all).contains("STDIN_TOPOLOGY:"));
}
