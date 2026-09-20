//! Omen Core domain types and identity primitives.

pub mod error;
pub mod id;
pub mod model;
pub mod resource;
pub mod types;

pub use error::{CoreError, ErrorCode};
pub use id::{
    ActionId, ArtifactId, BackendId, ExecutionId, FactId, InteractiveSessionId, OperationId,
    ProcessId, PtySessionId, ResourceId, RuntimeLeaseId, ToolId,
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
