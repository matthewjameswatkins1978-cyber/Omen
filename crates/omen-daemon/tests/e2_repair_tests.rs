//! OMEN 0.9-E2 repair proofs: terminal dedup, pre-dispatch cancel truth,
//! persistence fail-closed behaviour, exact runtime replay, and Failed
//! receipt identity preservation.
//!
//! Governing invariant: a consequential request ID may create physical work
//! only when no durable receipt for that ID exists. Once a receipt exists,
//! the identity is consumed — no state silently falls through to dispatch.
//! And: the same execution must not change its story merely because the
//! daemon restarted (exact `RuntimeStatus` survives durable replay; the
//! coarse history label is presentation only).
//! All durable assertions go through the read-only history path or the
//! durable receipt table, never through in-memory caches.
//
// ENV_LOCK discipline: the guard serializes OMEN_STATE_HOME mutation and
// is held for the whole scenario that depends on it, i.e. across awaits,
// by design. No other lock in this file is awaited while it is held and
// siblings only block on ENV_LOCK itself, so this cannot deadlock.
#![allow(clippy::await_holding_lock)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use omen_core::{ExecutionId, InteractiveSessionId};
use omen_daemon::workspace::{
    BrokerExecutionParams, PersistenceFailpoint, PrespawnGate, WorkspaceState,
};
use omen_engine::backend::ExecutionWaitFuture;
use omen_engine::{
    BackendAvailability, BackendCapabilities, BackendDescriptor, BackendKind, ExecutionBackend,
    ExecutionHandle, ProcessSupervisor, PtyExecutionHandle, PtyExecutionRequest,
};
use omen_ipc::{CancelOutcome, LocalIpcError};
use omen_knowledge::db::Database;
use omen_knowledge::history::{HistoryQuery, HistoryStatus, HistoryStatusEnvelope, query_history};
use omen_knowledge::workspace::{
    canonical_workspace_db_path, canonical_workspace_db_path_readonly,
};
use omen_knowledge::{
    ExecutionHistory, ExecutionRecord, RequestReceiptRecord, WorkspacePersistence,
};
use omen_test_fixtures::{INTEGRATION_TIMEOUT, run_with_test_timeout, wait_for_condition};
use tempfile::tempdir;

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

/// All history statuses recorded for one execution ID (read-only path).
fn history_statuses_for(
    workspace_root: &std::path::Path,
    execution_id: &str,
) -> Vec<HistoryStatus> {
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

/// Total history rows in the workspace (read-only path).
fn total_history_rows(workspace_root: &std::path::Path) -> usize {
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
    history.entries.len()
}

async fn receipt_status_of(
    ws: &Arc<WorkspaceState>,
    dedup_id: &str,
) -> Option<(String, Option<String>)> {
    ws.query_request_receipt(dedup_id)
        .await
        .map(|rec| (rec.status, rec.execution_id))
}

async fn submit_consequential(
    ws: &Arc<WorkspaceState>,
    dedup_id: &'static str,
    tool: String,
    args: Vec<String>,
    timeout_ms: u64,
) -> Result<omen_ipc::ExecutionResultSummary, LocalIpcError> {
    let operation = String::new();
    let cwd = String::new();
    let session_id = "sess_e2_repair".to_string();
    ws.execute_broker(BrokerExecutionParams {
        dedup_id,
        session_id: &session_id,
        tool: &tool,
        operation: &operation,
        args: &args,
        cwd: &cwd,
        timeout_ms,
    })
    .await
}

fn seed_receipt(workspace_root: &std::path::Path, dedup_id: &str, status: &str) {
    let db_path = canonical_workspace_db_path(workspace_root);
    let db = Database::open(&db_path).expect("daemon database must exist");
    WorkspacePersistence::record_request_receipt(
        &db,
        &RequestReceiptRecord {
            consequential_request_id: dedup_id.into(),
            execution_id: Some(format!("exec-seeded-{dedup_id}")),
            status: status.into(),
            recorded_at: chrono::Utc::now().to_rfc3339(),
        },
    )
    .expect("receipt seeding must succeed");
}

/// REPAIR 1 — a successfully cancelled consequential operation must never
/// resurrect merely because the caller retried the same request ID after a
/// restart (which clears the in-memory execution cache).
#[tokio::test(flavor = "multi_thread")]
async fn cancelled_receipt_survives_restart_without_resurrection() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "cancelled_receipt_survives_restart_without_resurrection",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();
            let gremlin = gremlin_exe().to_string_lossy().into_owned();

            ctx.phase("SUBMIT_AND_CANCEL");
            let exec_id = {
                let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 1).unwrap());
                let ws_submit = Arc::clone(&ws);
                let gremlin_task = gremlin.clone();
                let submitter = tokio::spawn(async move {
                    submit_consequential(
                        &ws_submit,
                        "req-repair-cancel-01",
                        "exec".to_string(),
                        vec![
                            gremlin_task,
                            "--sleep-ms".to_string(),
                            "30000".to_string(),
                        ],
                        60000,
                    )
                    .await
                });
                wait_for_condition(
                    Duration::from_secs(20),
                    Duration::from_millis(20),
                    "Running receipt for req-repair-cancel-01",
                    || {
                        let ws = Arc::clone(&ws);
                        async move {
                            matches!(
                                receipt_status_of(&ws, "req-repair-cancel-01").await,
                                Some((status, _)) if status == "Running"
                            )
                        }
                    },
                )
                .await
                .expect("Running receipt must appear");
                let exec_id = receipt_status_of(&ws, "req-repair-cancel-01")
                    .await
                    .expect("receipt must exist")
                    .1
                    .expect("Running receipt carries the execution id");
                let record = ws.cancel_execution(&exec_id).await;
                assert_eq!(
                    record.outcome,
                    CancelOutcome::TerminationConfirmed,
                    "live cancel of a spawned child must confirm, got {:?} ({})",
                    record.outcome,
                    record.detail
                );
                let summary = submitter.await.unwrap().unwrap();
                assert_eq!(
                    summary.runtime_status,
                    omen_core::RuntimeStatus::Cancelled
                );
                assert_eq!(summary.execution_id, exec_id);
                exec_id
            };

            ctx.phase("VERIFY_CANCELLED_TRUTH_PRE_RESTART");
            assert_eq!(
                history_statuses_for(&ws_path, &exec_id),
                vec![HistoryStatus::Cancelled]
            );

            ctx.phase("RESTART_AND_RESUBMIT_SAME_ID");
            // A fresh WorkspaceState: the in-memory execution cache is gone.
            // The durable Cancelled receipt is the authority now.
            let ws2 = Arc::new(WorkspaceState::new(ws_path.clone(), 2).unwrap());
            assert_eq!(
                receipt_status_of(&ws2, "req-repair-cancel-01").await,
                Some(("Cancelled".to_string(), Some(exec_id.clone()))),
                "Cancelled receipt must survive restart"
            );
            let replay = submit_consequential(
                &ws2,
                "req-repair-cancel-01",
                "exec".to_string(),
                vec![gremlin, "--sleep-ms".to_string(), "30000".to_string()],
                60000,
            )
            .await
            .expect("Cancelled resubmit must replay truth, not error");

            ctx.phase("VERIFY_NO_RESURRECTION");
            assert_eq!(
                replay.execution_id, exec_id,
                "resubmit must return the ORIGINAL execution id — a new id would mean new physical work"
            );
            assert_eq!(
                replay.runtime_status,
                omen_core::RuntimeStatus::Cancelled,
                "replay must reproduce the recorded cancellation truth"
            );
            assert_eq!(
                history_statuses_for(&ws_path, &exec_id),
                vec![HistoryStatus::Cancelled],
                "exactly one original history entry: zero new child spawns, zero new execution IDs"
            );
            assert_eq!(
                receipt_status_of(&ws2, "req-repair-cancel-01").await,
                Some(("Cancelled".to_string(), Some(exec_id))),
                "receipt must stay Cancelled, never re-armed"
            );
        },
    )
    .await;
    unlock_env(guard);
}

