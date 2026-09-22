//! Omen Core domain types and identity primitives.

pub mod authority;
pub mod composition;
pub mod error;
pub mod id;
pub mod machine_contract;
pub mod model;
pub mod resource;
pub mod resource_identity;
pub mod types;

pub use authority::{
    AdmissionRequest, AdmissionVerdict, AuthorityEvidenceReference, CapabilityIdentity,
    LiveAdmissionStatus, NotAdmittedReason, ScopeIdentity, check_live_admission,
    live_admission_status,
};
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
pub use resource_identity::{
    CanonicalResource, NormalizedResource, PresentedResource, ResourceAuthority,
};
pub use types::{
    Assurance, BlobState, EnforcementLevel, ExecutionClass, LeaseRights, PtyState, RetentionClass,
    RuntimeLeaseState, SecretInjectionContract, StdioMode, ValidityState,
};
