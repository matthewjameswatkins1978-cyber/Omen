//! H2 acceptance matrix over the scripted fake Gate (plus real-process
//! supervision tests in `gate_process_tests.rs`).
//!
//! Every case asserts spawn truth via an explicit sentinel: the counting
//! executor writes a marker file per physical execution. Marker absent +
//! zero calls == zero spawn (never inferred from return codes); marker
//! content `spawn-1` == exactly one.

mod support;

use omen_authority::{
    AdmitExecute, AskPolicy, AuthorityState, CountingExecutor, ExecAttempt, GateTransport,
    HumanOutcome, OutcomeJournal, RoundtripError, dispatch::DispatchContext, run::run_once,
    verify_dispatch,
};
use omen_core::{ProcessExit, RuntimeStatus};
use serde_json::json;
use support::*;

fn harness(
    prepare: PrepareScript,
    commit: CommitScript,
    eval: &str,
) -> (
    tempfile::TempDir,
    omen_authority::AuthorityIntent,
    FakeGate,
    std::path::PathBuf,
    OutcomeJournal,
) {
    let dir = tempfile::tempdir().unwrap();
    let intent = test_intent(eval, dir.path());
    let mut gate = FakeGate::new(prepare, commit);
    register_intent(&mut gate, &intent);
    let marker = dir.path().join("spawn.marker");
    let journal = OutcomeJournal::open(dir.path().join("outcome-journal.jsonl"));
    (dir, intent, gate, marker, journal)
}

fn succeeding(marker: &std::path::Path) -> CountingExecutor {
    CountingExecutor::succeeding(Some(marker.to_path_buf()))
}

fn marker_text(marker: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(marker).ok()
}

// 1. ALLOW: prepare -> commit -> exactly one spawn -> outcome success.
#[tokio::test]
async fn m01_allow_exactly_one_spawn_and_success_outcome() {
    let (_dir, intent, gate, marker, journal) =
        harness(PrepareScript::Allow, CommitScript::Admit, "eval_01");
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let report = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("allow path must drive cleanly");
    assert_eq!(report.spawn_count, 1);
    assert_eq!(exec.calls, 1);
    assert_eq!(marker_text(&marker).as_deref(), Some("spawn-1\n"));
    assert!(matches!(report.human, HumanOutcome::Admitted));
    assert_eq!(report.projection.authority_state, AuthorityState::Admitted);
    assert!(report.projection.attempted);
    assert_eq!(
        report.projection.outcome_state.as_deref(),
        Some("succeeded")
    );
    // Journal holds the execution terminal.
    assert!(journal.is_terminal("exec_prep_1_action_1"));
    assert_eq!(driver.transport().outcome_calls, 1);
}

// 2. DENY: zero spawn.
#[tokio::test]
async fn m02_deny_zero_spawn() {
    let (_dir, intent, gate, marker, journal) =
        harness(PrepareScript::Deny, CommitScript::Admit, "eval_02");
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let report = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Approve,
        &mut exec,
        &journal,
    )
    .await
    .expect("deny must surface, not error");
    assert!(matches!(report.human, HumanOutcome::Denied(_)));
    assert_eq!(report.spawn_count, 0);
    assert_eq!(exec.calls, 0);
    assert!(
        marker_text(&marker).is_none(),
        "DENY executed: marker exists"
    );
    assert_eq!(
        driver.transport().commit_calls,
        0,
        "deny must never reach commit"
    );
    assert_eq!(driver.transport().outcome_calls, 0);
}

// 3. ASK before approval: zero spawn.
#[tokio::test]
async fn m03_ask_before_approval_zero_spawn() {
    let (_dir, intent, gate, marker, journal) =
        harness(PrepareScript::Ask, CommitScript::Admit, "eval_03");
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let report = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("ask must surface, not error");
    assert!(matches!(report.human, HumanOutcome::ApprovalRequired(_)));
    assert_eq!(report.spawn_count, 0);
    assert_eq!(exec.calls, 0);
    assert!(
        marker_text(&marker).is_none(),
        "ASK executed before approval"
    );
    assert_eq!(driver.transport().approval_calls, 0, "no approval relayed");
    assert_eq!(
        driver.transport().commit_calls,
        0,
        "no commit without approval"
    );
}