/// REPAIR 1 — a terminal Failed receipt must never silently retry physical
/// work when the same consequential request ID is resubmitted.
#[tokio::test(flavor = "multi_thread")]
async fn failed_receipt_never_reexecutes() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "failed_receipt_never_reexecutes",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();
            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 1).unwrap());

            ctx.phase("PRODUCE_TERMINAL_FAILED_RECEIPT");
            let first = submit_consequential(
                &ws,
                "req-repair-failed-01",
                "omen-e2-no-such-binary-xyz".to_string(),
                vec![],
                10000,
            )
            .await;
            assert!(
                first.is_err(),
                "spawn of a nonexistent binary must fail, got {first:?}"
            );
            assert_eq!(
                receipt_status_of(&ws, "req-repair-failed-01")
                    .await
                    .map(|(s, _)| s),
                Some("Failed".to_string()),
                "failed dispatch must leave a terminal Failed receipt"
            );

            ctx.phase("RESUBMIT_SAME_ID");
            let retry = submit_consequential(
                &ws,
                "req-repair-failed-01",
                "omen-e2-no-such-binary-xyz".to_string(),
                vec![],
                10000,
            )
            .await;

            ctx.phase("VERIFY_NO_REEXECUTION");
            match &retry {
                Err(LocalIpcError::RequestDuplicate(message)) => {
                    assert!(
                        message.contains("terminal status 'Failed'"),
                        "refusal must name the terminal truth, got: {message}"
                    );
                    assert!(
                        message.contains("refusing re-execution"),
                        "refusal must be explicit, got: {message}"
                    );
                }
                other => panic!("Failed resubmit must be refused explicitly, got {other:?}"),
            }
            assert_eq!(
                receipt_status_of(&ws, "req-repair-failed-01")
                    .await
                    .map(|(s, _)| s),
                Some("Failed".to_string()),
                "terminal truth remains failure"
            );
            assert_eq!(
                total_history_rows(&ws_path),
                0,
                "no physical execution may be recorded on retry"
            );
        },
    )
    .await;
    unlock_env(guard);
}

/// REPAIR 1 — `Running` / `CancellationRequested` receipts with no live
/// owner (panic/crash boundary after startup reconcile) must reconcile and
/// fail closed, never dispatch.
#[tokio::test(flavor = "multi_thread")]
async fn nonterminal_receipt_without_live_owner_fails_closed() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "nonterminal_receipt_without_live_owner_fails_closed",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE_AND_SEED");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();
            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 1).unwrap());
            // Seeded AFTER construction so startup reconcile cannot hide
            // the execute_broker branch: a live task died (or never
            // existed) without writing a terminal receipt.
            seed_receipt(&ws_path, "req-repair-ownerless-run", "Running");
            seed_receipt(
                &ws_path,
                "req-repair-ownerless-cancelreq",
                "CancellationRequested",
            );

            ctx.phase("RESUBMIT_RUNNING_WITHOUT_OWNER");
            let retry_run = submit_consequential(
                &ws,
                "req-repair-ownerless-run",
                "exec".to_string(),
                vec![gremlin_exe().to_string_lossy().into_owned()],
                10000,
            )
            .await;
            assert!(
                matches!(retry_run, Err(LocalIpcError::ExecutionStatusUnknown(_))),
                "ownerless Running receipt must fail closed, got {retry_run:?}"
            );

            ctx.phase("RESUBMIT_CANCELLATIONREQUESTED_WITHOUT_OWNER");
            let retry_cancelreq = submit_consequential(
                &ws,
                "req-repair-ownerless-cancelreq",
                "exec".to_string(),
                vec![gremlin_exe().to_string_lossy().into_owned()],
                10000,
            )
            .await;
            assert!(
                matches!(
                    retry_cancelreq,
                    Err(LocalIpcError::ExecutionStatusUnknown(_))
                ),
                "ownerless CancellationRequested receipt must fail closed, got {retry_cancelreq:?}"
            );

            ctx.phase("VERIFY_ZERO_SPAWN_AND_RECONCILED");
            assert_eq!(
                total_history_rows(&ws_path),
                0,
                "fail-closed resubmits must spawn nothing"
            );
            assert_eq!(
                receipt_status_of(&ws, "req-repair-ownerless-run")
                    .await
                    .map(|(s, _)| s),
                Some("Unknown".to_string())
            );
            assert_eq!(
                receipt_status_of(&ws, "req-repair-ownerless-cancelreq")
                    .await
                    .map(|(s, _)| s),
                Some("Unknown".to_string())
            );
        },
    )
    .await;
    unlock_env(guard);
}

