//! E2.6 hostile proofs: concurrent cancels, timeout/cancel races, and
//! timeout replay truth. Every scenario is bounded by the outer test
//! watchdog with named phases; a timeout names the wedged phase.
//!
//! Invariants under attack:
//! - N concurrent cancels: no panic, no duplicate terminal record, one
//!   coherent receipt/history truth.
//! - timeout racing cancel: EITHER outcome is physically truthful, but
//!   exactly one history record exists and the receipt matches it.
//! - timeout replay: resubmitting the same key reproduces the recorded
//!   TimedOut truth (not a hardcoded Completed).

// ENV_LOCK discipline: the guard serializes OMEN_STATE_HOME mutation and
// is held for the whole scenario that depends on it, i.e. across awaits,
// by design. No other lock in this file is awaited while it is held and
// siblings only block on ENV_LOCK itself, so this cannot deadlock.
#![allow(clippy::await_holding_lock)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use omen_client::OmenClient;
use omen_daemon::{DaemonServer, WorkspaceRegistry};
use omen_ipc::{CancelOutcome, PlatformStream};
use omen_knowledge::WorkspacePersistence;
use omen_knowledge::db::Database;
use omen_knowledge::history::{HistoryQuery, HistoryStatus, query_history};
use omen_knowledge::workspace::canonical_workspace_db_path_readonly;
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

fn history_entries_for(workspace_root: &std::path::Path, execution_id: &str) -> Vec<HistoryStatus> {
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
        .map(|e| e.status.clone())
        .collect()
}

struct LiveDaemon {
    _server: DaemonServer,
    registry: Arc<WorkspaceRegistry>,
    instance_id: String,
    shutdown_rx: watch::Receiver<bool>,
}

fn start_daemon(endpoint: &str) -> LiveDaemon {
    let server = DaemonServer::new(Some(endpoint.to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let shutdown_rx = server.subscribe_shutdown();
    LiveDaemon {
        _server: server,
        registry,
        instance_id,
        shutdown_rx,
    }
}

async fn connect_client(daemon: &LiveDaemon, session: &str, workspace: &str) -> OmenClient {
    let (c_stream, s_stream) = PlatformStream::duplex_pair(8192);
    let reg = daemon.registry.clone();
    let inst = daemon.instance_id.clone();
    let rx = daemon.shutdown_rx.clone();
    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(s_stream, inst, reg, rx).await;
    });
    let client = OmenClient::from_stream(c_stream, None, Some(session.to_string()))
        .await
        .expect("phase=connect: client should connect");
    client
        .attach_workspace(workspace)
        .await
        .expect("phase=attach: workspace attach");
    client
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_storm_leaves_single_coherent_truth() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "cancel_storm_leaves_single_coherent_truth",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_DAEMON");
            let daemon = start_daemon("e2-storm-endpoint");
            let tmp = tempdir().unwrap();
            let path = tmp.path().to_str().unwrap().to_string();
            let gremlin = gremlin_exe().to_string_lossy().into_owned();

            ctx.phase("SUBMIT_LONG_RUNNING");
            let submitter = connect_client(&daemon, "sess_e2_storm_sub", &path).await;
            let ws_for_task = path.clone();
            let gremlin_for_task = gremlin.clone();
            let submit_task = tokio::spawn(async move {
                submitter
                    .submit_consequential_execution(
                        "req-e2-storm-01",
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
                let ws_root = std::path::Path::new(&path);
                let db_path = omen_knowledge::canonical_workspace_db_path(ws_root);
                let db = Database::open(&db_path).expect("daemon database must exist");
                WorkspacePersistence::get_request_receipt(&db, "req-e2-storm-01")
                    .expect("receipt query must work")
                    .expect("Running receipt must exist")
                    .execution_id
                    .expect("running receipt carries the execution id")
            };

            ctx.phase("FIRE_EIGHT_CONCURRENT_CANCELS");
            let mut cancel_tasks = Vec::new();
            for i in 0..8 {
                let canceller = connect_client(&daemon, &format!("sess_e2_storm_{i}"), &path).await;
                let exec_clone = exec_id.clone();
                cancel_tasks.push(tokio::spawn(async move {
                    canceller.cancel_execution(&exec_clone).await
                }));
            }
            let mut confirmed = 0;
            let mut finished = 0;
            for task in cancel_tasks {
                let record = tokio::time::timeout(std::time::Duration::from_secs(90), task)
                    .await
                    .expect("phase=cancel-wait: each cancel must resolve bounded")
                    .expect("cancel task must not panic")
                    .expect("cancel RPC must succeed");
                match record.outcome {
                    CancelOutcome::TerminationConfirmed => confirmed += 1,
                    CancelOutcome::AlreadyFinished { .. } => finished += 1,
                    other => panic!("storm cancel must confirm or observe finish, got {other:?}"),
                }
            }

            ctx.phase("VERIFY_SINGLE_TRUTH");
            // Every storm cancel either observed the confirmed stop or the
            // already-terminal outcome: all reports agree on one coherent
            // truth (no Unknown, no NotFound, no panic, no divergence).
            assert_eq!(
                confirmed + finished,
                8,
                "all 8 storm cancels must resolve coherently"
            );
            assert!(
                confirmed >= 1,
                "at least one cancel must observe the confirmed stop"
            );
            let summary = submit_task.await.unwrap().unwrap();
            assert_eq!(summary.runtime_status, omen_core::RuntimeStatus::Cancelled);
            let entries = history_entries_for(std::path::Path::new(&path), &exec_id);
            assert_eq!(
                entries,
                vec![HistoryStatus::Cancelled],
                "storm must leave exactly one CANCELLED history record"
            );
        },
    )
    .await;
    unlock_env(guard);
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_racing_cancel_yields_exactly_one_truthful_record() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "timeout_racing_cancel_yields_exactly_one_truthful_record",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_DAEMON");
            let daemon = start_daemon("e2-race-endpoint");
            let tmp = tempdir().unwrap();
            let path = tmp.path().to_str().unwrap().to_string();
            let gremlin = gremlin_exe().to_string_lossy().into_owned();

            ctx.phase("SUBMIT_WITH_TIGHT_DEADLINE");
            let submitter = connect_client(&daemon, "sess_e2_race_sub", &path).await;
            let ws_for_task = path.clone();
            let gremlin_for_task = gremlin.clone();
            let submit_task = tokio::spawn(async move {
                submitter
                    .submit_consequential_execution(
                        "req-e2-race-01",
                        "exec",
                        gremlin_for_task,
                        vec!["--sleep-ms".into(), "30000".into()],
                        ws_for_task,
                        1200,
                    )
                    .await
            });
            // Fire cancel just before the 1200ms child deadline: either the
            // deadline or the stop wins the race. Both are physically true.
            tokio::time::sleep(std::time::Duration::from_millis(900)).await;
            let exec_id = {
                let ws_root = std::path::Path::new(&path);
                let db_path = omen_knowledge::canonical_workspace_db_path(ws_root);
                let db = Database::open(&db_path).expect("daemon database must exist");
                WorkspacePersistence::get_request_receipt(&db, "req-e2-race-01")
                    .expect("receipt query must work")
                    .expect("receipt must exist")
                    .execution_id
                    .expect("receipt carries the execution id")
            };
            let canceller = connect_client(&daemon, "sess_e2_race_cancel", &path).await;

            ctx.phase("RACE_CANCEL_AGAINST_DEADLINE");
            let record = canceller.cancel_execution(&exec_id).await.unwrap();
            let summary = submit_task.await.unwrap().unwrap();

            ctx.phase("VERIFY_ONE_RECORD_MATCHES_RECEIPT");
            let entries = history_entries_for(std::path::Path::new(&path), &exec_id);
            assert_eq!(
                entries.len(),
                1,
                "race must leave exactly one history record (got {entries:?}, cancel said {:?})",
                record.outcome
            );
            match record.outcome {
                CancelOutcome::TerminationConfirmed => {
                    assert_eq!(summary.runtime_status, omen_core::RuntimeStatus::Cancelled);
                    assert_eq!(entries, vec![HistoryStatus::Cancelled]);
                }
                CancelOutcome::AlreadyFinished { .. } => {
                    assert_eq!(summary.runtime_status, omen_core::RuntimeStatus::TimedOut);
                    assert_eq!(entries, vec![HistoryStatus::TimedOut]);
                }
                other => panic!("race outcome must be confirm-or-finish, got {other:?}"),
            }
        },
    )
    .await;
    unlock_env(guard);
}