// 4. ASK + APPROVE + COMMIT: exactly one spawn.
#[tokio::test]
async fn m04_ask_approve_commit_exactly_one_spawn() {
    let (_dir, intent, gate, marker, journal) =
        harness(PrepareScript::Ask, CommitScript::Admit, "eval_04");
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let report = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Approve,
        &mut exec,
        &journal,
    )
    .await
    .expect("approved ask must execute");
    assert!(matches!(report.human, HumanOutcome::Admitted));
    assert_eq!(report.spawn_count, 1);
    assert_eq!(exec.calls, 1);
    assert_eq!(marker_text(&marker).as_deref(), Some("spawn-1\n"));
    assert!(report.projection.approval_consumed);
    assert_eq!(driver.transport().approval_calls, 1);
    assert_eq!(driver.transport().commit_calls, 1);
}

// 5. PREPARE then REVOKE before COMMIT: zero spawn.
#[tokio::test]
async fn m05_revoke_before_commit_zero_spawn() {
    let (_dir, intent, mut gate, marker, journal) =
        harness(PrepareScript::Allow, CommitScript::Admit, "eval_05");
    gate.revoke = RevokeMode::CommitOnly;
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let report = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("revocation must surface, not error");
    assert!(matches!(report.human, HumanOutcome::StaleRevoked(_)));
    assert_eq!(report.spawn_count, 0);
    assert_eq!(exec.calls, 0);
    assert!(
        marker_text(&marker).is_none(),
        "REVOKED executed: marker exists"
    );
    assert_eq!(driver.transport().outcome_calls, 0);
}

// 6. RE-ADMIT after revocation: fresh COMMIT -> exactly one spawn.
#[tokio::test]
async fn m06_readmit_after_revocation_exactly_one_spawn() {
    let (_dir, intent, mut gate, marker, journal) =
        harness(PrepareScript::Allow, CommitScript::Admit, "eval_06");
    gate.revoke = RevokeMode::CommitOnly;
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let first = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("first attempt surfaces revocation");
    assert_eq!(first.spawn_count, 0);
    // Authority is valid again: a NEW admission path (fresh evaluation).
    driver.transport_mut().revoke = RevokeMode::None;
    let intent2 = test_intent("eval_06b", &intent.cwd);
    register_intent(driver.transport_mut(), &intent2);
    let second = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent2,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("re-admission must execute");
    assert!(matches!(second.human, HumanOutcome::Admitted));
    assert_eq!(second.spawn_count, 1);
    assert_eq!(exec.calls, 1, "exactly one physical execution total");
    assert_eq!(marker_text(&marker).as_deref(), Some("spawn-1\n"));
}

// 7. Multi-step: step 1 executes; revoked; step 2 zero spawn, no inheritance.
#[tokio::test]
async fn m07_multistep_each_step_readmitted() {
    let (_dir, intent1, gate, marker, journal) =
        harness(PrepareScript::Allow, CommitScript::Admit, "eval_07a");
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let step1 = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent1,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("step 1 executes");
    assert!(matches!(step1.human, HumanOutcome::Admitted));
    assert!(
        journal.is_terminal("exec_prep_1_action_1"),
        "step 1 is historical truth"
    );
    // Authority revoked before step 2.
    driver.transport_mut().revoke = RevokeMode::PrepareAndCommit;
    let intent2 = test_intent("eval_07b", &intent1.cwd);
    register_intent(driver.transport_mut(), &intent2);
    let step2 = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent2,
        AskPolicy::Approve,
        &mut exec,
        &journal,
    )
    .await
    .expect("step 2 refusal surfaces");
    assert!(matches!(step2.human, HumanOutcome::Denied(_)));
    assert_eq!(exec.calls, 1, "step 2 must not inherit step 1 permission");
    assert_eq!(marker_text(&marker).as_deref(), Some("spawn-1\n"));
    assert!(
        journal.is_terminal("exec_prep_1_action_1"),
        "step 1 history untouched"
    );
}

// 13. Malformed/mismatched dispatch proof: zero spawn.
#[tokio::test]
async fn m13_dispatch_prepared_id_mismatch_zero_spawn() {
    let (_dir, intent, gate, marker, journal) = harness(
        PrepareScript::Allow,
        CommitScript::AdmitMutated(support::DispatchMutation::PreparedId),
        "eval_13",
    );
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let err = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect_err("forged dispatch must fail binding");
    assert!(
        format!("{err:?}").contains("prepared_id_mismatch"),
        "got: {err:?}"
    );
    assert_eq!(exec.calls, 0);
    assert!(marker_text(&marker).is_none());
}

