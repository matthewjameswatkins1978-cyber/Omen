use omen_core::{ExecutionId, InteractiveSessionId};
use omen_knowledge::{Database, ExecutionHistory, ExecutionRecord, canonical_workspace_db_path};
use omen_knowledge::{HistoryQuery, query_history};
use omen_mcp::{McpServer, protocol::*};
use serde_json::{Value, json};
use std::process::Command;
use std::time::Instant;
use tempfile::tempdir;

fn seed_history(workspace: &std::path::Path) {
    let mut db = Database::open(&canonical_workspace_db_path(workspace)).unwrap();
    let session = InteractiveSessionId::new("sess_history_parity").unwrap();
    for (id, command, exit_code, created_at) in [
        (
            "exec-history-1",
            "cargo test",
            Some(0),
            "2026-01-01T00:00:01Z",
        ),
        (
            "exec-history-2",
            "cargo check",
            Some(1),
            "2026-01-01T00:00:02Z",
        ),
    ] {
        ExecutionHistory::record_execution(
            &mut db,
            &ExecutionRecord {
                execution_id: ExecutionId::new(id).unwrap(),
                session_id: session.clone(),
                command: command.into(),
                exit_code,
                duration_ms: Some(25),
                stdout_artifact: Some(format!("artifact://stdout/{id}")),
                stderr_artifact: None,
                envelope_json: None,
                created_at: created_at.into(),
            },
            &[],
            &[],
            &[],
        )
        .unwrap();
    }
    ExecutionHistory::record_execution(
        &mut db,
        &ExecutionRecord {
            execution_id: ExecutionId::new("exec-history-timeout").unwrap(),
            session_id: session,
            command: "release-preview".into(),
            exit_code: None,
            duration_ms: Some(100),
            stdout_artifact: None,
            stderr_artifact: None,
            envelope_json: Some(
                r#"{"status":"TIMED_OUT","state_changed":"POSSIBLE","evidence_artifact":"artifact://sha256/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","error":{"code":"TIMEOUT"}}"#.into(),
            ),
            created_at: "2026-01-01T00:00:03Z".into(),
        },
        &[],
        &[],
        &[],
    )
    .unwrap();
}

#[tokio::test]
async fn cli_machine_and_mcp_history_share_canonical_result_after_restart() {
    let temp = tempdir().unwrap();
    seed_history(temp.path());

    let output = Command::new(env!("CARGO_BIN_EXE_omen"))
        .args(["--machine", "history", "--limit", "3"])
        .arg("--workspace")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli: Value = serde_json::from_slice(&output.stdout).unwrap();

    let server = McpServer::new(temp.path().to_path_buf(), None);
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: "tools/call".into(),
            params: Some(json!({
                "name": "omen_history_query",
                "arguments": {"limit": 3}
            })),
        })
        .await;
    let call: CallToolResult = serde_json::from_value(response.result.unwrap()).unwrap();
    let mcp: Value = serde_json::from_str(&call.content[0].text).unwrap();
    assert_eq!(cli, mcp);
    assert_eq!(cli["entries"][0]["status"], "TIMED_OUT");
    assert_eq!(cli["entries"][0]["state_changed"], "POSSIBLE");
    assert_eq!(cli["entries"][1]["status"], "FAILED");
    assert_eq!(cli["entries"][2]["status"], "COMPLETED");
    assert_eq!(cli["ordering"], "sqlite_rowid_desc");
    assert_eq!(
        cli["entries"][0]["evidence"][0],
        "artifact://sha256/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
}

#[tokio::test]
async fn history_query_errors_keep_the_frozen_error_meaning() {
    let temp = tempdir().unwrap();
    seed_history(temp.path());

    let output = Command::new(env!("CARGO_BIN_EXE_omen"))
        .args(["--machine", "history", "--limit", "101"])
        .arg("--workspace")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let cli: Value = serde_json::from_slice(&output.stdout).unwrap();

    let server = McpServer::new(temp.path().to_path_buf(), None);
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(2)),
            method: "tools/call".into(),
            params: Some(json!({
                "name": "omen_history_query",
                "arguments": {"limit": 101}
            })),
        })
        .await;
    let call: CallToolResult = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(
        cli["code"],
        serde_json::to_value(call.error.as_ref().unwrap().code).unwrap()
    );
    assert_eq!(cli["code"], "SCHEMA_VIOLATION");
}

#[tokio::test]
async fn empty_history_is_equivalent_across_cli_and_mcp() {
    let temp = tempdir().unwrap();
    {
        let _db = Database::open(&canonical_workspace_db_path(temp.path())).unwrap();
    }

    let output = Command::new(env!("CARGO_BIN_EXE_omen"))
        .args(["--machine", "history"])
        .arg("--workspace")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let cli: Value = serde_json::from_slice(&output.stdout).unwrap();

    let server = McpServer::new(temp.path().to_path_buf(), None);
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(3)),
            method: "tools/call".into(),
            params: Some(json!({
                "name": "omen_history_query",
                "arguments": {}
            })),
        })
        .await;
    let call: CallToolResult = serde_json::from_value(response.result.unwrap()).unwrap();
    let mcp: Value = serde_json::from_str(&call.content[0].text).unwrap();
    assert_eq!(cli, mcp);
    assert!(cli["entries"].as_array().unwrap().is_empty());
}

