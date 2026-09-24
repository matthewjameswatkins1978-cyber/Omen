//! Gate transport abstraction: one physical owner per session.
//!
//! The production transport is a persistent local Gate child over stdio
//! NDJSON ([`crate::gate::GateProcess`]). Tests use scripted fakes behind
//! the same trait — the admission seam cannot tell them apart, which is
//! what makes the zero-spawn matrix deterministic.

use crate::AuthorityError;
use crate::protocol::ResponseFrame;
use serde_json::Value;

/// One request/response exchange with a Tethers Gate.
pub trait GateTransport: Send {
    /// Human-readable identity of the far end (binary path, `fake:<name>`).
    /// Never trusted for authority — only for diagnostics.
    fn label(&self) -> String;
    /// Send one operation frame and return the parsed `result` value, or
    /// the Gate error body on `status: error`.
    fn roundtrip(&mut self, operation: &str, payload: Value) -> Result<Value, RoundtripError>;
    /// Best-effort orderly session end. Failure is diagnostic only.
    fn shutdown(&mut self);
}

/// A transport-level exchange outcome: either a Gate result or a Gate
/// refusal (both are *understood* frames), as distinct from transport
/// failure (timeout, malformed, crash).
#[derive(Debug, Clone)]
pub enum RoundtripError {
    /// Gate answered `status: error` with a machine code.
    Refused {
        code: String,
        message: String,
        data: Option<Value>,
    },
    /// Transport failure: timeout, malformed frame, crash, oversize.
    /// Always fails closed.
    Transport(AuthorityError),
}

impl RoundtripError {
    pub fn refused_code(&self) -> Option<&str> {
        match self {
            RoundtripError::Refused { code, .. } => Some(code),
            RoundtripError::Transport(_) => None,
        }
    }
}

impl From<RoundtripError> for AuthorityError {
    fn from(e: RoundtripError) -> Self {
        match e {
            RoundtripError::Refused { code, message, .. } => {
                AuthorityError::CommitRefused(format!("{code}: {message}"))
            }
            RoundtripError::Transport(e) => e,
        }
    }
}

/// Parse one raw response line into result-or-refusal.
pub fn interpret(line: &str) -> Result<Value, RoundtripError> {
    let frame = ResponseFrame::parse(line).map_err(RoundtripError::Transport)?;
    frame.into_result().map_err(|e| RoundtripError::Refused {
        code: e.code,
        message: e.message,
        data: e.data,
    })
}
