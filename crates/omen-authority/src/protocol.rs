//! `tethers.authority/1` typed frames.
//!
//! Omen is a consumer of this frozen machine protocol: it matches `schema`
//! exactly and never infers protocol identity from the product version.
//! Only the operations Omen needs are modelled: `hello`, `prepare`,
//! `approval_decision`, `commit`, `outcome`, `status`, `shutdown`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Exact frozen protocol identity. Any other schema value refuses.
pub const AUTHORITY_PROTOCOL: &str = "tethers.authority/1";
/// Expected compatible Tethers product version (independent axis).
pub const TETHERS_PRODUCT_VERSION: &str = "0.8.0";
/// Dispatch record schema returned by a successful `commit`.
pub const DISPATCH_SCHEMA: &str = "tethers.dispatch/1";
/// Maximum accepted response frame size (mirrors the Gate bound).
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Outbound request frame.
#[derive(Debug, Clone, Serialize)]
pub struct RequestFrame {
    pub schema: String,
    pub request_id: String,
    pub operation: String,
    pub payload: Value,
}

impl RequestFrame {
    pub fn new(request_id: String, operation: &str, payload: Value) -> Self {
        Self {
            schema: AUTHORITY_PROTOCOL.to_string(),
            request_id,
            operation: operation.to_string(),
            payload,
        }
    }

    pub fn to_line(&self) -> Result<String, crate::AuthorityError> {
        let mut line = serde_json::to_string(self).map_err(|e| {
            crate::AuthorityError::Request(format!("request.serialize.failed: {e}"))
        })?;
        if line.len() > MAX_FRAME_BYTES {
            return Err(crate::AuthorityError::Request(
                "request.oversized: frame exceeds 1 MiB".to_string(),
            ));
        }
        line.push('\n');
        Ok(line)
    }
}

/// Inbound response frame (strict: exactly one of result/error).
#[derive(Debug, Clone, Deserialize)]
pub struct ResponseFrame {
    pub schema: String,
    pub request_id: String,
    pub status: String,
    pub result: Option<Value>,
    pub error: Option<GateErrorBody>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GateErrorBody {
    pub code: String,
    pub message: String,
    pub data: Option<Value>,
}

impl ResponseFrame {
    pub fn parse(line: &str) -> Result<Self, crate::AuthorityError> {
        if line.len() > MAX_FRAME_BYTES {
            return Err(crate::AuthorityError::Receive(
                "frame.oversized: response exceeds 1 MiB".to_string(),
            ));
        }
        let frame: ResponseFrame = serde_json::from_str(line)
            .map_err(|e| crate::AuthorityError::Receive(format!("frame.invalid_json: {e}")))?;
        if frame.schema != AUTHORITY_PROTOCOL {
            return Err(crate::AuthorityError::Receive(format!(
                "frame.unsupported_schema: {}",
                frame.schema
            )));
        }
        match (
            &frame.status[..],
            frame.result.is_some(),
            frame.error.is_some(),
        ) {
            ("ok", true, false) | ("error", false, true) => Ok(frame),
            ("ok", _, _) => Err(crate::AuthorityError::Receive(
                "frame.malformed: ok without exactly result".to_string(),
            )),
            ("error", _, _) => Err(crate::AuthorityError::Receive(
                "frame.malformed: error without exactly error".to_string(),
            )),
            _ => Err(crate::AuthorityError::Receive(format!(
                "frame.malformed: unknown status {}",
                frame.status
            ))),
        }
    }

    pub fn into_result(self) -> Result<Value, GateErrorBody> {
        match self.status.as_str() {
            "ok" => Ok(self.result.unwrap_or(Value::Null)),
            _ => Err(self.error.unwrap_or(GateErrorBody {
                code: "frame.malformed".to_string(),
                message: "error status without error body".to_string(),
                data: None,
            })),
        }
    }
}

/// Typed `hello` result.
#[derive(Debug, Clone, Deserialize)]
pub struct HelloResult {
    pub protocol: String,
    pub protocol_versions: Vec<String>,
    pub product_version: String,
    pub git_sha: Option<String>,
    pub features: Vec<String>,
    pub gate_instance_id: String,
    pub authority_granted: bool,
    pub provider_invocations: u64,
}

/// `prepare` decision vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrepareDecision {
    AllowPrepared,
    Ask,
    Deny,
    Unavailable,
}

impl PrepareDecision {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow_prepared" => Some(PrepareDecision::AllowPrepared),
            "ask" => Some(PrepareDecision::Ask),
            "deny" => Some(PrepareDecision::Deny),
            "unavailable" => Some(PrepareDecision::Unavailable),
            _ => None,
        }
    }
}

/// Typed `prepare` result (plus raw value for echo checks).
#[derive(Debug, Clone)]
pub struct PrepareResult {
    pub prepared_id: Option<String>,
    pub decision: PrepareDecision,
    pub reason: String,
    pub authorizes_dispatch: bool,
    pub evaluation_id: String,
    pub action_id: String,
    pub approval: Option<ApprovalInfo>,
    pub raw: Value,
}

