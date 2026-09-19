use std::sync::Arc;
use tempfile::tempdir;

use omen_core::InteractiveSessionId;
use omen_daemon::workspace::{BrokerExecutionParams, WorkspaceState};
use omen_ipc::LocalIpcError;
use omen_knowledge::{
    Database, ExecutionHistory, RequestReceiptRecord, WorkspacePersistence,
    canonical_workspace_db_path,
};
use omen_test_fixtures::{INTEGRATION_TIMEOUT, run_with_test_timeout};

#[tokio::test(flavor = "multi_thread")]
async fn test_receipt_execution_id_matches_history_execution_id() {
    run_with_test_timeout(
        "test_receipt_execution_id_matches_history_execution_id",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();

            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 1).unwrap());
            let dedup_id = "req-consequential-01";
            let session_id = "sess_consequential_human";

            let params = BrokerExecutionParams {
                dedup_id,
                session_id,
                tool: "exec",
                operation: "",
                args: &["cargo".into(), "--version".into()],
                cwd: "",
                timeout_ms: 10000,
            };

            ctx.phase("EXECUTE_BROKER");
            let summary = ws.execute_broker(params).await.unwrap();
            let returned_exec_id = summary.execution_id;

            ctx.phase("CHECK_RECEIPT_SQLITE");
            // Check receipt in SQLite
            let receipt = ws
                .query_request_receipt(dedup_id)
                .await
                .expect("Receipt must exist");
            assert_eq!(receipt.status, "Completed");
            assert_eq!(
                receipt.execution_id.as_deref(),
                Some(returned_exec_id.as_str())
            );

            ctx.phase("CHECK_HISTORY_SQLITE");
            // Check history in SQLite
            let db = ws.db();
            let db_lock = db.lock().await;
            let sid = InteractiveSessionId::new(session_id).unwrap();
            let last = ExecutionHistory::get_last_execution(&db_lock, &sid)
                .unwrap()
                .expect("History must be recorded");

            assert_eq!(last.execution_id.as_str(), returned_exec_id.as_str());
            assert_eq!(
                receipt.execution_id.unwrap(),
                last.execution_id.as_str(),
                "Receipt execution ID MUST be identical to history execution ID"
            );
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_running_receipt_after_crash_becomes_unknown_not_retried() {
    run_with_test_timeout(
        "test_running_receipt_after_crash_becomes_unknown_not_retried",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();

            ctx.phase("SEED_RUNNING_RECEIPT");
            // 1. Manually seed a "Running" receipt in SQLite, simulating a crash mid-execution
            let db_path = canonical_workspace_db_path(&ws_path);
            let db = Database::open(&db_path).unwrap();
            WorkspacePersistence::record_request_receipt(
                &db,
                &RequestReceiptRecord {
                    consequential_request_id: "req-crashed-run".into(),
                    execution_id: Some("exec-abandoned-01".into()),
                    status: "Running".into(),
                    recorded_at: chrono::Utc::now().to_rfc3339(),
                },
            )
            .unwrap();
            drop(db);

            ctx.phase("RESTART_DAEMON_WORKSPACE");
            // 2. Start a new daemon WorkspaceState (reconciliation runs on startup)
            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 2).unwrap());

            ctx.phase("VERIFY_RECONCILED_UNKNOWN");
            // Verify receipt transitioned to "Unknown"
            let receipt = ws
                .query_request_receipt("req-crashed-run")
                .await
                .expect("Receipt must exist");
            assert_eq!(
                receipt.status, "Unknown",
                "Running receipt must be reconciled to Unknown on restart"
            );

            ctx.phase("ATTEMPT_REEXECUTION_AND_VERIFY_REFUSAL");
            // 3. Attempt to execute the same consequential request
            let params = BrokerExecutionParams {
                dedup_id: "req-crashed-run",
                session_id: "sess_retry_after_crash",
                tool: "exec",
                operation: "",
                args: &["cargo".into(), "check".into()],
                cwd: "",
                timeout_ms: 10000,
            };

            let result = ws.execute_broker(params).await;
            match result {
                Err(LocalIpcError::ExecutionStatusUnknown(id)) => {
                    assert_eq!(id, "req-crashed-run");
                }
                other => panic!("Expected ExecutionStatusUnknown refusal, got {other:?}"),
            }
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_consequential_request_survives_daemon_restart_without_reexecution() {
    run_with_test_timeout(
        "test_consequential_request_survives_daemon_restart_without_reexecution",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();

            let dedup_id = "req-idempotent-02";
            let session_id = "sess_idem";

            ctx.phase("EXECUTE_SESSION_1");
            // 1. Run in session 1
            {
                let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 1).unwrap());
                let params = BrokerExecutionParams {
                    dedup_id,
                    session_id,
                    tool: "exec",
                    operation: "",
                    args: &["cargo".into(), "--version".into()],
                    cwd: "",
                    timeout_ms: 10000,
                };
                let summary1 = ws.execute_broker(params).await.unwrap();
                assert!(summary1.exit_code == Some(0));
            }

            ctx.phase("RESTART_DAEMON_EPOCH_2");
            // 2. Restart daemon with epoch 2 (in-memory cache is wiped)
            let ws2 = Arc::new(WorkspaceState::new(ws_path.clone(), 2).unwrap());

            ctx.phase("EXECUTE_AGAIN_AND_VERIFY_NO_REEXEC");
            let params2 = BrokerExecutionParams {
                dedup_id,
                session_id,
                tool: "exec",
                operation: "",
                args: &["cargo".into(), "--version".into()],
                cwd: "",
                timeout_ms: 10000,
            };

            let summary2 = ws2.execute_broker(params2).await.unwrap();
            let receipt = ws2.query_request_receipt(dedup_id).await.unwrap();

            assert_eq!(receipt.status, "Completed");
            assert_eq!(summary2.execution_id, receipt.execution_id.unwrap());
        },
    )
    .await;
}
