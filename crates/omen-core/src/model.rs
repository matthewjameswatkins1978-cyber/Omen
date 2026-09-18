use crate::id::{ActionId, ExecutionId, ResourceId};
use crate::resource::ResourceUri;
use crate::types::{Assurance, EnforcementLevel, LeaseRights, StdioMode};
use serde::{Deserialize, Serialize};

/// Domain execution intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intent {
    pub tool: ResourceUri,
    pub operation: String,
    pub args: Vec<String>,
}

/// Domain lease request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRequest {
    pub resource: ResourceUri,
    pub rights: Vec<LeaseRights>,
    pub native_access: bool,
}

/// Domain stdio configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StdioConfig {
    pub stdin: StdioMode,
    pub stdout: StdioMode,
    pub stderr: StdioMode,
}

impl Default for StdioConfig {
    fn default() -> Self {
        Self {
            stdin: StdioMode::Closed,
            stdout: StdioMode::Inline,
            stderr: StdioMode::Inline,
        }
    }
}

/// Domain execution constraints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionConstraints {
    pub timeout_ms: u64,
    pub network_denied: bool,
}

/// Required assurance levels per subsystem.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RequiredAssurance {
    pub filesystem: Option<Assurance>,
    pub network: Option<Assurance>,
    pub descendants: Option<Assurance>,
}

/// Authoritative domain Execution Contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionContract {
    pub execution_id: ExecutionId,
    pub actor: ResourceUri,
    pub intent: Intent,
    pub leases: Vec<LeaseRequest>,
    pub stdio: StdioConfig,
    pub constraints: ExecutionConstraints,
    pub required_assurance: RequiredAssurance,
}

/// Execution runtime status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RuntimeStatus {
    Completed,
    SpawnFailed,
    TimedOut,
    Cancelled,
    ContainmentFailed,
    IoFailed,
}

/// Process exit representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessExit {
    pub code: Option<i32>,
    pub signal: Option<String>,
}

impl ProcessExit {
    pub fn success(code: i32) -> Self {
        Self {
            code: Some(code),
            signal: None,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.code == Some(0)
    }
}

/// Adapter-level semantic classification of outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AdapterClassification {
    Success,
    Failure,
    Refusal,
    Unknown,
}

/// Achieved containment and assurance levels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnforcementReport {
    pub filesystem: EnforcementLevel,
    pub network: EnforcementLevel,
    pub descendant_processes: EnforcementLevel,
    pub symlink_escape: EnforcementLevel,
}

/// Authoritative physical Execution Result reported by Omen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub execution_id: ExecutionId,
    pub action_id: ActionId,
    pub runtime_status: RuntimeStatus,
    pub process_exit: ProcessExit,
    pub adapter_classification: AdapterClassification,
    pub enforcement: EnforcementReport,
    pub observations: Vec<String>,
    pub fact_updates: Vec<ResourceId>,
    pub artifacts: Vec<ResourceUri>,
    pub reduced_summary: String,
}