// 14. Argument digest mismatch: zero spawn.
#[tokio::test]
async fn m14_argument_digest_mismatch_zero_spawn() {
    let (_dir, intent, gate, marker, journal) = harness(
        PrepareScript::Allow,
        CommitScript::AdmitMutated(support::DispatchMutation::ArgumentDigest),
        "eval_14",
    );
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let err = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect_err("digest mismatch must fail binding");
    assert!(
        format!("{err:?}").contains("argument_digest_mismatch"),
        "got: {err:?}"
    );
    assert_eq!(exec.calls, 0);
    assert!(marker_text(&marker).is_none());
}

// 15. Capability/scope mismatch: zero spawn (each variant).
#[tokio::test]
async fn m15_capability_scope_mismatch_zero_spawn() {
    for mutation in [
        support::DispatchMutation::Capability,
        support::DispatchMutation::CapabilityVersion,
        support::DispatchMutation::ManifestDigest,
        support::DispatchMutation::Provider,
        support::DispatchMutation::OwnershipFlag,
    ] {
        let (_dir, intent, gate, marker, journal) = harness(
            PrepareScript::Allow,
            CommitScript::AdmitMutated(mutation),
            "eval_15",
        );
        let mut driver = AdmitExecute::new(gate);
        let mut exec = succeeding(&marker);
        let err = run_once(
            &mut driver,
            Some("gate_fake".to_string()),
            &intent,
            AskPolicy::Defer,
            &mut exec,
            &journal,
        )
        .await
        .expect_err("capability/scope mismatch must fail binding");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("mismatch") || msg.contains("ownership_inverted"),
            "got: {msg}"
        );
        assert_eq!(exec.calls, 0);
        assert!(
            marker_text(&marker).is_none(),
            "mismatched admission executed"
        );
    }
}

// 16. Response timeout: zero spawn.
#[tokio::test]
async fn m16_authority_response_timeout_zero_spawn() {
    let (_dir, intent, mut gate, marker, journal) =
        harness(PrepareScript::Allow, CommitScript::Admit, "eval_16");
    // Commit never answers within any bound: simulate at the transport
    // layer by refusing the roundtrip as a timeout.
    gate.commit_sleep_ms = 0;
    struct HangingGate(FakeGate);
    impl GateTransport for HangingGate {
        fn label(&self) -> String {
            "fake:hanging".to_string()
        }
        fn roundtrip(
            &mut self,
            operation: &str,
            payload: serde_json::Value,
        ) -> Result<serde_json::Value, RoundtripError> {
            if operation == "commit" {
                return Err(RoundtripError::Transport(
                    omen_authority::AuthorityError::Receive("gate.response.timeout".to_string()),
                ));
            }
            self.0.roundtrip(operation, payload)
        }
        fn shutdown(&mut self) {}
    }
    let mut driver = AdmitExecute::new(HangingGate(gate));
    let mut exec = succeeding(&marker);
    let err = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect_err("hung commit must fail closed");
    assert!(format!("{err:?}").contains("timeout"), "got: {err:?}");
    assert_eq!(exec.calls, 0);
    assert!(marker_text(&marker).is_none());
}

// Direct binding unit: prepare -> commit -> verify_dispatch accepts.
#[tokio::test]
async fn m13b_faithful_dispatch_verifies() {
    let (_dir, intent, gate, _marker, _journal) =
        harness(PrepareScript::Allow, CommitScript::Admit, "eval_13b");
    let mut driver = AdmitExecute::new(gate);
    register_intent(driver.transport_mut(), &intent);
    let prepared_id = match driver.prepare(&intent).expect("prepare works") {
        omen_authority::PrepareOutcome::AllowPrepared { prepared_id, .. } => prepared_id,
        other => panic!("want allow, got {other:?}"),
    };
    let dispatch = match driver.commit(&prepared_id, None).expect("commit works") {
        omen_authority::CommitOutcome::Admitted { dispatch, .. } => *dispatch,
        other => panic!("want admitted, got {other:?}"),
    };
    let ctx = DispatchContext {
        evaluation_id: intent.evaluation_id.clone(),
        action_id: intent.action_id.clone(),
        prepared_id: prepared_id.clone(),
        expected_capability: intent.expected_capability.clone(),
        expected_capability_version: intent.expected_capability_version,
        expected_manifest_digest: intent.expected_manifest_digest.clone(),
        expected_provider: intent.expected_provider.clone(),
        expected_arguments: intent.expected_arguments.clone(),
    };
    let verified = verify_dispatch(&dispatch, &ctx).expect("faithful dispatch verifies");
    assert_eq!(verified.execution_id, format!("exec_{prepared_id}"));
}