#[tokio::test(flavor = "multi_thread")]
async fn broker_timeout_replays_recorded_truth() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "broker_timeout_replays_recorded_truth",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_DAEMON");
            let daemon = start_daemon("e2-timeout-replay-endpoint");
            let tmp = tempdir().unwrap();
            let path = tmp.path().to_str().unwrap().to_string();
            let gremlin = gremlin_exe().to_string_lossy().into_owned();
            let client = connect_client(&daemon, "sess_e2_timeout", &path).await;

            ctx.phase("TIMEOUT_ONCE");
            let first = client
                .submit_consequential_execution(
                    "req-e2-timeout-01",
                    "exec",
                    gremlin.clone(),
                    vec!["--sleep-ms".into(), "30000".into()],
                    path.clone(),
                    400,
                )
                .await
                .unwrap();
            assert_eq!(first.runtime_status, omen_core::RuntimeStatus::TimedOut);
            assert_eq!(first.exit_code, None);
            let exec_id = first.execution_id.clone();

            ctx.phase("VERIFY_TIMEOUT_HISTORY");
            let entries = history_entries_for(std::path::Path::new(&path), &exec_id);
            assert_eq!(
                entries,
                vec![HistoryStatus::TimedOut],
                "broker timeout must project TIMED_OUT in history, not UNKNOWN"
            );

            ctx.phase("RESUBMIT_REPLAYS_TRUTH");
            let second = client
                .submit_consequential_execution(
                    "req-e2-timeout-01",
                    "exec",
                    gremlin,
                    vec!["--sleep-ms".into(), "30000".into()],
                    path.clone(),
                    400,
                )
                .await
                .unwrap();
            assert_eq!(
                second.execution_id, exec_id,
                "replay must return the recorded execution, not a new one"
            );
            assert_eq!(
                second.runtime_status,
                omen_core::RuntimeStatus::TimedOut,
                "replay must reproduce the recorded timeout, not a hardcoded Completed"
            );
            let entries_after = history_entries_for(std::path::Path::new(&path), &exec_id);
            assert_eq!(
                entries_after,
                vec![HistoryStatus::TimedOut],
                "replay must not append a duplicate record"
            );
        },
    )
    .await;
    unlock_env(guard);
}
