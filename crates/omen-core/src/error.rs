use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Canonical core error codes and representations.
#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoreError {
    #[error("Invalid resource URI: {0}")]
    InvalidUri(String),

    #[error("Invalid identifier: {0}")]
    InvalidId(String),

    #[error("Schema violation: {0}")]
    SchemaViolation(String),

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Fact is dirty and explicit revalidation is required: {0}")]
    FactDirty(String),

    #[error(
        "Required assurance level '{required:?}' cannot be satisfied (available: '{available:?}')"
    )]
    AssuranceNotSatisfied { required: String, available: String },

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Internal error: {0}")]
    Internal(String),
}
