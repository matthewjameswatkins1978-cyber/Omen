//! Admission orchestration: PREPARE → (ASK → approval) → COMMIT.
//!
//! This layer NEVER spawns the action. It builds exact requests from
//! Omen's current intent, surfaces DENY/ASK/UNAVAILABLE as explicit
//! truth, and returns an admitted [`DispatchRecord`] only for an accepted
//! COMMIT. COMMIT is always fresh — a previous admission is never a
//! reusable permission slip, and approval never becomes permission
//! (it only unlocks a fresh COMMIT).

use crate::AuthorityError;
use crate::protocol::{ApprovalInfo, DispatchRecord, PrepareDecision, PrepareResult};
use crate::transport::{GateTransport, RoundtripError};
use serde_json::{Value, json};

/// Omen's current intent for one consequential step. Everything the Gate
/// needs is derived here; Omen never sends a caller `plan` or any
/// authority boolean (the Gate recomputes every decision).
#[derive(Debug, Clone)]
pub struct AuthorityIntent {
    pub tether_id: String,
    pub tether_version: String,
    pub evaluation_id: String,
    pub action_id: String,
    pub event_id: String,
    pub event_name: String,
    pub event_data: Value,
    pub facts: Value,
    /// Exact action arguments Omen embedded in the event (the digest of
    /// these must equal the admitted `argument_digest`).
    pub expected_arguments: Value,
    pub expected_capability: String,
    pub expected_capability_version: u32,
    pub expected_manifest_digest: String,
    pub expected_provider: String,
    /// Physical command Omen will run after admission (argv-only, exact).
    pub argv: Vec<String>,
    pub cwd: std::path::PathBuf,
    pub timeout_ms: u64,
    /// Success projection Omen will report (`result` on succeeded).
    pub success_result: Value,
}

impl AuthorityIntent {
    pub fn prepare_payload(&self) -> Value {
        json!({
            "action_id": self.action_id,
            "evaluation_id": self.evaluation_id,
            "tether": {"id": self.tether_id, "version": self.tether_version},
            "event": {"id": self.event_id, "name": self.event_name, "data": self.event_data},
            "facts": self.facts,
        })
    }
}

/// What PREPARE concluded (never authorises dispatch).
#[derive(Debug, Clone)]
pub enum PrepareOutcome {
    AllowPrepared {
        prepared_id: String,
        reason: String,
        result: PrepareResult,
    },
    Ask {
        prepared_id: String,
        reason: String,
        approval: ApprovalInfo,
        result: PrepareResult,
    },
    Deny {
        reason: String,
    },
    Unavailable {
        reason: String,
    },
}

/// What an approval decision concluded (never authorises dispatch).
#[derive(Debug, Clone)]
pub struct ApprovalOutcome {
    pub approval_id: String,
    pub state: String,
    pub action_id: String,
}

/// What COMMIT concluded. Only `Admitted` may cross the execution
/// boundary.
#[derive(Debug, Clone)]
pub enum CommitOutcome {
    Admitted {
        dispatch: Box<DispatchRecord>,
        prepared_id: String,
        approval_consumed: bool,
    },
    Refused {
        code: String,
        message: String,
    },
}

/// Human approval input. Explicit only — no implicit approval, no retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Deny,
    Cancel,
}

impl ApprovalDecision {
    fn as_str(&self) -> &'static str {
        match self {
            ApprovalDecision::Approve => "approve",
            ApprovalDecision::Deny => "deny",
            ApprovalDecision::Cancel => "cancel",
        }
    }
}

/// One admission driver over a Gate session. Owns nothing physical.
pub struct AdmitExecute<T: GateTransport> {
    transport: T,
}

