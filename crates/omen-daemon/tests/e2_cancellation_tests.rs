//! E2.3 cancellation proofs: intent to stop != proof of stop.
//!
//! The daemon fires a stop flag (intent, `CancellationRequested`), performs
//! the physical tree-stop, and then observes the child. Only observed death
//! is `TerminationConfirmed`; anything unconfirmed stays `OutcomeUnknown`.
//! All durable assertions go through the read-only history path.
//
// ENV_LOCK discipline: the guard serializes OMEN_STATE_HOME mutation and
// is held for the whole scenario that depends on it, i.e. across awaits,
// by design. No other lock in this file is awaited while it is held and
// siblings only block on ENV_LOCK itself, so this cannot deadlock.
#![allow(clippy::await_holding_lock)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use omen_client::OmenClient;
use omen_daemon::DaemonServer;
use omen_daemon::workspace::WorkspaceState;
use omen_ipc::{CancelOutcome, ExecutionStatusCode, PlatformStream};
use omen_knowledge::db::Database;
use omen_knowledge::history::{HistoryQuery, HistoryStatus, query_history};
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

fn lock_env() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
    let guard = ENV_LOCK.lock().expect("env lock poisoned");
    let state_home = tempdir().expect("state home tempdir");
    // ENV_LOCK serializes process-global env mutation against sibling tests.
    unsafe {
        std::env::set_var("OMEN_STATE_HOME", state_home.path());
    }
    (guard, state_home)
}

fn unlock_env(_guard: std::sync::MutexGuard<'static, ()>) {
    unsafe {
        std::env::remove_var("OMEN_STATE_HOME");
    }
}

