use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum LocalIpcError {
    #[error("Protocol version unsupported: {0}")]
    ProtocolVersionUnsupported(String),

    #[error("Frame too large: {declared_bytes} exceeds limit of {max_bytes} bytes")]
    FrameTooLarge {
        declared_bytes: usize,
        max_bytes: usize,
    },

    #[error("Malformed request: {0}")]
    MalformedRequest(String),

    #[error("Local peer denied: {0}")]
    LocalPeerDenied(String),

    #[error("Workspace not attached: {0}")]
    WorkspaceNotAttached(String),

    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("Resync required: {0}")]
    ResyncRequired(String),

    #[error("Daemon degraded: {0}")]
    DaemonDegraded(String),

    #[error("Service not found: {0}")]
    ServiceNotFound(String),

    #[error("Service already running: {0}")]
    ServiceAlreadyRunning(String),

    #[error("Execution status unknown for request {0}; automatic retry refused")]
    ExecutionStatusUnknown(String),

    #[error("Duplicate request: {0}")]
    RequestDuplicate(String),

    #[error("Internal runtime error: {0}")]
    InternalRuntimeError(String),

    #[error("IO error: {0}")]
    Io(String),
}

impl From<std::io::Error> for LocalIpcError {
    fn from(err: std::io::Error) -> Self {
        LocalIpcError::Io(err.to_string())
    }
}
