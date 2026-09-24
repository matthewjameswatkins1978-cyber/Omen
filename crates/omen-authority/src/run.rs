//! One consequential step, fully chained:
//!
//! ```text
//! journal terminal? -> status recovery? -> PREPARE -> [ASK -> approval]
//!   -> fresh COMMIT -> verify dispatch -> trusted resolve + bind physical
//!   -> EXECUTE once -> OUTCOME -> journal recorded
//! ```
//!
//! Every arrow-kink fails closed with zero spawn except the single
//! admitted execution. No cached admission, no reused preparation, no
//! second execution for one admission. The physical command is resolved
//! by the Omen-owned trusted resolver AFTER the COMMIT is verified and
//! executed ONLY as a `VerifiedExecutionBinding`: authorised A executes
//! A, nothing else.

use crate::AuthorityError;
use crate::admission::{
    AdmitExecute, ApprovalDecision, AuthorityIntent, CommitOutcome, PrepareOutcome,
};
use crate::binding::{FixtureProvision, VerifiedExecutionBinding, resolve_execution};
use crate::contract::{AuthorityProjection, AuthorityState, HumanOutcome};
use crate::dispatch::{DispatchContext, verify_dispatch};
use crate::executor::{ExecAttempt, PhysicalExecutor};
use crate::outcome::{
    OutcomeDelivered, OutcomeJournal, OutcomeRecord, deliver_outcome, outcome_payload,
};
use crate::protocol::TETHERS_PRODUCT_VERSION;
use crate::transport::GateTransport;
use crate::{AUTHORITY_PROTOCOL, ExpectedGate};
use serde_json::Value;

/// What the operator decided when ASK met them (explicit only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskPolicy {
    Approve,
    Deny,
    Cancel,
    /// No decision: surface approval-required, zero spawn.
    Defer,
}

/// Full report of one driven step (machine + human truth).
#[derive(Debug, Clone)]
pub struct RunReport {
    pub human: HumanOutcome,
    pub projection: AuthorityProjection,
    pub spawn_count: u64,
    pub outcome_terminal: Option<String>,
}