/// Approval object attached to an `ask` prepare.
#[derive(Debug, Clone, Deserialize)]
pub struct ApprovalInfo {
    pub approval_id: String,
    pub action_id: String,
    pub capability: CapabilityRef,
    pub reason: String,
    pub argument_digest: String,
    pub effect_summary: String,
    pub state: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityRef {
    pub name: String,
    pub version: u32,
}

/// Typed `tethers.dispatch/1` commit result.
#[derive(Debug, Clone, Deserialize)]
pub struct CommitResult {
    pub schema: String,
    pub execution_id: String,
    pub evaluation_id: String,
    pub action_id: String,
    pub event_id: String,
    pub capability: CapabilityRef,
    pub argument_digest: String,
    pub manifest_digest: String,
    pub provider_identity: String,
    pub prepared_id: String,
    pub approval_consumed: bool,
    pub authorizes_physical_execution_by_tethers: bool,
    pub host_must_report_outcome: bool,
    pub provider_invocations: u64,
}

/// Dispatch record: the exact admitted action Omen may execute.
#[derive(Debug, Clone)]
pub struct DispatchRecord {
    pub commit: CommitResult,
}

impl DispatchRecord {
    pub fn parse(raw: &Value) -> Result<Self, crate::AuthorityError> {
        let commit: CommitResult = serde_json::from_value(raw.clone())
            .map_err(|e| crate::AuthorityError::Validate(format!("dispatch.parse.failed: {e}")))?;
        if commit.schema != DISPATCH_SCHEMA {
            return Err(crate::AuthorityError::Validate(format!(
                "dispatch.schema_mismatch: {}",
                commit.schema
            )));
        }
        Ok(Self { commit })
    }
}

/// OUTCOME classification vocabulary (Omen reports truthfully).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeClassification {
    Succeeded,
    Failed,
    Uncertain,
}

impl OutcomeClassification {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutcomeClassification::Succeeded => "succeeded",
            OutcomeClassification::Failed => "failed",
            OutcomeClassification::Uncertain => "uncertain",
        }
    }
}

/// Typed first-report `outcome` result.
#[derive(Debug, Clone)]
pub struct OutcomeResult {
    pub execution_id: String,
    pub status: String,
    pub idempotent: bool,
    pub replay_terminal: Option<String>,
    pub recovered: bool,
    pub raw: Value,
}

impl OutcomeResult {
    pub fn parse(raw: &Value) -> Result<Self, crate::AuthorityError> {
        let obj = raw.as_object().ok_or_else(|| {
            crate::AuthorityError::Validate(
                "outcome.parse.failed: result not an object".to_string(),
            )
        })?;
        let get = |k: &str| {
            obj.get(k)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| {
                    crate::AuthorityError::Validate(format!("outcome.parse.failed: missing {k}"))
                })
        };
        Ok(Self {
            execution_id: get("execution_id")?,
            status: get("status").unwrap_or_else(|_| "unknown".to_string()),
            idempotent: obj
                .get("idempotent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            replay_terminal: obj
                .get("replay_terminal")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            recovered: obj
                .get("recovered")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            raw: raw.clone(),
        })
    }
}

/// Bounded `status` durable-reconciliation view (fields Omen needs).
#[derive(Debug, Clone, Default)]
pub struct DurableView {
    pub gate_instance_id: String,
    pub healthy: bool,
    pub reconciliation_state: String,
    pub recovery_required: Vec<Value>,
    pub unresolved_commits: Vec<Value>,
    pub terminal_outcomes: Vec<Value>,
    pub raw: Value,
}

impl DurableView {
    pub fn parse(raw: &Value) -> Result<Self, crate::AuthorityError> {
        let obj = raw.as_object().ok_or_else(|| {
            crate::AuthorityError::Validate("status.parse.failed: result not an object".to_string())
        })?;
        let str_field = |k: &str| {
            obj.get(k)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_default()
        };
        let arr_field = |k: &str| {
            obj.get(k)
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default()
        };
        let reconciliation_state = obj
            .get("durable_reconciliation")
            .and_then(|v| v.as_object())
            .and_then(|o| o.get("state"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default();
        Ok(Self {
            gate_instance_id: str_field("gate_instance_id"),
            healthy: obj
                .get("healthy")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            reconciliation_state,
            recovery_required: arr_field("recovery_required"),
            unresolved_commits: arr_field("unresolved_commits"),
            terminal_outcomes: arr_field("terminal_outcomes"),
            raw: raw.clone(),
        })
    }

    /// Fail-closed: any durable disagreement forbids fresh physical
    /// execution until an operator resolves it.
    pub fn recovery_blocks_execution(&self) -> bool {
        !self.recovery_required.is_empty()
    }

    pub fn terminal_known(&self, execution_id: &str) -> bool {
        self.terminal_outcomes
            .iter()
            .any(|v| v.get("execution_id").and_then(|id| id.as_str()) == Some(execution_id))
    }
}