/// REPAIR 2 — drive the actual broker race (cancel arrives before physical
/// spawn) via the pre-spawn gate: zero spawns, coherent Cancelled truth,
/// and the cancel result must NOT claim a termination that never happened.
#[tokio::test(flavor = "multi_thread")]
async fn broker_predispatch_cancel_reports_dispatch_prevented() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "broker_predispatch_cancel_reports_dispatch_prevented",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE_AND_GATE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();
            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 1).unwrap());
            let gate = PrespawnGate::default();
            ws.set_prespawn_gate(Some(gate.clone()));
            let gremlin = gremlin_exe().to_string_lossy().into_owned();

            ctx.phase("SUBMIT_PARKED_PRE_DISPATCH");
            let ws_submit = Arc::clone(&ws);
            let gremlin_task = gremlin.clone();
            let submitter = tokio::spawn(async move {
                submit_consequential(
                    &ws_submit,
                    "req-repair-predispatch-01",
                    "exec".to_string(),
                    vec![gremlin_task, "--sleep-ms".to_string(), "30000".to_string()],
                    60000,
                )
                .await
            });
            tokio::time::timeout(Duration::from_secs(20), gate.task_parked.notified())
                .await
                .expect("phase=park: broker task must park pre-dispatch");
            let exec_id = receipt_status_of(&ws, "req-repair-predispatch-01")
                .await
                .expect("receipt must exist")
                .1
                .expect("Running receipt carries the execution id");

            ctx.phase("FIRE_CANCEL_WHILE_PARKED");
            let ws_cancel = Arc::clone(&ws);
            let exec_clone = exec_id.clone();
            let canceller =
                tokio::spawn(async move { ws_cancel.cancel_execution(&exec_clone).await });
            // The cancel flag is fired once the durable receipt shows
            // intent; only then is the broker task released into the
            // pre-dispatch check that must observe the fired flag.
            wait_for_condition(
                Duration::from_secs(20),
                Duration::from_millis(20),
                "CancellationRequested receipt for req-repair-predispatch-01",
                || {
                    let ws = Arc::clone(&ws);
                    async move {
                        matches!(
                            receipt_status_of(&ws, "req-repair-predispatch-01").await,
                            Some((status, _)) if status == "CancellationRequested"
                        )
                    }
                },
            )
            .await
            .expect("cancel intent must be recorded while parked");

            ctx.phase("RELEASE_AND_OBSERVE");
            gate.release.notify_one();
            let record = tokio::time::timeout(Duration::from_secs(25), canceller)
                .await
                .expect("phase=cancel-wait: cancel must resolve bounded")
                .unwrap();

            ctx.phase("VERIFY_DISPATCH_PREVENTED_TRUTH");
            assert_eq!(
                record.outcome,
                CancelOutcome::DispatchPrevented,
                "pre-dispatch cancel must NOT claim TerminationConfirmed, got {:?} ({})",
                record.outcome,
                record.detail
            );
            assert!(
                !record.detail.contains("tree-stop issued"),
                "must not claim a tree-stop, got: {}",
                record.detail
            );
            assert!(
                !record.detail.contains("physical death observed"),
                "must not claim observed death, got: {}",
                record.detail
            );
            assert!(
                record.detail.contains("no process was spawned"),
                "must state the zero-spawn fact, got: {}",
                record.detail
            );
            let summary = submitter.await.unwrap().unwrap();
            assert_eq!(summary.runtime_status, omen_core::RuntimeStatus::Cancelled);
            assert!(
                summary.dispatch_prevented,
                "typed dispatch truth must travel in the summary"
            );
            assert_eq!(
                summary.execution_id, exec_id,
                "no new execution ID may be minted"
            );
            assert_eq!(
                history_statuses_for(&ws_path, &exec_id),
                vec![HistoryStatus::Cancelled],
                "exactly one CANCELLED history record: spawn count = 0"
            );
            assert_eq!(
                receipt_status_of(&ws, "req-repair-predispatch-01")
                    .await
                    .map(|(s, _)| s),
                Some("Cancelled".to_string())
            );
            ws.set_prespawn_gate(None);
        },
    )
    .await;
    unlock_env(guard);
}

