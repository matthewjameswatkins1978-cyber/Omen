//! Omen Core domain types and identity primitives.

pub mod composition;
pub mod error;
pub mod id;
pub mod machine_contract;
pub mod model;
pub mod resource;
pub mod types;

pub use error::{
    CoreError, ERROR_SCHEMA_VERSION, ErrorCategory, ErrorCode, ErrorEvidence, OmenError,
    Retryability,
};
pub use id::{
    ActionId, ArtifactId, BackendId, ExecutionId, FactId, InteractiveSessionId, OperationId,
    ProcessId, PtySessionId, ResourceId, RuntimeLeaseId, SemanticProviderId, SymbolId, ToolId,
};
pub use model::{
    AdapterClassification, EnforcementReport, ExecutionConstraints, ExecutionContract,
    ExecutionResult, Intent, LeaseRequest, ProcessExit, RequiredAssurance, RuntimeStatus,
    SecretHandle, StdioConfig,
};
pub use resource::{ResourceKind, ResourceUri};
pub use types::{
    Assurance, BlobState, EnforcementLevel, ExecutionClass, LeaseRights, PtyState, RetentionClass,
    RuntimeLeaseState, SecretInjectionContract, StdioMode, ValidityState,
};