impl<T: GateTransport> AdmitExecute<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    /// PREPARE under current truth. Returns explicit decision truth.
    pub fn prepare(&mut self, intent: &AuthorityIntent) -> Result<PrepareOutcome, AuthorityError> {
        let raw = self.roundtrip_ok("prepare", intent.prepare_payload())?;
        let result = parse_prepare(&raw, intent)?;
        match result.decision {
            PrepareDecision::AllowPrepared => {
                let prepared_id = result.prepared_id.clone().ok_or_else(|| {
                    AuthorityError::Validate(
                        "prepare.malformed: allow without prepared_id".to_string(),
                    )
                })?;
                Ok(PrepareOutcome::AllowPrepared {
                    prepared_id,
                    reason: result.reason.clone(),
                    result,
                })
            }
            PrepareDecision::Ask => {
                let prepared_id = result.prepared_id.clone().ok_or_else(|| {
                    AuthorityError::Validate(
                        "prepare.malformed: ask without prepared_id".to_string(),
                    )
                })?;
                let approval = result.approval.clone().ok_or_else(|| {
                    AuthorityError::Validate("prepare.malformed: ask without approval".to_string())
                })?;
                // Bind the ask-time digest to Omen's intent NOW: a later
                // substitution cannot survive the commit-time check.
                crate::verify_argument_digest(
                    &approval.argument_digest,
                    &intent.expected_arguments,
                )?;
                if approval.action_id != intent.action_id {
                    return Err(AuthorityError::Validate(format!(
                        "prepare.approval_action_mismatch: {} != {}",
                        approval.action_id, intent.action_id
                    )));
                }
                Ok(PrepareOutcome::Ask {
                    prepared_id,
                    reason: result.reason.clone(),
                    approval,
                    result,
                })
            }
            PrepareDecision::Deny => Ok(PrepareOutcome::Deny {
                reason: result.reason,
            }),
            PrepareDecision::Unavailable => Ok(PrepareOutcome::Unavailable {
                reason: result.reason,
            }),
        }
    }

    /// Relay one exact human decision. Tethers owns the record; the
    /// result authorises nothing by itself.
    pub fn decide_approval(
        &mut self,
        approval_id: &str,
        decision: ApprovalDecision,
    ) -> Result<ApprovalOutcome, AuthorityError> {
        let raw = self.roundtrip_ok(
            "approval_decision",
            json!({"approval_id": approval_id, "decision": decision.as_str()}),
        )?;
        let obj = raw.as_object().ok_or_else(|| {
            AuthorityError::Validate("approval.parse.failed: not an object".to_string())
        })?;
        let get = |k: &str| {
            obj.get(k)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| {
                    AuthorityError::Validate(format!("approval.parse.failed: missing {k}"))
                })
        };
        Ok(ApprovalOutcome {
            approval_id: get("approval_id")?,
            state: get("state")?,
            action_id: obj
                .get("action_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
    }

    /// Fresh COMMIT under CURRENT truth. The Gate re-evaluates policy,
    /// scope, revocation, and replay at this moment — a stale or revoked
    /// preparation refuses here even after approval.
    pub fn commit(
        &mut self,
        prepared_id: &str,
        approval_id: Option<&str>,
    ) -> Result<CommitOutcome, AuthorityError> {
        let mut payload = json!({"prepared_id": prepared_id});
        if let Some(a) = approval_id {
            payload["approval_id"] = Value::String(a.to_string());
        }
        match self.transport.roundtrip("commit", payload) {
            Ok(raw) => {
                let dispatch = DispatchRecord::parse(&raw)?;
                let approval_consumed = raw
                    .get("approval_consumed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Ok(CommitOutcome::Admitted {
                    dispatch: Box::new(dispatch),
                    prepared_id: prepared_id.to_string(),
                    approval_consumed,
                })
            }
            Err(RoundtripError::Refused { code, message, .. }) => {
                Ok(CommitOutcome::Refused { code, message })
            }
            Err(RoundtripError::Transport(e)) => Err(e),
        }
    }

    /// Bounded durable-reconciliation read (reconnect/recovery surface).
    pub fn status(&mut self) -> Result<crate::protocol::DurableView, AuthorityError> {
        let raw = self.roundtrip_ok("status", json!({}))?;
        crate::protocol::DurableView::parse(&raw)
    }

    fn roundtrip_ok(&mut self, operation: &str, payload: Value) -> Result<Value, AuthorityError> {
        match self.transport.roundtrip(operation, payload) {
            Ok(v) => Ok(v),
            Err(RoundtripError::Refused { code, message, .. }) => Err(
                AuthorityError::CommitRefused(format!("{operation}.{code}: {message}")),
            ),
            Err(RoundtripError::Transport(e)) => Err(e),
        }
    }
}

fn parse_prepare(raw: &Value, intent: &AuthorityIntent) -> Result<PrepareResult, AuthorityError> {
    let obj = raw.as_object().ok_or_else(|| {
        AuthorityError::Validate("prepare.parse.failed: result not an object".to_string())
    })?;
    let get_str = |k: &str| {
        obj.get(k)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| AuthorityError::Validate(format!("prepare.parse.failed: missing {k}")))
    };
    let decision_s = get_str("decision")?;
    let decision = PrepareDecision::parse(&decision_s).ok_or_else(|| {
        AuthorityError::Validate(format!("prepare.decision_unknown: {decision_s}"))
    })?;
    // Contract invariants the Gate must uphold: prepare never authorises
    // dispatch and always plans through Core.
    if obj.get("authorizes_dispatch").and_then(|v| v.as_bool()) != Some(false) {
        return Err(AuthorityError::Validate(
            "prepare.contract_violated: authorizes_dispatch != false".to_string(),
        ));
    }
    if obj.get("core_planned").and_then(|v| v.as_bool()) != Some(true) {
        return Err(AuthorityError::Validate(
            "prepare.contract_violated: core_planned != true".to_string(),
        ));
    }
    // Echo binding: the preparation must be ABOUT our intent.
    let echo = |k: &str, want: &str| {
        let got = obj.get(k).and_then(|v| v.as_str()).unwrap_or("");
        if got != want {
            return Err(AuthorityError::Validate(format!(
                "prepare.echo_mismatch: {k} {got} != {want}"
            )));
        }
        Ok(())
    };
    echo("evaluation_id", &intent.evaluation_id)?;
    echo("action_id", &intent.action_id)?;
    let approval =
        match obj.get("approval") {
            Some(v) if !v.is_null() => Some(serde_json::from_value(v.clone()).map_err(|e| {
                AuthorityError::Validate(format!("prepare.approval_malformed: {e}"))
            })?),
            _ => None,
        };
    Ok(PrepareResult {
        prepared_id: obj
            .get("prepared_id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        decision,
        reason: get_str("reason")?,
        authorizes_dispatch: false,
        evaluation_id: intent.evaluation_id.clone(),
        action_id: intent.action_id.clone(),
        approval,
        raw: raw.clone(),
    })
}
