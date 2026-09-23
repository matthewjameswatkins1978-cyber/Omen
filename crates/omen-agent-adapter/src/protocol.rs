//! Wire protocol `omen.agent-adapter/0.1`.
//!
//! Every frame is one UTF-8 JSON object per line on the child stdout
//! (adapter -> Omen) or child stdin (Omen -> adapter). Human logs go to
//! stderr only and are never parsed. Unknown fields are preserved by
//! parsers but have zero semantic effect.

use serde::{Deserialize, Serialize};

/// Canonical adapter protocol schema name (distinct axis from Machine Contract).
pub const ADAPTER_PROTOCOL_SCHEMA: &str = "omen.agent-adapter";
/// Current adapter protocol version.
pub const ADAPTER_PROTOCOL_VERSION: &str = "0.1";
/// Adapter protocol versions Omen speaks, most preferred first.
pub const OMEN_SUPPORTED_ADAPTER_PROTOCOLS: &[&str] = &[ADAPTER_PROTOCOL_VERSION];

/// Maximum accepted size of a single NDJSON frame (matches local IPC cap).
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
/// Maximum accepted size of a single text field inside a frame.
pub const MAX_TEXT_FIELD_BYTES: usize = 256 * 1024;
/// Maximum number of proposed actions / tool calls admitted per response.
pub const MAX_ACTIONS_PER_RESPONSE: usize = 32;
/// Maximum number of unknown metadata entries preserved per frame.
pub const MAX_UNKNOWN_FIELDS: usize = 32;

/// Closed vocabulary of adapter-side error codes. Anything else arriving
/// on the wire maps to `internal` with bounded detail; Codex/vendor
/// strings never become Omen semantics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdapterErrorCode {
    AuthRequired,
    Unavailable,
    RateLimited,
    Timeout,
    UnsupportedCapability,
    Malformed,
    Incompatible,
    TransportFailure,
    Internal,
}

impl AdapterErrorCode {
    pub fn parse(s: &str) -> (Self, bool) {
        match s {
            "auth_required" => (Self::AuthRequired, true),
            "unavailable" => (Self::Unavailable, true),
            "rate_limited" => (Self::RateLimited, true),
            "timeout" => (Self::Timeout, true),
            "unsupported_capability" => (Self::UnsupportedCapability, true),
            "malformed" => (Self::Malformed, true),
            "incompatible" => (Self::Incompatible, true),
            "transport_failure" => (Self::TransportFailure, true),
            "internal" => (Self::Internal, true),
            _ => (Self::Internal, false),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuthRequired => "auth_required",
            Self::Unavailable => "unavailable",
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::Malformed => "malformed",
            Self::Incompatible => "incompatible",
            Self::TransportFailure => "transport_failure",
            Self::Internal => "internal",
        }
    }
}

/// Omen -> adapter frame types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OmenFrame {
    Hello { id: u64, payload: OmenHello },
    Reason { id: u64, payload: ReasonRequest },
    ToolResult { id: u64, payload: ToolResult },
    Cancel { id: u64, payload: CancelRequest },
    Shutdown { id: u64, payload: ShutdownRequest },
}

/// Adapter -> Omen frame types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdapterFrame {
    Hello {
        id: u64,
        payload: AdapterHello,
    },
    Response {
        id: u64,
        payload: AdapterResponse,
    },
    ToolRequest {
        id: u64,
        payload: AdapterToolRequest,
    },
    Error {
        id: u64,
        payload: AdapterErrorFrame,
    },
}

/// Omen handshake offer: contract identity, protocol range, feature needs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OmenHello {
    pub protocol: String,
    pub omen_contract: String,
    pub adapter_protocols: Vec<String>,
    pub required_features: Vec<String>,
    pub optional_features: Vec<String>,
}

/// Adapter handshake answer: identity, selection, capabilities.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterHello {
    pub protocol: String,
    pub adapter_id: String,
    pub adapter_version: String,
    pub protocol_version: String,
    pub capabilities: Vec<String>,
    pub features: Vec<String>,
    pub contract_versions: Vec<String>,
    pub credential_labels: Vec<String>,
    /// Unknown adapter metadata: preserved, bounded, semantically inert.
    #[serde(default, flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Bounded reasoning request. Context is dependency-complete and small;
/// detail is acquired progressively via tool requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReasonRequest {
    pub request_id: String,
    pub prompt: String,
    pub orientation: Orientation,
    pub tool_allowlist: Vec<String>,
}

/// Small bootstrap orientation: never the whole contract, schemas, or history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Orientation {
    pub omen_contract: String,
    pub adapter_protocol: String,
    pub capabilities: Vec<String>,
    pub workspace_root: String,
    pub cwd: String,
}

/// Adapter tool call. The tool name must be allowlisted by Omen; anything
/// else is refused, never reinterpreted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterToolRequest {
    pub request_id: String,
    pub call_id: String,
    pub tool: String,
    pub input: serde_json::Value,
}

/// Omen answer to a tool call. `ok == false` means Omen refused or the
/// tool failed; the adapter must still produce a final response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolResult {
    pub request_id: String,
    pub call_id: String,
    pub ok: bool,
    pub result: serde_json::Value,
}