// 17. Physical execution failure after valid COMMIT: one spawn + failed outcome.
#[tokio::test]
async fn m17_exec_failure_truthful_failed_outcome() {
    let (_dir, intent, gate, marker, journal) =
        harness(PrepareScript::Allow, CommitScript::Admit, "eval_17");
    let mut driver = AdmitExecute::new(gate);
    let mut exec = CountingExecutor::new(
        Some(marker.clone()),
        ExecAttempt {
            attempted: true,
            runtime: RuntimeStatus::Completed,
            exit: ProcessExit::success(2),
            stdout: b"".to_vec(),
            stderr: b"boom".to_vec(),
            duration_ms: 5,
            omen_exec_id: "omen-exec-test".to_string(),
        },
    );
    let report = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("failed exec still reports");
    assert_eq!(exec.calls, 1, "exactly one spawn even on failure");
    assert_eq!(marker_text(&marker).as_deref(), Some("spawn-1\n"));
    assert!(matches!(report.human, HumanOutcome::ExecutionFailed(_)));
    assert_eq!(report.projection.outcome_state.as_deref(), Some("failed"));
    assert!(journal.is_terminal("exec_prep_1_action_1"));
}

// 18. Cancellation after COMMIT (before dispatch): zero spawn, deferred outcome.
#[tokio::test]
async fn m18_cancel_before_dispatch_zero_spawn_deferred() {
    let (_dir, intent, gate, marker, journal) =
        harness(PrepareScript::Allow, CommitScript::Admit, "eval_18");
    let mut driver = AdmitExecute::new(gate);
    let mut exec = CountingExecutor::new(
        Some(marker.clone()),
        ExecAttempt {
            attempted: false,
            runtime: RuntimeStatus::Cancelled,
            exit: ProcessExit {
                code: None,
                signal: None,
            },
            stdout: Vec::new(),
            stderr: Vec::new(),
            duration_ms: 1,
            omen_exec_id: "omen-exec-test".to_string(),
        },
    );
    let report = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("cancel-before-dispatch defers");
    assert_eq!(
        exec.calls, 1,
        "executor invoked once (it reports not-attempted)"
    );
    assert!(
        marker_text(&marker).is_none(),
        "cancelled dispatch must not spawn"
    );
    assert_eq!(report.spawn_count, 0);
    assert_eq!(
        driver.transport().outcome_calls,
        0,
        "attempted:false is never sent"
    );
    assert!(matches!(report.human, HumanOutcome::OutcomeIncomplete(_)));
}

// 19. OUTCOME response lost: retry succeeds, no second execution.
#[tokio::test]
async fn m19_outcome_loss_no_reexecution() {
    let (_dir, intent, mut gate, marker, journal) =
        harness(PrepareScript::Allow, CommitScript::Admit, "eval_19");
    gate.outcome = support::OutcomeScript::DropFirstThenRecord;
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let report = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("lost outcome retries delivery");
    assert!(matches!(report.human, HumanOutcome::Admitted));
    assert_eq!(exec.calls, 1, "no duplicate physical execution");
    assert_eq!(marker_text(&marker).as_deref(), Some("spawn-1\n"));
    assert_eq!(driver.transport().outcome_calls, 2, "delivery retried once");
    assert!(journal.is_terminal("exec_prep_1_action_1"));
}

// 19c. OUTCOME refused with conflict: surfaces incomplete, no re-execution.
#[tokio::test]
async fn m19c_outcome_conflict_incomplete_no_reexec() {
    let (_dir, intent, mut gate, marker, journal) =
        harness(PrepareScript::Allow, CommitScript::Admit, "eval_19c");
    gate.outcome = support::OutcomeScript::RefuseConflict;
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let report = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("conflict surfaces as incomplete");
    assert!(matches!(report.human, HumanOutcome::OutcomeIncomplete(_)));
    assert_eq!(exec.calls, 1, "no duplicate physical execution");
    assert!(
        !journal.is_terminal("exec_prep_1_action_1"),
        "conflicted outcome is not terminal"
    );
}

