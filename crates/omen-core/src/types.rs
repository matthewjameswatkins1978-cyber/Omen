use serde::{Deserialize, Serialize};

/// Assurance/provenance classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Assurance {
    Deterministic,
    Verified,
    Enforced,
    Observed,
    Claimed,
    Inferred,
    Unknown,
}

/// Validity state of facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ValidityState {
    Current,
    Dirty,
    Stale,
    Superseded,
    Historical,
}

/// Containment enforcement level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EnforcementLevel {
    Enforced,
    Observed,
    Prevented,
    Unsupported,
}

/// Execution classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionClass {
    Deterministic,
    Tracked,
    Opaque,
}

/// Lease rights for execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseRights {
    Read,
    Write,
    Execute,
}

/// Standard I/O mode for execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StdioMode {
    Closed,
    Inline,
    Artifact,
    Interactive,
    Inherit,
}

/// CAS blob retention policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RetentionClass {
    Pinned,
    Referenced,
    Cache,
    Ephemeral,
}

/// CAS blob state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BlobState {
    Present,
    Evicted,
    Missing,
}