/// REPAIR 3 — a Running-receipt persistence failure before spawn must
/// refuse dispatch: explicit failure, zero physical work.
#[tokio::test(flavor = "multi_thread")]
async fn running_receipt_persistence_failure_refuses_dispatch() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "running_receipt_persistence_failure_refuses_dispatch",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE_AND_FAILPOINT");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();
            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 1).unwrap());
            ws.set_persistence_failpoint(PersistenceFailpoint::FailRunningReceipt);

            ctx.phase("SUBMIT_WITH_RECEIPT_FAULT");
            let result = submit_consequential(
                &ws,
                "req-repair-noreceipt-01",
                "exec".to_string(),
                vec![
                    gremlin_exe().to_string_lossy().into_owned(),
                    "--stdout".to_string(),
                    "must-never-run".to_string(),
                ],
                10000,
            )
            .await;

            ctx.phase("VERIFY_ZERO_SPAWN_AND_EXPLICIT_FAILURE");
            match &result {
                Err(LocalIpcError::PersistenceFailure {
                    execution_id,
                    stage,
                    ..
                }) => {
                    assert_eq!(stage, "running_receipt");
                    assert!(
                        execution_id.starts_with("exec_"),
                        "failure must carry the execution identity, got {execution_id}"
                    );
                }
                other => panic!("receipt fault must surface explicitly, got {other:?}"),
            }
            assert_eq!(
                receipt_status_of(&ws, "req-repair-noreceipt-01").await,
                None,
                "no durable receipt may exist"
            );
            assert_eq!(
                total_history_rows(&ws_path),
                0,
                "zero physical work: no history may be recorded"
            );
            ws.set_persistence_failpoint(PersistenceFailpoint::Off);
        },
    )
    .await;
    unlock_env(guard);
}

/// REPAIR 3 — history persistence failure after physical execution: the
/// physical outcome stays explicit, the durable failure stays explicit,
/// and the same consequential ID can never execute again.
#[tokio::test(flavor = "multi_thread")]
async fn history_persistence_failure_preserves_truth_and_blocks_replay() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "history_persistence_failure_preserves_truth_and_blocks_replay",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE_AND_FAILPOINT");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();
            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 1).unwrap());
            ws.set_persistence_failpoint(PersistenceFailpoint::FailHistoryWrite);
            let gremlin = gremlin_exe().to_string_lossy().into_owned();

            ctx.phase("SUBMIT_WITH_HISTORY_FAULT");
            let result = submit_consequential(
                &ws,
                "req-repair-nohistory-01",
                "exec".to_string(),
                vec![
                    gremlin,
                    "--stdout".to_string(),
                    "ran-but-unrecorded".to_string(),
                ],
                10000,
            )
            .await;

            ctx.phase("VERIFY_EXPLICIT_DUAL_TRUTH");
            let exec_id = match &result {
                Err(LocalIpcError::PersistenceFailure {
                    execution_id,
                    stage,
                    physical_outcome,
                    ..
                }) => {
                    assert_eq!(stage, "history");
                    assert!(
                        physical_outcome.contains("Some(0)"),
                        "physical outcome (exit 0) must stay explicit, got: {physical_outcome}"
                    );
                    execution_id.clone()
                }
                other => panic!("history fault must surface explicitly, got {other:?}"),
            };
            assert!(
                exec_id.starts_with("exec_"),
                "execution identity must be retained, got {exec_id}"
            );
            assert_eq!(
                receipt_status_of(&ws, "req-repair-nohistory-01")
                    .await
                    .map(|(s, _)| s),
                Some("Running".to_string()),
                "receipt must stay non-terminal: the identity is consumed but not durably terminal"
            );
            assert_eq!(
                total_history_rows(&ws_path),
                0,
                "no durable history may be claimed"
            );

            ctx.phase("VERIFY_IDENTITY_NEVER_REEXECUTES");
            let retry = submit_consequential(
                &ws,
                "req-repair-nohistory-01",
                "exec".to_string(),
                vec![
                    gremlin_exe().to_string_lossy().into_owned(),
                    "--stdout".to_string(),
                    "must-never-run".to_string(),
                ],
                10000,
            )
            .await;
            assert!(
                matches!(retry, Err(LocalIpcError::ExecutionStatusUnknown(_))),
                "identity with failed durability must fail closed, got {retry:?}"
            );
            assert_eq!(total_history_rows(&ws_path), 0, "retry must spawn nothing");
            ws.set_persistence_failpoint(PersistenceFailpoint::Off);
        },
    )
    .await;
    unlock_env(guard);
}

/// REPAIR 3 — terminal-receipt failure after history succeeded: history
/// truth is preserved, and the identity still cannot dispatch again.
#[tokio::test(flavor = "multi_thread")]
async fn terminal_receipt_persistence_failure_blocks_replay() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "terminal_receipt_persistence_failure_blocks_replay",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE_AND_FAILPOINT");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();
            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 1).unwrap());
            ws.set_persistence_failpoint(PersistenceFailpoint::FailTerminalReceipt);
            let gremlin = gremlin_exe().to_string_lossy().into_owned();

            ctx.phase("SUBMIT_WITH_TERMINAL_RECEIPT_FAULT");
            let result = submit_consequential(
                &ws,
                "req-repair-noterminal-01",
                "exec".to_string(),
                vec![
                    gremlin,
                    "--stdout".to_string(),
                    "ran-and-recorded".to_string(),
                ],
                10000,
            )
            .await;

            ctx.phase("VERIFY_HISTORY_PRESERVED_AND_EXPLICIT_FAILURE");
            match &result {
                Err(LocalIpcError::PersistenceFailure { stage, .. }) => {
                    assert_eq!(stage, "terminal_receipt");
                }
                other => panic!("terminal receipt fault must surface explicitly, got {other:?}"),
            }
            assert_eq!(
                total_history_rows(&ws_path),
                1,
                "already-recorded history truth must be preserved"
            );
            assert_eq!(
                receipt_status_of(&ws, "req-repair-noterminal-01")
                    .await
                    .map(|(s, _)| s),
                Some("Running".to_string()),
                "receipt must stay non-terminal so the identity cannot dispatch again"
            );

            ctx.phase("VERIFY_IDENTITY_NEVER_REEXECUTES");
            let retry = submit_consequential(
                &ws,
                "req-repair-noterminal-01",
                "exec".to_string(),
                vec![
                    gremlin_exe().to_string_lossy().into_owned(),
                    "--stdout".to_string(),
                    "must-never-run".to_string(),
                ],
                10000,
            )
            .await;
            assert!(
                matches!(retry, Err(LocalIpcError::ExecutionStatusUnknown(_))),
                "identity with failed terminal durability must fail closed, got {retry:?}"
            );
            assert_eq!(
                total_history_rows(&ws_path),
                1,
                "retry must spawn nothing; the one history row is the original"
            );
            ws.set_persistence_failpoint(PersistenceFailpoint::Off);
        },
    )
    .await;
    unlock_env(guard);
}