#[test]
fn history_query_observations_cover_empty_ten_and_hundred_entries() {
    for count in [0usize, 10, 100] {
        let temp = tempdir().unwrap();
        let db_path = canonical_workspace_db_path(temp.path());
        let mut db = Database::open(&db_path).unwrap();
        let session = InteractiveSessionId::new("sess_history_measure").unwrap();
        for index in 0..count {
            ExecutionHistory::record_execution(
                &mut db,
                &ExecutionRecord {
                    execution_id: ExecutionId::new(format!("exec-measure-{index}")).unwrap(),
                    session_id: session.clone(),
                    command: "cargo check".into(),
                    exit_code: Some(0),
                    duration_ms: Some(1),
                    stdout_artifact: None,
                    stderr_artifact: None,
                    envelope_json: None,
                    created_at: format!("2026-01-01T00:00:{index:02}Z"),
                },
                &[],
                &[],
                &[],
            )
            .unwrap();
        }
        drop(db);
        let read_db = Database::open_read_only(&db_path).unwrap();
        let start = Instant::now();
        let result = query_history(
            &read_db,
            &HistoryQuery {
                all_sessions: true,
                session_id: None,
                limit: 100,
            },
        )
        .unwrap();
        println!(
            "history_query entries={count} elapsed_us={}",
            start.elapsed().as_micros()
        );
        assert_eq!(result.entries.len(), count.min(100));
    }
}


#[tokio::test]
async fn direct_local_execution_status_is_equivalent_across_cli_and_mcp() {
    let temp = tempdir().unwrap();

    let executed = Command::new(env!("CARGO_BIN_EXE_omen"))
        .args(["--machine", "exec", "rustc", "--", "--version"])
        .arg("--workspace")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(
        executed.status.success(),
        "{}",
        String::from_utf8_lossy(&executed.stderr)
    );
    let execution: Value = serde_json::from_slice(&executed.stdout).unwrap();
    assert!(execution["execution_id"].as_str().unwrap().starts_with("exec_"));
    assert_eq!(execution["exit_code"], 0);
    assert!(execution["stdout_artifact_uri"]
        .as_str()
        .unwrap()
        .starts_with("artifact://sha256/"));
    assert!(execution["stderr_artifact_uri"]
        .as_str()
        .unwrap()
        .starts_with("artifact://sha256/"));

    let cli_output = Command::new(env!("CARGO_BIN_EXE_omen"))
        .args(["--machine", "history"])
        .arg("--workspace")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(cli_output.status.success());
    let cli: Value = serde_json::from_slice(&cli_output.stdout).unwrap();

    let server = McpServer::new(temp.path().to_path_buf(), None);
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(4)),
            method: "tools/call".into(),
            params: Some(json!({
                "name": "omen_history_query",
                "arguments": {}
            })),
        })
        .await;
    let call: CallToolResult = serde_json::from_value(response.result.unwrap()).unwrap();
    let mcp: Value = serde_json::from_str(&call.content[0].text).unwrap();

    assert_eq!(cli, mcp);
    assert_eq!(cli["history_status"], "UNJOURNALED_LOCAL_EXECUTION");
    assert_eq!(
        cli["local_execution"]["execution_id"],
        execution["execution_id"]
    );
}

#[test]
fn human_exec_propagates_child_failure_and_stderr() {
    let temp = tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_omen"))
        .args(["exec", "rustc", "--", "--definitely-not-a-rustc-option"])
        .arg("--workspace")
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(!output.status.success(), "child failure must reach the shell");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("exec_"), "execution identity must be visible: {stderr}");
    assert!(
        stderr.contains("stderr evidence: artifact://sha256/"),
        "failure evidence must be visible: {stderr}"
    );
}

#[test]
fn closed_stdin_does_not_turn_ripgrep_into_empty_stdin_search() {
    if Command::new("rg").arg("--version").output().is_err() {
        eprintln!("SKIP: ripgrep is unavailable on this host");
        return;
    }

    let temp = tempdir().unwrap();
    std::fs::write(
        temp.path().join("needle.txt"),
        "OMEN_STDIN_REGRESSION_NEEDLE\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_omen"))
        .args([
            "--machine",
            "exec",
            "rg",
            "--",
            "OMEN_STDIN_REGRESSION_NEEDLE",
        ])
        .arg("--workspace")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["exit_code"], 0);
    assert!(
        result["stdout_bounded"]
            .as_str()
            .unwrap()
            .contains("needle.txt"),
        "ripgrep must search the workspace rather than an EOF pipe: {result}"
    );
}
