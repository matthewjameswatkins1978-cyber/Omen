//! E2.2 recovery proofs: restart / crash / reconnect preserve documented
//! truth, and retries never duplicate consequential execution.
//!
//! All assertions that an external surface would make go through the
//! CLI/MCP read-only history path (`open_read_only` + `query_history`),
//! never through the daemon's live connection.

// ENV_LOCK discipline: the guard serializes OMEN_STATE_HOME mutation and
// is held for the whole scenario that depends on it, i.e. across awaits,
// by design. No other lock in this file is awaited while it is held and
// siblings only block on ENV_LOCK itself, so this cannot deadlock.
#![allow(clippy::await_holding_lock)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use omen_client::OmenClient;
use omen_daemon::DaemonServer;
use omen_daemon::workspace::{BrokerExecutionParams, WorkspaceState};
use omen_ipc::{ExecutionStatusCode, PlatformStream};
use omen_knowledge::db::Database;
use omen_knowledge::history::{HistoryQuery, query_history};
use omen_knowledge::workspace::{
    canonical_workspace_db_path, canonical_workspace_db_path_readonly,
};
use omen_knowledge::{RequestReceiptRecord, WorkspacePersistence};
use omen_test_fixtures::{INTEGRATION_TIMEOUT, run_with_test_timeout};
use tempfile::tempdir;
use tokio::sync::watch;

static ENV_LOCK: Mutex<()> = Mutex::new(());

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
    assert!(fallback.exists(), "omen-gremlin fixture must be built");
    fallback
}