/// Repair A stub backend: returns a FIXED terminal outcome through the real
/// supervisor/broker path (no OS wait failure required). Counts spawns so
/// replay tests prove exactly-once physical dispatch.
struct FixedOutcomeHandle {
    status: omen_core::RuntimeStatus,
    code: Option<i32>,
}

impl ExecutionHandle for FixedOutcomeHandle {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn terminate_tree(&mut self) -> Result<(), omen_core::CoreError> {
        Ok(())
    }
    fn wait_bounded(self: Box<Self>, _timeout: Duration) -> ExecutionWaitFuture {
        let (status, code) = (self.status, self.code);
        Box::pin(async move {
            Ok((
                status,
                omen_core::ProcessExit {
                    code,
                    signal: Some("injected-test-outcome".into()),
                },
                Vec::new(),
                Vec::new(),
            ))
        })
    }
    fn wait_cancelable(
        self: Box<Self>,
        _timeout: Duration,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> ExecutionWaitFuture {
        self.wait_bounded(_timeout)
    }
}

struct FixedOutcomeBackend {
    status: omen_core::RuntimeStatus,
    code: Option<i32>,
    spawns: Arc<AtomicUsize>,
}

impl ExecutionBackend for FixedOutcomeBackend {
    fn id(&self) -> omen_core::BackendId {
        omen_core::BackendId::native()
    }
    fn descriptor(&self) -> BackendDescriptor {
        BackendDescriptor {
            id: self.id(),
            name: "FixedOutcome".into(),
            kind: BackendKind::NativeHost,
            availability: BackendAvailability::Available,
            capabilities: BackendCapabilities {
                filesystem: omen_core::EnforcementLevel::Observed,
                network: omen_core::EnforcementLevel::Observed,
                descendants: omen_core::EnforcementLevel::Observed,
                symlink_escape: omen_core::EnforcementLevel::Observed,
                pty: false,
            },
        }
    }
    fn spawn(
        &self,
        _req: &omen_engine::supervisor::ExecutionRequest,
    ) -> Result<Box<dyn ExecutionHandle>, omen_core::CoreError> {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FixedOutcomeHandle {
            status: self.status,
            code: self.code,
        }))
    }
    fn spawn_pty(
        &self,
        _req: &PtyExecutionRequest,
    ) -> Result<Box<dyn PtyExecutionHandle>, omen_core::CoreError> {
        Err(omen_core::CoreError::ExecutionFailed(
            "FixedOutcome has no PTY".into(),
        ))
    }
}

fn stub_workspace(
    ws_path: &std::path::Path,
    epoch: u64,
    status: omen_core::RuntimeStatus,
    code: Option<i32>,
    spawns: &Arc<AtomicUsize>,
) -> Arc<WorkspaceState> {
    Arc::new(
        WorkspaceState::new_with_supervisor(
            ws_path.to_path_buf(),
            epoch,
            Arc::new(ProcessSupervisor::with_backend(Arc::new(
                FixedOutcomeBackend {
                    status,
                    code,
                    spawns: Arc::clone(spawns),
                },
            ))),
        )
        .unwrap(),
    )
}

