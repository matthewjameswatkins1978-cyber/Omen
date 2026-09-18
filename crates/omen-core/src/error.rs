use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Machine-readable canonical error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidUri,
    InvalidId,
    SchemaViolation,
    UnsupportedSchemaVersion,
    NotFound,
    FactDirty,
    FactStale,
    AssuranceNotSatisfied,
    ExecutionFailed,
    SpawnFailed,
    TimedOut,
    Cancelled,
    ContainmentFailed,
    IoFailed,
    Refusal,
    Internal,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidUri => "INVALID_URI",
            Self::InvalidId => "INVALID_ID",
            Self::SchemaViolation => "SCHEMA_VIOLATION",
            Self::UnsupportedSchemaVersion => "UNSUPPORTED_SCHEMA_VERSION",
            Self::NotFound => "NOT_FOUND",
            Self::FactDirty => "FACT_DIRTY",
            Self::FactStale => "FACT_STALE",
            Self::AssuranceNotSatisfied => "ASSURANCE_NOT_SATISFIED",
            Self::ExecutionFailed => "EXECUTION_FAILED",
            Self::SpawnFailed => "SPAWN_FAILED",
            Self::TimedOut => "TIMED_OUT",
            Self::Cancelled => "CANCELLED",
            Self::ContainmentFailed => "CONTAINMENT_FAILED",
            Self::IoFailed => "IO_FAILED",
            Self::Refusal => "REFUSAL",
            Self::Internal => "INTERNAL",
        }
    }
}

/// Canonical core error representations.
#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoreError {
    #[error("Invalid resource URI: {0}")]
    InvalidUri(String),

    #[error("Invalid identifier: {0}")]
    InvalidId(String),

    #[error("Schema violation: {0}")]
    SchemaViolation(String),

    #[error("Unsupported schema version '{version}' (expected '{expected}')")]
    UnsupportedSchemaVersion { version: String, expected: String },

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Fact is dirty and explicit revalidation is required: {0}")]
    FactDirty(String),

    #[error("Fact is stale: {0}")]
    FactStale(String),

    #[error("Required assurance level '{required}' cannot be satisfied (available: '{available}')")]
    AssuranceNotSatisfied { required: String, available: String },

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl CoreError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidUri(_) => ErrorCode::InvalidUri,
            Self::InvalidId(_) => ErrorCode::InvalidId,
            Self::SchemaViolation(_) => ErrorCode::SchemaViolation,
            Self::UnsupportedSchemaVersion { .. } => ErrorCode::UnsupportedSchemaVersion,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::FactDirty(_) => ErrorCode::FactDirty,
            Self::FactStale(_) => ErrorCode::FactStale,
            Self::AssuranceNotSatisfied { .. } => ErrorCode::AssuranceNotSatisfied,
            Self::ExecutionFailed(_) => ErrorCode::ExecutionFailed,
            Self::Internal(_) => ErrorCode::Internal,
        }
    }
}
