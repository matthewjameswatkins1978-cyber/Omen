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

/// Containment enforcement level according to Omen 0.6 assurance standard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EnforcementLevel {
    /// The OS/runtime will deny the prohibited action independent of Omen's continued cooperation.
    Enforced,
    /// The action must pass through a controller/broker that can deny it, but the process is not completely confined against alternate paths.
    Mediated,
    /// Omen can detect/report the behaviour but cannot reliably prevent it.
    Observed,
    /// Omen attempts to constrain behaviour but known escape/gap classes exist.
    BestEffort,
    /// No meaningful mechanism exists in the active backend.
    Unsupported,
    /// Deprecated legacy outcome variant from 0.2-0.5; canonicalized to Enforced.
    #[serde(alias = "PREVENTED")]
    Prevented,
}

impl EnforcementLevel {
    /// Maps legacy levels to canonical 0.6 assurance levels.
    pub fn canonicalize(self) -> Self {
        match self {
            Self::Prevented => Self::Enforced,
            other => other,
        }
    }

    /// Converts enforcement level to an overall Assurance level.
    pub fn to_assurance(self) -> Assurance {
        match self {
            Self::Enforced | Self::Prevented => Assurance::Enforced,
            Self::Mediated => Assurance::Verified,
            Self::Observed | Self::BestEffort => Assurance::Observed,
            Self::Unsupported => Assurance::Unknown,
        }
    }
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

/// Lifecycle state of a daemon-managed PTY session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PtyState {
    Starting,
    Running,
    Detached,
    Exited,
    Lost,
    Unknown,
}

/// Lifecycle state of a physical process runtime lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RuntimeLeaseState {
    Owned,
    Detached,
    Expired,
    Observed,
    Lost,
}

/// Injection mechanism approved for physical secret usage.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretInjectionContract {
    /// Injected into environment variable.
    EnvironmentVariable { name: String },
    /// Injected into process stdin stream.
    Stdin,
    /// Written to a temporary file with restricted permissions and explicit bounded cleanup.
    TemporaryFile { file_name: Option<String> },
}