/// Read the raw durable envelope for one execution ID.
fn raw_envelope_for(workspace_root: &std::path::Path, execution_id: &str) -> Option<String> {
    let db_path = canonical_workspace_db_path(workspace_root);
    let db = Database::open(&db_path).expect("daemon database must exist");
    db.conn()
        .query_row(
            "SELECT envelope_json FROM execution_history WHERE execution_id = ?1",
            rusqlite::params![execution_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
}

/// REPAIR A — the same physical execution must not change its story across
/// a restart: exact `RuntimeStatus` survives durable replay for every
/// broker terminal outcome, while the coarse history label stays
/// presentation-appropriate.
#[tokio::test(flavor = "multi_thread")]
async fn exact_runtime_survives_restart_replay() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "exact_runtime_survives_restart_replay",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();
            let gremlin = gremlin_exe().to_string_lossy().into_owned();
            let mut epoch = 1u64;

            // Each case: submit -> terminal truth -> restart (fresh
            // WorkspaceState, real restart path) -> resubmit same
            // consequential ID -> replay must reproduce the EXACT runtime
            // with the SAME execution ID and no new dispatch.

            ctx.phase("CASE_COMPLETED_EXIT_0");
            {
                let ws = Arc::new(WorkspaceState::new(ws_path.clone(), epoch).unwrap());
                let original = submit_consequential(
                    &ws,
                    "req-replay-completed-0",
                    "exec".to_string(),
                    vec![gremlin.clone(), "--stdout".to_string(), "ok".to_string()],
                    10000,
                )
                .await
                .unwrap();
                assert_eq!(original.runtime_status, omen_core::RuntimeStatus::Completed);
                assert_eq!(original.exit_code, Some(0));
                epoch += 1;
                let ws2 = Arc::new(WorkspaceState::new(ws_path.clone(), epoch).unwrap());
                let replay = submit_consequential(
                    &ws2,
                    "req-replay-completed-0",
                    "exec".to_string(),
                    vec![gremlin.clone(), "--stdout".to_string(), "ok".to_string()],
                    10000,
                )
                .await
                .unwrap();
                assert_eq!(replay.runtime_status, omen_core::RuntimeStatus::Completed);
                assert_eq!(replay.execution_id, original.execution_id);
                assert_eq!(
                    history_statuses_for(&ws_path, &original.execution_id),
                    vec![HistoryStatus::Completed],
                    "coarse label stays COMPLETED"
                );
            }

            ctx.phase("CASE_COMPLETED_NONZERO_EXIT");
            {
                let ws = Arc::new(WorkspaceState::new(ws_path.clone(), epoch).unwrap());
                let original = submit_consequential(
                    &ws,
                    "req-replay-completed-3",
                    "exec".to_string(),
                    vec![gremlin.clone(), "--exit".to_string(), "3".to_string()],
                    10000,
                )
                .await
                .unwrap();
                assert_eq!(original.runtime_status, omen_core::RuntimeStatus::Completed);
                assert_eq!(original.exit_code, Some(3));
                epoch += 1;
                let ws2 = Arc::new(WorkspaceState::new(ws_path.clone(), epoch).unwrap());
                let replay = submit_consequential(
                    &ws2,
                    "req-replay-completed-3",
                    "exec".to_string(),
                    vec![gremlin.clone(), "--exit".to_string(), "3".to_string()],
                    10000,
                )
                .await
                .unwrap();
                assert_eq!(
                    replay.runtime_status,
                    omen_core::RuntimeStatus::Completed,
                    "non-zero exit replays Completed (exit carries the failure), not a guess"
                );
                assert_eq!(replay.execution_id, original.execution_id);
                assert_eq!(
                    history_statuses_for(&ws_path, &original.execution_id),
                    vec![HistoryStatus::Failed],
                    "coarse label stays FAILED while exact runtime stays Completed"
                );
            }

            ctx.phase("CASE_TIMED_OUT");
            {
                let ws = Arc::new(WorkspaceState::new(ws_path.clone(), epoch).unwrap());
                let original = submit_consequential(
                    &ws,
                    "req-replay-timedout",
                    "exec".to_string(),
                    vec![
                        gremlin.clone(),
                        "--sleep-ms".to_string(),
                        "30000".to_string(),
                    ],
                    1200,
                )
                .await
                .unwrap();
                assert_eq!(original.runtime_status, omen_core::RuntimeStatus::TimedOut);
                epoch += 1;
                let ws2 = Arc::new(WorkspaceState::new(ws_path.clone(), epoch).unwrap());
                let replay = submit_consequential(
                    &ws2,
                    "req-replay-timedout",
                    "exec".to_string(),
                    vec![
                        gremlin.clone(),
                        "--sleep-ms".to_string(),
                        "30000".to_string(),
                    ],
                    1200,
                )
                .await
                .unwrap();
                assert_eq!(replay.runtime_status, omen_core::RuntimeStatus::TimedOut);
                assert_eq!(replay.execution_id, original.execution_id);
                assert_eq!(
                    history_statuses_for(&ws_path, &original.execution_id),
                    vec![HistoryStatus::TimedOut]
                );
            }

            ctx.phase("CASE_CANCELLED");
            {
                let ws = Arc::new(WorkspaceState::new(ws_path.clone(), epoch).unwrap());
                let ws_submit = Arc::clone(&ws);
                let gremlin_task = gremlin.clone();
                let submitter = tokio::spawn(async move {
                    submit_consequential(
                        &ws_submit,
                        "req-replay-cancelled",
                        "exec".to_string(),
                        vec![gremlin_task, "--sleep-ms".to_string(), "30000".to_string()],
                        60000,
                    )
                    .await
                });
                wait_for_condition(
                    Duration::from_secs(20),
                    Duration::from_millis(20),
                    "Running receipt for req-replay-cancelled",
                    || {
                        let ws = Arc::clone(&ws);
                        async move {
                            matches!(
                                receipt_status_of(&ws, "req-replay-cancelled").await,
                                Some((status, _)) if status == "Running"
                            )
                        }
                    },
                )
                .await
                .expect("Running receipt must appear");
                let exec_id = receipt_status_of(&ws, "req-replay-cancelled")
                    .await
                    .expect("receipt must exist")
                    .1
                    .expect("Running receipt carries the execution id");
                let record = ws.cancel_execution(&exec_id).await;
                assert_eq!(record.outcome, CancelOutcome::TerminationConfirmed);
                let original = submitter.await.unwrap().unwrap();
                assert_eq!(original.runtime_status, omen_core::RuntimeStatus::Cancelled);
                epoch += 1;
                let ws2 = Arc::new(WorkspaceState::new(ws_path.clone(), epoch).unwrap());
                let replay = submit_consequential(
                    &ws2,
                    "req-replay-cancelled",
                    "exec".to_string(),
                    vec![
                        gremlin.clone(),
                        "--sleep-ms".to_string(),
                        "30000".to_string(),
                    ],
                    60000,
                )
                .await
                .unwrap();
                assert_eq!(replay.runtime_status, omen_core::RuntimeStatus::Cancelled);
                assert_eq!(replay.execution_id, exec_id);
                assert_eq!(
                    history_statuses_for(&ws_path, &exec_id),
                    vec![HistoryStatus::Cancelled]
                );
            }

            ctx.phase("CASE_IO_FAILED_DETERMINISTIC_STUB");
            {
                let spawns = Arc::new(AtomicUsize::new(0));
                let exec_id = {
                    let ws = stub_workspace(
                        &ws_path,
                        epoch,
                        omen_core::RuntimeStatus::IoFailed,
                        None,
                        &spawns,
                    );
                    let original = submit_consequential(
                        &ws,
                        "req-replay-iofailed",
                        "exec".to_string(),
                        vec!["stubbed".to_string()],
                        10000,
                    )
                    .await
                    .unwrap();
                    assert_eq!(
                        original.runtime_status,
                        omen_core::RuntimeStatus::IoFailed,
                        "stub must deliver IoFailed through the real broker path"
                    );
                    assert_eq!(spawns.load(Ordering::SeqCst), 1);
                    original.execution_id.clone()
                };
                // Coarse label is FAILED while exact truth is IoFailed.
                assert_eq!(
                    history_statuses_for(&ws_path, &exec_id),
                    vec![HistoryStatus::Failed]
                );
                epoch += 1;
                // Real restart path (native supervisor): replay must read
                // the DB only — never spawn.
                let ws2 = Arc::new(WorkspaceState::new(ws_path.clone(), epoch).unwrap());
                let replay = submit_consequential(
                    &ws2,
                    "req-replay-iofailed",
                    "exec".to_string(),
                    vec!["stubbed".to_string()],
                    10000,
                )
                .await
                .unwrap();
                assert_eq!(
                    replay.runtime_status,
                    omen_core::RuntimeStatus::IoFailed,
                    "IoFailed must NEVER become Completed across restart"
                );
                assert_eq!(replay.execution_id, exec_id);
                assert_eq!(
                    history_statuses_for(&ws_path, &exec_id),
                    vec![HistoryStatus::Failed],
                    "exactly one history identity; coarse label unchanged"
                );
                assert_eq!(
                    spawns.load(Ordering::SeqCst),
                    1,
                    "replay must not physically dispatch again"
                );
            }

            ctx.phase("CASE_OUTCOME_UNKNOWN_PRESERVED_NOT_REWRITTEN");
            {
                let spawns = Arc::new(AtomicUsize::new(0));
                let exec_id = {
                    let ws = stub_workspace(
                        &ws_path,
                        epoch,
                        omen_core::RuntimeStatus::OutcomeUnknown,
                        None,
                        &spawns,
                    );
                    let original = submit_consequential(
                        &ws,
                        "req-replay-unknown",
                        "exec".to_string(),
                        vec!["stubbed".to_string()],
                        10000,
                    )
                    .await
                    .unwrap();
                    assert_eq!(
                        original.runtime_status,
                        omen_core::RuntimeStatus::OutcomeUnknown
                    );
                    original.execution_id.clone()
                };
                epoch += 1;
                let ws2 = Arc::new(WorkspaceState::new(ws_path.clone(), epoch).unwrap());
                // The Unknown receipt fails closed (identity consumed, no
                // replay, no re-execution) — and the durable story stays
                // Unknown rather than being rewritten.
                let retry = submit_consequential(
                    &ws2,
                    "req-replay-unknown",
                    "exec".to_string(),
                    vec!["stubbed".to_string()],
                    10000,
                )
                .await;
                assert!(
                    matches!(retry, Err(LocalIpcError::ExecutionStatusUnknown(_))),
                    "Unknown identity must fail closed, got {retry:?}"
                );
                let envelope =
                    raw_envelope_for(&ws_path, &exec_id).expect("history row must exist");
                let parsed = HistoryStatusEnvelope::parse(&envelope).expect("envelope must parse");
                assert_eq!(
                    parsed.runtime_status,
                    Some(omen_core::RuntimeStatus::OutcomeUnknown)
                );
                assert_eq!(parsed.status, "OUTCOME_UNKNOWN");
                assert_eq!(
                    history_statuses_for(&ws_path, &exec_id),
                    vec![HistoryStatus::Unknown]
                );
                assert_eq!(spawns.load(Ordering::SeqCst), 1);
            }
        },
    )
    .await;
    unlock_env(guard);
}