fn history_status_of(
    workspace_root: &std::path::Path,
    execution_id: &str,
) -> Option<HistoryStatus> {
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
        .find(|e| e.execution_id.as_str() == execution_id)
        .map(|e| e.status.clone())
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_during_execution_confirms_termination() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "cancel_during_execution_confirms_termination",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_SERVER_AND_CLIENT");
            let server = DaemonServer::new(Some("e2-cancel-live".to_string()));
            let registry = server.registry();
            let instance_id = server.instance_id().to_string();
            let (_shutdown_tx, shutdown_rx) = watch::channel(false);

            let tmp = tempdir().unwrap();
            let path = tmp.path().to_str().unwrap().to_string();
            let gremlin = gremlin_exe();

            let (c_stream, s_stream) = PlatformStream::duplex_pair(8192);
            let reg = registry.clone();
            let inst = instance_id.clone();
            let rx = shutdown_rx.clone();
            tokio::spawn(async move {
                let _ = DaemonServer::handle_connection(s_stream, inst, reg, rx).await;
            });
            let client =
                OmenClient::from_stream(c_stream, None, Some("sess_e2_cancel".to_string()))
                    .await
                    .unwrap();
            client.attach_workspace(&path).await.unwrap();

            ctx.phase("SUBMIT_LONG_RUNNING");
            let gremlin_path = gremlin.to_string_lossy().into_owned();
            let ws_path = path.clone();
            let submitter = tokio::spawn(async move {
                client
                    .submit_consequential_execution(
                        "req-e2-cancel-01",
                        "exec",
                        gremlin_path,
                        vec!["--sleep-ms".into(), "30000".into()],
                        ws_path,
                        60000,
                    )
                    .await
            });
            // Let the child actually start before requesting the stop.
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;

            ctx.phase("CANCEL_AND_CONFIRM");
            // A second connection observes/cancels by canonical execution ID.
            // The execution ID is not known until submission finishes, so
            // resolve it through the receipt table (durable, live).
            let db_path = canonical_workspace_db_path(std::path::Path::new(&path));
            let exec_id = {
                let db = Database::open(&db_path).expect("daemon database must exist");
                WorkspacePersistence::get_request_receipt(&db, "req-e2-cancel-01")
                    .expect("receipt query must work")
                    .expect("Running receipt must exist")
                    .execution_id
                    .expect("running receipt carries the execution id")
            };
            let (c2_stream, s2_stream) = PlatformStream::duplex_pair(8192);
            let reg2 = registry.clone();
            let inst2 = instance_id.clone();
            let rx2 = shutdown_rx.clone();
            tokio::spawn(async move {
                let _ = DaemonServer::handle_connection(s2_stream, inst2, reg2, rx2).await;
            });
            let canceller =
                OmenClient::from_stream(c2_stream, None, Some("sess_e2_canceller".to_string()))
                    .await
                    .unwrap();
            canceller.attach_workspace(&path).await.unwrap();
            let record = canceller.cancel_execution(&exec_id).await.unwrap();
            assert_eq!(
                record.outcome,
                CancelOutcome::TerminationConfirmed,
                "live cancel must confirm physical death, got {:?} ({})",
                record.outcome,
                record.detail
            );

            ctx.phase("VERIFY_TERMINAL_TRUTH");
            let summary = submitter.await.unwrap().unwrap();
            assert_eq!(
                summary.runtime_status,
                omen_core::RuntimeStatus::Cancelled,
                "broker task must observe the confirmed stop"
            );
            assert_eq!(
                history_status_of(std::path::Path::new(&path), &exec_id),
                Some(HistoryStatus::Cancelled),
                "history must record the confirmed cancellation, not success or failure"
            );
            let status = canceller
                .query_request_status("req-e2-cancel-01")
                .await
                .unwrap();
            assert_eq!(status.status, ExecutionStatusCode::Cancelled);
        },
    )
    .await;
    unlock_env(guard);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_after_completion_reports_already_finished() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "cancel_after_completion_reports_already_finished",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_SERVER_AND_CLIENT");
            let server = DaemonServer::new(Some("e2-cancel-done".to_string()));
            let registry = server.registry();
            let instance_id = server.instance_id().to_string();
            let (_shutdown_tx, shutdown_rx) = watch::channel(false);

            let tmp = tempdir().unwrap();
            let path = tmp.path().to_str().unwrap().to_string();
            let gremlin = gremlin_exe();

            let (c_stream, s_stream) = PlatformStream::duplex_pair(8192);
            let reg = registry.clone();
            let inst = instance_id.clone();
            let rx = shutdown_rx.clone();
            tokio::spawn(async move {
                let _ = DaemonServer::handle_connection(s_stream, inst, reg, rx).await;
            });
            let client =
                OmenClient::from_stream(c_stream, None, Some("sess_e2_canceldone".to_string()))
                    .await
                    .unwrap();
            client.attach_workspace(&path).await.unwrap();

            ctx.phase("COMPLETE_NATURALLY");
            let summary = client
                .submit_consequential_execution(
                    "req-e2-cancel-02",
                    "exec",
                    gremlin.to_string_lossy().into_owned(),
                    vec!["--stdout".into(), "done-fast".into()],
                    path.clone(),
                    10000,
                )
                .await
                .unwrap();
            assert_eq!(summary.runtime_status, omen_core::RuntimeStatus::Completed);

            ctx.phase("CANCEL_TOO_LATE");
            let record = client
                .cancel_execution(&summary.execution_id)
                .await
                .unwrap();
            assert_eq!(
                record.outcome,
                CancelOutcome::AlreadyFinished {
                    terminal_status: "Completed".to_string()
                },
                "cancel after natural completion must not rewrite the outcome"
            );
            assert_eq!(
                history_status_of(std::path::Path::new(&path), &summary.execution_id),
                Some(HistoryStatus::Completed),
                "history must still show the natural completion"
            );
        },
    )
    .await;
    unlock_env(guard);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_unknown_id_reports_not_found() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "cancel_unknown_id_reports_not_found",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_SERVER_AND_CLIENT");
            let server = DaemonServer::new(Some("e2-cancel-missing".to_string()));
            let registry = server.registry();
            let instance_id = server.instance_id().to_string();
            let (_shutdown_tx, shutdown_rx) = watch::channel(false);

            let tmp = tempdir().unwrap();
            let path = tmp.path().to_str().unwrap().to_string();

            let (c_stream, s_stream) = PlatformStream::duplex_pair(8192);
            let reg = registry.clone();
            let inst = instance_id.clone();
            let rx = shutdown_rx.clone();
            tokio::spawn(async move {
                let _ = DaemonServer::handle_connection(s_stream, inst, reg, rx).await;
            });
            let client =
                OmenClient::from_stream(c_stream, None, Some("sess_e2_cancelnf".to_string()))
                    .await
                    .unwrap();
            client.attach_workspace(&path).await.unwrap();

            ctx.phase("CANCEL_GHOST");
            let record = client
                .cancel_execution("exec_does_not_exist_0000")
                .await
                .unwrap();
            assert_eq!(record.outcome, CancelOutcome::NotFound);
        },
    )
    .await;
    unlock_env(guard);
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_disconnect_during_cancel_preserves_terminal_truth() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "provider_disconnect_during_cancel_preserves_terminal_truth",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_SERVER_AND_SUBMITTER");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();
            let path_str = ws_path.to_str().unwrap().to_string();
            let gremlin = gremlin_exe().to_string_lossy().into_owned();

            let server = DaemonServer::new(Some("e2-cancel-drop".to_string()));
            let registry = server.registry();
            let instance_id = server.instance_id().to_string();
            let (_shutdown_tx, shutdown_rx) = watch::channel(false);

            // Provider connection: submits, then vanishes mid-flight.
            let (c1_stream, s1_stream) = PlatformStream::duplex_pair(8192);
            let reg1 = registry.clone();
            let inst1 = instance_id.clone();
            let rx1 = shutdown_rx.clone();
            let provider_conn = tokio::spawn(async move {
                let _ = DaemonServer::handle_connection(s1_stream, inst1, reg1, rx1).await;
            });
            let provider =
                OmenClient::from_stream(c1_stream, None, Some("sess_e2_vanishing".to_string()))
                    .await
                    .unwrap();
            provider.attach_workspace(&path_str).await.unwrap();

            ctx.phase("SUBMIT_LONG_RUNNING");
            let ws_for_task = path_str.clone();
            let gremlin_for_task = gremlin.clone();
            let provider_for_task = provider;
            let submitter = tokio::spawn(async move {
                provider_for_task
                    .submit_consequential_execution(
                        "req-e2-cancel-03",
                        "exec",
                        gremlin_for_task,
                        vec!["--sleep-ms".into(), "30000".into()],
                        ws_for_task,
                        60000,
                    )
                    .await
            });
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            let exec_id = {
                let db = Database::open(&canonical_workspace_db_path(&ws_path))
                    .expect("daemon database must exist");
                WorkspacePersistence::get_request_receipt(&db, "req-e2-cancel-03")
                    .expect("receipt query must work")
                    .expect("Running receipt must exist")
                    .execution_id
                    .expect("running receipt carries the execution id")
            };

            ctx.phase("PROVIDER_VANISHES");
            // The provider connection dies (server task aborted, pending
            // submit detached). Omen still owns the physical work.
            provider_conn.abort();
            drop(submitter);

            ctx.phase("SECOND_CLIENT_CANCELS");
            let (c2_stream, s2_stream) = PlatformStream::duplex_pair(8192);
            let reg2 = registry.clone();
            let inst2 = instance_id.clone();
            let rx2 = shutdown_rx.clone();
            tokio::spawn(async move {
                let _ = DaemonServer::handle_connection(s2_stream, inst2, reg2, rx2).await;
            });
            let canceller =
                OmenClient::from_stream(c2_stream, None, Some("sess_e2_aftermath".to_string()))
                    .await
                    .unwrap();
            canceller.attach_workspace(&path_str).await.unwrap();
            let record = canceller.cancel_execution(&exec_id).await.unwrap();
            assert_eq!(
                record.outcome,
                CancelOutcome::TerminationConfirmed,
                "cancel after provider loss must still confirm the stop"
            );

            ctx.phase("VERIFY_TERMINAL_TRUTH");
            let status = canceller
                .query_request_status("req-e2-cancel-03")
                .await
                .unwrap();
            assert_eq!(status.status, ExecutionStatusCode::Cancelled);
            assert_eq!(
                history_status_of(&ws_path, &exec_id),
                Some(HistoryStatus::Cancelled),
                "provider loss mid-flight must not erase the terminal truth"
            );
        },
    )
    .await;
    unlock_env(guard);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_across_restart_boundary_reports_unknown() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "cancel_across_restart_boundary_reports_unknown",
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
                    consequential_request_id: "req-e2-cancel-crash".into(),
                    execution_id: Some("exec-cancel-abandoned".into()),
                    status: "Running".into(),
                    recorded_at: chrono::Utc::now().to_rfc3339(),
                },
            )
            .unwrap();
            drop(db);

            ctx.phase("RESTART_THEN_CANCEL");
            // Restart reconciles Running → Unknown (no live handle exists).
            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 2).unwrap());
            let record = ws.cancel_execution("exec-cancel-abandoned").await;
            assert_eq!(
                record.outcome,
                CancelOutcome::OutcomeUnknown,
                "cancel with no live handle must preserve uncertainty"
            );
            assert_eq!(
                history_status_of(&ws_path, "exec-cancel-abandoned"),
                None,
                "no phantom history may appear for the abandoned execution"
            );
        },
    )
    .await;
    unlock_env(guard);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_requested_receipt_reconciled_on_restart() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "cancellation_requested_receipt_reconciled_on_restart",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();

            ctx.phase("SEED_CANCELLATION_REQUESTED_RECEIPT");
            let db_path = canonical_workspace_db_path(&ws_path);
            let db = Database::open(&db_path).unwrap();
            WorkspacePersistence::record_request_receipt(
                &db,
                &RequestReceiptRecord {
                    consequential_request_id: "req-e2-cancel-intent".into(),
                    execution_id: Some("exec-cancel-intent".into()),
                    status: "CancellationRequested".into(),
                    recorded_at: chrono::Utc::now().to_rfc3339(),
                },
            )
            .unwrap();
            drop(db);

            ctx.phase("RESTART_AND_VERIFY_UNKNOWN");
            // Intent without a surviving handle is unobservable after a
            // restart: it must become Unknown, never Confirmed.
            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 3).unwrap());
            let receipt = ws
                .query_request_receipt("req-e2-cancel-intent")
                .await
                .expect("receipt must exist");
            assert_eq!(receipt.status, "Unknown");
        },
    )
    .await;
    unlock_env(guard);
}