/// Adapter final response. Shape mirrors `AgentResponse`; Omen validates
/// every action before admission.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterResponse {
    pub request_id: String,
    pub kind: String,
    pub message: String,
    pub proposed_actions: Vec<serde_json::Value>,
    pub references: Vec<String>,
    pub uncertainty: Option<String>,
    #[serde(default, flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterErrorFrame {
    pub request_id: Option<String>,
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CancelRequest {
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShutdownRequest {}

/// Serializes one frame with envelope checks (protocol tag + size bound).
pub fn encode_frame<T: Serialize>(frame: &T) -> Result<String, ProtocolError> {
    let mut value =
        serde_json::to_value(frame).map_err(|e| ProtocolError::Encode(e.to_string()))?;
    value
        .as_object_mut()
        .ok_or_else(|| ProtocolError::Encode("frame is not an object".into()))?
        .insert(
            "protocol".into(),
            format!("{ADAPTER_PROTOCOL_SCHEMA}/{ADAPTER_PROTOCOL_VERSION}").into(),
        );
    let mut line =
        serde_json::to_string(&value).map_err(|e| ProtocolError::Encode(e.to_string()))?;
    if line.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge {
            bytes: line.len(),
            max: MAX_FRAME_BYTES,
        });
    }
    line.push('\n');
    Ok(line)
}

/// Parses one adapter frame with size, UTF-8, protocol-tag, and
/// unknown-field bound checks. Unknown fields never error and never act.
pub fn decode_adapter_frame(line: &str) -> Result<AdapterFrame, ProtocolError> {
    if line.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge {
            bytes: line.len(),
            max: MAX_FRAME_BYTES,
        });
    }
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
    let protocol = value.get("protocol").and_then(|v| v.as_str()).unwrap_or("");
    let expected = format!("{ADAPTER_PROTOCOL_SCHEMA}/{ADAPTER_PROTOCOL_VERSION}");
    if protocol != expected {
        return Err(ProtocolError::WrongProtocol {
            got: protocol.to_string(),
            expected,
        });
    }
    let frame: AdapterFrame =
        serde_json::from_value(value).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
    frame.check_bounds()?;
    Ok(frame)
}

/// Parses one Omen frame (used by adapters and tests).
pub fn decode_omen_frame(line: &str) -> Result<OmenFrame, ProtocolError> {
    if line.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge {
            bytes: line.len(),
            max: MAX_FRAME_BYTES,
        });
    }
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
    let protocol = value.get("protocol").and_then(|v| v.as_str()).unwrap_or("");
    let expected = format!("{ADAPTER_PROTOCOL_SCHEMA}/{ADAPTER_PROTOCOL_VERSION}");
    if protocol != expected {
        return Err(ProtocolError::WrongProtocol {
            got: protocol.to_string(),
            expected,
        });
    }
    let frame: OmenFrame =
        serde_json::from_value(value).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
    frame.check_bounds()?;
    Ok(frame)
}

fn check_text_len(field: &str, value: &str) -> Result<(), ProtocolError> {
    if value.len() > MAX_TEXT_FIELD_BYTES {
        return Err(ProtocolError::FieldTooLarge {
            field: field.to_string(),
            bytes: value.len(),
            max: MAX_TEXT_FIELD_BYTES,
        });
    }
    Ok(())
}

fn check_unknown(
    map: &std::collections::BTreeMap<String, serde_json::Value>,
) -> Result<(), ProtocolError> {
    if map.len() > MAX_UNKNOWN_FIELDS {
        return Err(ProtocolError::TooManyUnknownFields { count: map.len() });
    }
    Ok(())
}

impl AdapterFrame {
    fn check_bounds(&self) -> Result<(), ProtocolError> {
        match self {
            AdapterFrame::Hello { payload, .. } => {
                check_text_len("adapter_id", &payload.adapter_id)?;
                check_text_len("adapter_version", &payload.adapter_version)?;
                check_unknown(&payload.extra)?;
            }
            AdapterFrame::Response { payload, .. } => {
                check_text_len("message", &payload.message)?;
                if payload.proposed_actions.len() > MAX_ACTIONS_PER_RESPONSE {
                    return Err(ProtocolError::TooManyActions {
                        count: payload.proposed_actions.len(),
                    });
                }
                check_unknown(&payload.extra)?;
            }
            AdapterFrame::ToolRequest { payload, .. } => {
                check_text_len("tool", &payload.tool)?;
            }
            AdapterFrame::Error { payload, .. } => {
                check_text_len("message", &payload.message)?;
            }
        }
        Ok(())
    }
}

impl OmenFrame {
    fn check_bounds(&self) -> Result<(), ProtocolError> {
        match self {
            OmenFrame::Reason { payload, .. } => {
                check_text_len("prompt", &payload.prompt)?;
            }
            OmenFrame::ToolResult { .. }
            | OmenFrame::Hello { .. }
            | OmenFrame::Cancel { .. }
            | OmenFrame::Shutdown { .. } => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("frame too large: {bytes} bytes (max {max})")]
    FrameTooLarge { bytes: usize, max: usize },
    #[error("field '{field}' too large: {bytes} bytes (max {max})")]
    FieldTooLarge {
        field: String,
        bytes: usize,
        max: usize,
    },
    #[error("too many proposed actions: {count}")]
    TooManyActions { count: usize },
    #[error("too many unknown metadata fields: {count}")]
    TooManyUnknownFields { count: usize },
    #[error("malformed frame: {0}")]
    Malformed(String),
    #[error("wrong protocol: got '{got}', expected '{expected}'")]
    WrongProtocol { got: String, expected: String },
    #[error("encode failed: {0}")]
    Encode(String),
}