fn readonly_history_len(workspace_root: &std::path::Path, execution_id: &str) -> usize {
    let db_path = canonical_workspace_db_path_readonly(workspace_root);
    let reader = Database::open_read_only(&db_path).expect("history database must exist");
    let history = query_history(
        &reader,
        &HistoryQuery {
            all_sessions: true,
            session_id: None,
            limit: 100,
        },
    )
    .expect("history query must succeed");
    history
        .entries
        .iter()
        .filter(|e| e.execution_id.as_str() == execution_id)
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn history_after_restart_visible_via_readonly_path() {
    let _guard = ENV_LOCK.lock().expect("env lock poisoned");
    // ENV_LOCK serializes process-global env mutation against sibling tests.
    let state_home = tempdir().expect("state home tempdir");
    unsafe {
        std::env::set_var("OMEN_STATE_HOME", state_home.path());
    }
    run_with_test_timeout(
        "history_after_restart_visible_via_readonly_path",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();
            let gremlin = gremlin_exe().to_string_lossy().into_owned();
            let args = vec![gremlin, "--stdout".to_string(), "e2-restart".to_string()];

            ctx.phase("EXECUTE_EPOCH_1");
            let exec_id = {
                let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 1).unwrap());
                let params = BrokerExecutionParams {
                    dedup_id: "req-e2-restart-01",
                    session_id: "sess_e2_restart",
                    tool: "exec",
                    operation: "",
                    args: &args,
                    cwd: "",
                    timeout_ms: 10000,
                };
                let summary = ws.execute_broker(params).await.unwrap();
                assert_eq!(summary.exit_code, Some(0));
                summary.execution_id.clone()
            };

            ctx.phase("VERIFY_VISIBLE_BEFORE_RESTART");
            assert_eq!(
                readonly_history_len(&ws_path, &exec_id),
                1,
                "execution must be visible via read-only path before restart"
            );

            ctx.phase("RESTART_EPOCH_2");
            let ws2 = Arc::new(WorkspaceState::new(ws_path.clone(), 2).unwrap());

            ctx.phase("VERIFY_SAME_IDENTITY_AFTER_RESTART");
            let receipt = ws2
                .query_request_receipt("req-e2-restart-01")
                .await
                .expect("receipt must survive restart");
            assert_eq!(receipt.status, "Completed");
            assert_eq!(receipt.execution_id.as_deref(), Some(exec_id.as_str()));
            assert_eq!(
                readonly_history_len(&ws_path, &exec_id),
                1,
                "same execution identity must survive restart with no duplicate record"
            );
        },
    )
    .await;
    unsafe {
        std::env::remove_var("OMEN_STATE_HOME");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn response_lost_reconnect_resubmit_does_not_duplicate() {
    let _guard = ENV_LOCK.lock().expect("env lock poisoned");
    let state_home = tempdir().expect("state home tempdir");
    unsafe {
        std::env::set_var("OMEN_STATE_HOME", state_home.path());
    }
    run_with_test_timeout(
        "response_lost_reconnect_resubmit_does_not_duplicate",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_SERVER_AND_CLIENT_1");
            let server = DaemonServer::new(Some("e2-nodup-endpoint".to_string()));
            let registry = server.registry();
            let instance_id = server.instance_id().to_string();
            let (_shutdown_tx, shutdown_rx) = watch::channel(false);

            let tmp = tempdir().unwrap();
            let path = tmp.path().to_str().unwrap().to_string();
            let gremlin = gremlin_exe();

            let (c1_stream, s1_stream) = PlatformStream::duplex_pair(4096);
            let reg1 = registry.clone();
            let inst1 = instance_id.clone();
            let rx1 = shutdown_rx.clone();
            tokio::spawn(async move {
                let _ = DaemonServer::handle_connection(s1_stream, inst1, reg1, rx1).await;
            });
            let client1 =
                OmenClient::from_stream(c1_stream, None, Some("sess_e2_lost".to_string()))
                    .await
                    .unwrap();
            client1.attach_workspace(&path).await.unwrap();

            ctx.phase("SUBMIT_THEN_LOSE_RESPONSE");
            let dedup_key = "req-e2-nodup-01";
            let gremlin_path = gremlin.to_string_lossy().into_owned();
            let ws_path = path.clone();
            tokio::spawn(async move {
                let _ = client1
                    .submit_consequential_execution(
                        dedup_key,
                        "exec",
                        gremlin_path,
                        vec![
                            "--sleep-ms".into(),
                            "500".into(),
                            "--stdout".into(),
                            "e2-nodup".into(),
                        ],
                        ws_path,
                        10000,
                    )
                    .await;
            });
            // Response is lost: the caller is gone before the reply arrives.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            ctx.phase("RECONNECT_AND_RESUBMIT");
            let (c2_stream, s2_stream) = PlatformStream::duplex_pair(4096);
            let reg2 = registry.clone();
            let inst2 = instance_id.clone();
            let rx2 = shutdown_rx.clone();
            tokio::spawn(async move {
                let _ = DaemonServer::handle_connection(s2_stream, inst2, reg2, rx2).await;
            });
            let client2 =
                OmenClient::from_stream(c2_stream, None, Some("sess_e2_retry".to_string()))
                    .await
                    .unwrap();
            client2.attach_workspace(&path).await.unwrap();

            let poll_start = std::time::Instant::now();
            let mut status = client2.query_request_status(dedup_key).await.unwrap();
            while status.status != ExecutionStatusCode::Completed
                && poll_start.elapsed() < std::time::Duration::from_secs(15)
            {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                status = client2.query_request_status(dedup_key).await.unwrap();
            }
            assert_eq!(status.status, ExecutionStatusCode::Completed);
            let exec_id = status.execution_id.clone().unwrap();

            // Retry with the same key: dedup, not re-execution.
            let retry = client2
                .submit_consequential_execution(
                    dedup_key,
                    "exec",
                    gremlin.to_string_lossy().into_owned(),
                    vec![],
                    path.clone(),
                    10000,
                )
                .await
                .unwrap();
            assert_eq!(
                retry.execution_id, exec_id,
                "resubmit with the same key must return the original execution"
            );

            ctx.phase("VERIFY_SINGLE_HISTORY_RECORD");
            assert_eq!(
                readonly_history_len(std::path::Path::new(&path), &exec_id),
                1,
                "lost response + retry must leave exactly one history record"
            );
        },
    )
    .await;
    unsafe {
        std::env::remove_var("OMEN_STATE_HOME");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn abandoned_running_receipt_leaves_no_phantom_history() {
    let _guard = ENV_LOCK.lock().expect("env lock poisoned");
    let state_home = tempdir().expect("state home tempdir");
    unsafe {
        std::env::set_var("OMEN_STATE_HOME", state_home.path());
    }
    run_with_test_timeout(
        "abandoned_running_receipt_leaves_no_phantom_history",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();

            ctx.phase("SEED_CRASHED_RUNNING_RECEIPT");
            let db_path = canonical_workspace_db_path(&ws_path);
            let db = Database::open(&db_path).unwrap();
            WorkspacePersistence::record_request_receipt(
                &db,
                &RequestReceiptRecord {
                    consequential_request_id: "req-e2-crashed-run".into(),
                    execution_id: Some("exec-abandoned-e2".into()),
                    status: "Running".into(),
                    recorded_at: chrono::Utc::now().to_rfc3339(),
                },
            )
            .unwrap();
            drop(db);

            ctx.phase("RESTART_AND_RECONCILE");
            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 2).unwrap());
            let receipt = ws
                .query_request_receipt("req-e2-crashed-run")
                .await
                .expect("receipt must exist");
            assert_eq!(receipt.status, "Unknown");

            ctx.phase("VERIFY_NO_PHANTOM_COMPLETION");
            // Uncertainty stays uncertainty: no history entry may claim the
            // abandoned execution completed (or ran at all, durably).
            assert_eq!(
                readonly_history_len(&ws_path, "exec-abandoned-e2"),
                0,
                "abandoned execution must not gain a phantom history record on recovery"
            );
        },
    )
    .await;
    unsafe {
        std::env::remove_var("OMEN_STATE_HOME");
    }
}