/// Seed a crafted history row + Completed receipt (legacy shapes).
fn seed_legacy_case(
    workspace_root: &std::path::Path,
    dedup_id: &str,
    exec_id: &str,
    exit_code: Option<i32>,
    envelope: Option<String>,
) {
    let db_path = canonical_workspace_db_path(workspace_root);
    let mut db = Database::open(&db_path).expect("daemon database must exist");
    let record = ExecutionRecord {
        execution_id: ExecutionId::new(exec_id).expect("seeded id must be valid"),
        session_id: InteractiveSessionId::new("sess_legacy_repair").expect("session must be valid"),
        command: "legacy-cmd".to_string(),
        exit_code,
        duration_ms: Some(5),
        stdout_artifact: None,
        stderr_artifact: None,
        envelope_json: envelope,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    ExecutionHistory::record_execution(&mut db, &record, &[], &[], &[])
        .expect("legacy history seeding must succeed");
    WorkspacePersistence::record_request_receipt(
        &db,
        &RequestReceiptRecord {
            consequential_request_id: dedup_id.to_string(),
            execution_id: Some(exec_id.to_string()),
            status: "Completed".to_string(),
            recorded_at: chrono::Utc::now().to_rfc3339(),
        },
    )
    .expect("legacy receipt seeding must succeed");
}

/// REPAIR A legacy policy: exact labels replay, ambiguous FAILED-without-exit
/// refuses (never invents Completed), FAILED-with-exit replays Completed
/// (the only FAILED-with-exit producer), envelope-less rows keep the narrow
/// pre-existing fallback.
#[tokio::test(flavor = "multi_thread")]
async fn legacy_envelope_replay_policy() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "legacy_envelope_replay_policy",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE_AND_SEED_LEGACY_ROWS");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();
            // Seed BEFORE construction: rows predate the exact field.
            let exec_a = ExecutionId::generate().to_string();
            let exec_b = ExecutionId::generate().to_string();
            let exec_c = ExecutionId::generate().to_string();
            let exec_d = ExecutionId::generate().to_string();
            seed_legacy_case(
                &ws_path,
                "req-legacy-a",
                &exec_a,
                Some(0),
                Some(r#"{"status":"COMPLETED","source":"broker"}"#.to_string()),
            );
            seed_legacy_case(
                &ws_path,
                "req-legacy-b",
                &exec_b,
                None,
                Some(r#"{"status":"FAILED","source":"broker"}"#.to_string()),
            );
            seed_legacy_case(
                &ws_path,
                "req-legacy-c",
                &exec_c,
                Some(3),
                Some(r#"{"status":"FAILED","source":"broker"}"#.to_string()),
            );
            seed_legacy_case(&ws_path, "req-legacy-d", &exec_d, Some(0), None);
            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 1).unwrap());

            ctx.phase("LEGACY_COMPLETED_REPLAYS");
            let replay_a = submit_consequential(
                &ws,
                "req-legacy-a",
                "exec".to_string(),
                vec!["unused".to_string()],
                10000,
            )
            .await
            .expect("legacy COMPLETED must replay");
            assert_eq!(replay_a.runtime_status, omen_core::RuntimeStatus::Completed);
            assert_eq!(replay_a.execution_id, exec_a);

            ctx.phase("AMBIGUOUS_FAILED_REFUSES");
            let replay_b = submit_consequential(
                &ws,
                "req-legacy-b",
                "exec".to_string(),
                vec!["unused".to_string()],
                10000,
            )
            .await;
            match &replay_b {
                Err(LocalIpcError::InternalRuntimeError(message)) => {
                    assert!(
                        message.contains("no exact runtime truth"),
                        "refusal must name the missing precision, got: {message}"
                    );
                }
                other => panic!("ambiguous legacy FAILED must refuse replay, got {other:?}"),
            }
            assert_eq!(
                receipt_status_of(&ws, "req-legacy-b").await,
                Some(("Completed".to_string(), Some(exec_b.clone()))),
                "refused identity stays consumed with its original truth"
            );
            assert_eq!(
                history_statuses_for(&ws_path, &exec_b).len(),
                1,
                "refusal must spawn nothing"
            );

            ctx.phase("FAILED_WITH_EXIT_REPLAYS_COMPLETED");
            let replay_c = submit_consequential(
                &ws,
                "req-legacy-c",
                "exec".to_string(),
                vec!["unused".to_string()],
                10000,
            )
            .await
            .expect("FAILED-with-exit must replay Completed");
            assert_eq!(replay_c.runtime_status, omen_core::RuntimeStatus::Completed);
            assert_eq!(replay_c.execution_id, exec_c);

            ctx.phase("ENVELOPELESS_KEEPS_NARROW_FALLBACK");
            let replay_d = submit_consequential(
                &ws,
                "req-legacy-d",
                "exec".to_string(),
                vec!["unused".to_string()],
                10000,
            )
            .await
            .expect("envelope-less Completed with exit must replay");
            assert_eq!(replay_d.runtime_status, omen_core::RuntimeStatus::Completed);
            assert_eq!(replay_d.execution_id, exec_d);
        },
    )
    .await;
    unlock_env(guard);
}