/// Drive one authority-required step to its conclusion.
///
/// `ask` controls the ASK branch. `provision` is the trusted
/// Omen-side installation truth the physical resolver maps the
/// authorised semantic action onto (pre-authority provisioning, never
/// caller argv). `spawn_count` is 0/1 by construction: the executor is
/// invoked at most once, only after a verified COMMIT bound to a
/// verified physical execution.
pub async fn run_once<T, E>(
    driver: &mut AdmitExecute<T>,
    gate_instance_id: Option<String>,
    intent: &AuthorityIntent,
    provision: &FixtureProvision,
    ask: AskPolicy,
    executor: &mut E,
    journal: &OutcomeJournal,
) -> Result<RunReport, AuthorityError>
where
    T: GateTransport,
    E: PhysicalExecutor,
{
    let base_projection = |state: AuthorityState,
                           execution_id: Option<String>,
                           prepared_id: Option<String>,
                           approval_id: Option<String>,
                           approval_consumed: bool,
                           attempted: bool,
                           outcome_state: Option<String>,
                           recovery_required: bool,
                           spawn_count: u64| {
        AuthorityProjection {
            authority_required: true,
            authority_state: state,
            tethers_protocol: AUTHORITY_PROTOCOL.to_string(),
            tethers_product: TETHERS_PRODUCT_VERSION.to_string(),
            gate_instance_id: gate_instance_id.clone(),
            execution_id,
            prepared_id,
            approval_id,
            approval_consumed,
            attempted,
            outcome_state,
            recovery_required,
            spawn_count,
        }
    };
    let halt = |human: HumanOutcome, state: AuthorityState, recovery_required: bool| RunReport {
        human,
        projection: base_projection(
            state,
            None,
            None,
            None,
            false,
            false,
            None,
            recovery_required,
            0,
        ),
        spawn_count: 0,
        outcome_terminal: None,
    };

    // Durable gate first: recovery_required fails closed before any
    // admission work for a fresh execution.
    let view = driver.status().map_err(|e| match e {
        AuthorityError::CommitRefused(m) => {
            AuthorityError::Validate(format!("status.refused: {m}"))
        }
        other => other,
    })?;
    if view.recovery_blocks_execution() {
        let states: Vec<String> = view
            .recovery_required
            .iter()
            .map(|v| {
                v.get("state")
                    .and_then(|s| s.as_str())
                    .unwrap_or("?")
                    .to_string()
                    + "@"
                    + v.get("execution_id")
                        .and_then(|s| s.as_str())
                        .unwrap_or("none")
            })
            .collect();
        return Ok(halt(
            HumanOutcome::StaleRevoked(format!(
                "durable reconciliation requires recovery [{}]; resolve before fresh execution",
                states.join(", ")
            )),
            AuthorityState::StaleRevoked,
            true,
        ));
    }

    let prepared = match driver.prepare(intent)? {
        PrepareOutcome::Deny { reason } => {
            return Ok(halt(
                HumanOutcome::Denied(reason),
                AuthorityState::Denied,
                false,
            ));
        }
        PrepareOutcome::Unavailable { reason } => {
            return Ok(halt(
                HumanOutcome::AuthorityUnavailable(reason),
                AuthorityState::AuthorityUnavailable,
                false,
            ));
        }
        PrepareOutcome::Ask {
            prepared_id,
            approval,
            ..
        } => {
            let approval_id = approval.approval_id.clone();
            match ask {
                AskPolicy::Defer => {
                    return Ok(RunReport {
                        human: HumanOutcome::ApprovalRequired(approval_id.clone()),
                        projection: base_projection(
                            AuthorityState::ApprovalRequired,
                            None,
                            Some(prepared_id),
                            Some(approval_id),
                            false,
                            false,
                            None,
                            false,
                            0,
                        ),
                        spawn_count: 0,
                        outcome_terminal: None,
                    });
                }
                AskPolicy::Approve => {
                    let out = driver.decide_approval(&approval_id, ApprovalDecision::Approve)?;
                    if out.state != "approved" {
                        return Ok(halt(
                            HumanOutcome::StaleRevoked(format!(
                                "approval {approval_id} state {} (want approved)",
                                out.state
                            )),
                            AuthorityState::StaleRevoked,
                            false,
                        ));
                    }
                    (prepared_id, Some(approval_id))
                }
                AskPolicy::Deny => {
                    let _ = driver.decide_approval(&approval_id, ApprovalDecision::Deny)?;
                    return Ok(halt(
                        HumanOutcome::Denied(format!("approval {approval_id} denied by operator")),
                        AuthorityState::Denied,
                        false,
                    ));
                }
                AskPolicy::Cancel => {
                    let _ = driver.decide_approval(&approval_id, ApprovalDecision::Cancel)?;
                    return Ok(halt(
                        HumanOutcome::Denied(format!(
                            "approval {approval_id} cancelled by operator"
                        )),
                        AuthorityState::Denied,
                        false,
                    ));
                }
            }
        }
        PrepareOutcome::AllowPrepared { prepared_id, .. } => (prepared_id, None),
    };
    let (prepared_id, approval_id) = prepared;

    // Fresh COMMIT under current truth (revocation lands here).
    let (dispatch, approval_consumed) = match driver.commit(&prepared_id, approval_id.as_deref())? {
        CommitOutcome::Admitted {
            dispatch,
            approval_consumed,
            ..
        } => (*dispatch, approval_consumed),
        CommitOutcome::Refused { code, message } => {
            return Ok(halt(
                HumanOutcome::StaleRevoked(format!("commit refused: {code}: {message}")),
                AuthorityState::StaleRevoked,
                code.starts_with("commit.replay"),
            ));
        }
    };

    // Exact binding before the physical boundary.
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
    let verified = verify_dispatch(&dispatch, &ctx).map_err(|e| {
        // Binding failure AFTER commit: the admission is spent but Omen
        // executes nothing. Surface as malformed authority.
        AuthorityError::Validate(format!("{e}"))
    })?;
    // Exact physical binding AFTER the verified COMMIT: the trusted
    // Omen resolver derives the physical command from the authorised
    // semantic action, and `bind` verifies it against the dispatch.
    // Binding failure AFTER commit: the admission is spent but Omen
    // executes nothing. Surface as malformed authority.
    let binding = resolve_execution(intent, provision)
        .map_err(|e| AuthorityError::Validate(format!("{e:?}")))?;
    let bound = VerifiedExecutionBinding::bind(&verified, &binding)
        .map_err(|e| AuthorityError::Validate(format!("{e:?}")))?;
    if journal.is_terminal(bound.execution_id()) {
        return Ok(RunReport {
            human: HumanOutcome::StaleRevoked(format!(
                "execution {} already terminal: no duplicate execution",
                bound.execution_id()
            )),
            projection: base_projection(
                AuthorityState::StaleRevoked,
                Some(bound.execution_id().to_string()),
                Some(prepared_id),
                approval_id,
                approval_consumed,
                false,
                Some("terminal_known".to_string()),
                false,
                0,
            ),
            spawn_count: 0,
            outcome_terminal: Some("terminal_known".to_string()),
        });
    }

    // Exactly one physical execution of EXACTLY the verified binding.
    let attempt: ExecAttempt = executor.execute(&bound).await?;
    let spawn_count = u64::from(attempt.attempted);

    // OUTCOME (or deferral when nothing was attempted).
    // Evidence: digest over the captured streams (bounded by the
    // executor); the Gate binds it to the execution, Omen keeps the
    // bytes in its own evidence store.
    let evidence = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(&attempt.stdout);
        h.update(&attempt.stderr);
        Some(format!("sha256:{:x}", h.finalize()))
    };
    let payload = outcome_payload(&bound, &attempt, &intent.success_result, evidence);
    let (outcome_state, terminal, human) = match payload {
        None => {
            journal.record(OutcomeRecord {
                execution_id: bound.execution_id().to_string(),
                action_id: bound.action_id().to_string(),
                capability: bound.capability_name().to_string(),
                attempted: false,
                classification: "not_attempted".to_string(),
                outcome_sent: false,
                terminal: false,
                replay_terminal: None,
            })?;
            (
                Some("deferred_not_attempted".to_string()),
                None,
                HumanOutcome::OutcomeIncomplete(
                    "cancelled before dispatch: outcome deferred, never re-execute silently"
                        .to_string(),
                ),
            )
        }
        Some(payload) => match deliver_outcome(driver.transport_mut(), &payload)? {
            OutcomeDelivered::Recorded(res) => {
                journal.record(OutcomeRecord {
                    execution_id: bound.execution_id().to_string(),
                    action_id: bound.action_id().to_string(),
                    capability: bound.capability_name().to_string(),
                    attempted: true,
                    classification: attempt.classification().as_str().to_string(),
                    outcome_sent: true,
                    terminal: true,
                    replay_terminal: res.replay_terminal.clone(),
                })?;
                let ok =
                    attempt.classification() == crate::protocol::OutcomeClassification::Succeeded;
                (
                    Some(res.status.clone()),
                    res.replay_terminal.clone(),
                    if ok {
                        HumanOutcome::Admitted
                    } else {
                        HumanOutcome::ExecutionFailed(
                            attempt
                                .error_text()
                                .unwrap_or_else(|| "execution failed".to_string()),
                        )
                    },
                )
            }
            OutcomeDelivered::Refused { code, message } => {
                journal.record(OutcomeRecord {
                    execution_id: bound.execution_id().to_string(),
                    action_id: bound.action_id().to_string(),
                    capability: bound.capability_name().to_string(),
                    attempted: true,
                    classification: attempt.classification().as_str().to_string(),
                    outcome_sent: false,
                    terminal: false,
                    replay_terminal: None,
                })?;
                (
                    Some(format!("refused:{code}")),
                    None,
                    HumanOutcome::OutcomeIncomplete(format!("outcome refused: {code}: {message}")),
                )
            }
        },
    };

    let state = human.state();
    Ok(RunReport {
        human,
        projection: base_projection(
            state,
            Some(bound.execution_id().to_string()),
            Some(prepared_id),
            approval_id,
            approval_consumed,
            attempt.attempted,
            outcome_state,
            false,
            spawn_count,
        ),
        spawn_count,
        outcome_terminal: terminal,
    })
}

/// Map a validate-style binding failure into the malformed-authority
/// human truth (used by surfaces that catch it separately).
pub fn malformed(human_detail: String) -> (HumanOutcome, AuthorityState) {
    (
        HumanOutcome::MalformedAuthority(human_detail),
        AuthorityState::MalformedAuthority,
    )
}

/// Human decision parsing for CLI surfaces (`approve|deny|cancel`).
pub fn parse_ask_policy(s: &str) -> Option<AskPolicy> {
    match s {
        "approve" => Some(AskPolicy::Approve),
        "deny" => Some(AskPolicy::Deny),
        "cancel" => Some(AskPolicy::Cancel),
        "defer" => Some(AskPolicy::Defer),
        _ => None,
    }
}

/// Machine-JSON rendering of a report (agents consume this, not prose).
pub fn report_json(report: &RunReport) -> Value {
    serde_json::to_value(&report.projection).unwrap_or(Value::Null)
}

/// Gate identity expectation helper for surfaces wiring hello.
pub fn expected_gate(tethers_source_sha: String, gate_exe_sha256: Option<String>) -> ExpectedGate {
    ExpectedGate::pinned(tethers_source_sha, gate_exe_sha256)
}