// 19b. OUTCOME loss total: surfaces incomplete, still no re-execution.
#[tokio::test]
async fn m19b_outcome_total_loss_incomplete_no_reexec() {
    let (_dir, intent, mut gate, marker, journal) =
        harness(PrepareScript::Allow, CommitScript::Admit, "eval_19b");
    gate.outcome = support::OutcomeScript::LoseAll;
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let err = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect_err("total outcome loss surfaces");
    assert!(
        format!("{err:?}").contains("outcome.delivery.lost"),
        "got: {err:?}"
    );
    assert_eq!(exec.calls, 1);
}

// 20. Restart/recovery terminal-known: no duplicate execution.
#[tokio::test]
async fn m20_terminal_known_no_duplicate_execution() {
    let (_dir, intent, gate, marker, journal) =
        harness(PrepareScript::Allow, CommitScript::Admit, "eval_20");
    journal
        .record(omen_authority::OutcomeRecord {
            execution_id: "exec_prep_1_action_1".to_string(),
            action_id: "action_1".to_string(),
            capability: support::CAPABILITY.to_string(),
            attempted: true,
            classification: "succeeded".to_string(),
            outcome_sent: true,
            terminal: true,
            replay_terminal: Some("recorded".to_string()),
        })
        .unwrap();
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let report = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("terminal-known surfaces, not errors");
    assert!(matches!(report.human, HumanOutcome::StaleRevoked(_)));
    assert_eq!(exec.calls, 0, "terminal execution must never run again");
    assert!(marker_text(&marker).is_none());
    assert_eq!(
        report.projection.outcome_state.as_deref(),
        Some("terminal_known")
    );
}

// 21. Durable reconciliation recovery_required: zero new spawn.
#[tokio::test]
async fn m21_recovery_required_zero_new_spawn() {
    let (_dir, intent, mut gate, marker, journal) =
        harness(PrepareScript::Allow, CommitScript::Admit, "eval_21");
    gate.status = support::StatusScript::RecoveryRequired;
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let report = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("recovery-required surfaces");
    assert!(matches!(report.human, HumanOutcome::StaleRevoked(_)));
    assert!(report.projection.recovery_required);
    assert_eq!(exec.calls, 0);
    assert!(marker_text(&marker).is_none());
    assert_eq!(
        driver.transport().prepare_calls,
        0,
        "no admission work while recovery required"
    );
    assert_eq!(driver.transport().commit_calls, 0);
}

// Unavailable prepare: authority unavailable, zero spawn.
#[tokio::test]
async fn m_unavailable_prepare_zero_spawn() {
    let (_dir, intent, gate, marker, journal) =
        harness(PrepareScript::Unavailable, CommitScript::Admit, "eval_u");
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let report = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("unavailable surfaces");
    assert!(matches!(
        report.human,
        HumanOutcome::AuthorityUnavailable(_)
    ));
    assert_eq!(exec.calls, 0);
    assert!(marker_text(&marker).is_none());
}

// Stale preparation: committing an unknown prepared_id refuses.
#[tokio::test]
async fn m_stale_preparation_commit_refuses() {
    let (_dir, intent, gate, marker, journal) =
        harness(PrepareScript::Allow, CommitScript::Admit, "eval_s");
    let mut driver = AdmitExecute::new(gate);
    register_intent(driver.transport_mut(), &intent);
    let outcome = driver
        .commit("prep_forged_stale", None)
        .expect("roundtrip works");
    assert!(
        matches!(outcome, omen_authority::CommitOutcome::Refused { .. }),
        "stale preparation must refuse, got {outcome:?}"
    );
    let _ = (marker, journal);
}

// Wrong capability at PREPARE echo is impossible by construction here
// (fake echoes); covered at dispatch (m15) and ask-approval (m04 asserts
// approval_consumed binding). Ask-time digest is verified in run_once.
#[test]
fn matrix_compiles_and_intent_sane() {
    let dir = tempfile::tempdir().unwrap();
    let intent = test_intent("eval_x", dir.path());
    assert_eq!(intent.expected_capability, "fixture.ping");
    assert!(!intent.argv.is_empty());
    let d = omen_authority::canonical_digest(&intent.expected_arguments).unwrap();
    assert!(d.starts_with("sha256:"));
    let _ = json!({"ok": true});
}