/// REPAIR A.1 — the terminal Failed transition must preserve the already
/// minted execution ID (receipts UPSERT): Running(exec_A) -> Failed(exec_A),
/// never Failed(None). Retry is refused with no second identity.
#[tokio::test(flavor = "multi_thread")]
async fn failed_receipt_preserves_execution_identity() {
    let (guard, _state) = lock_env();
    run_with_test_timeout(
        "failed_receipt_preserves_execution_identity",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE_AND_GATE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();
            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 1).unwrap());
            let gate = PrespawnGate::default();
            ws.set_prespawn_gate(Some(gate.clone()));

            ctx.phase("SUBMIT_FAILING_DISPATCH_PARKED");
            let ws_submit = Arc::clone(&ws);
            let submitter = tokio::spawn(async move {
                submit_consequential(
                    &ws_submit,
                    "req-repair-failedid-01",
                    "omen-e2-no-such-binary-xyz".to_string(),
                    vec![],
                    10000,
                )
                .await
            });
            tokio::time::timeout(Duration::from_secs(20), gate.task_parked.notified())
                .await
                .expect("phase=park: broker task must park pre-dispatch");
            let running_exec = receipt_status_of(&ws, "req-repair-failedid-01")
                .await
                .expect("Running receipt must exist");
            assert_eq!(
                running_exec.0, "Running",
                "task must still be pre-dispatch, got {:?}",
                running_exec
            );
            let exec_a = running_exec
                .1
                .expect("Running receipt carries the minted id");

            ctx.phase("RELEASE_INTO_SPAWN_FAILURE");
            gate.release.notify_one();
            let first = tokio::time::timeout(Duration::from_secs(25), submitter)
                .await
                .expect("phase=submit: submit must resolve bounded")
                .unwrap();
            assert!(
                first.is_err(),
                "spawn of a nonexistent binary must fail, got {first:?}"
            );

            ctx.phase("VERIFY_IDENTITY_PRESERVED");
            assert_eq!(
                receipt_status_of(&ws, "req-repair-failedid-01").await,
                Some(("Failed".to_string(), Some(exec_a.clone()))),
                "Failed receipt must preserve the minted execution ID, not erase it"
            );

            ctx.phase("VERIFY_RETRY_REFUSED_WITHOUT_SECOND_IDENTITY");
            let retry = submit_consequential(
                &ws,
                "req-repair-failedid-01",
                "omen-e2-no-such-binary-xyz".to_string(),
                vec![],
                10000,
            )
            .await;
            assert!(
                matches!(retry, Err(LocalIpcError::RequestDuplicate(_))),
                "Failed identity must refuse re-execution, got {retry:?}"
            );
            assert_eq!(
                receipt_status_of(&ws, "req-repair-failedid-01").await,
                Some(("Failed".to_string(), Some(exec_a))),
                "terminal truth and identity unchanged after retry"
            );
            assert_eq!(
                total_history_rows(&ws_path),
                0,
                "failed dispatch records no history and spawns nothing on retry"
            );
            ws.set_prespawn_gate(None);
        },
    )
    .await;
    unlock_env(guard);
}
