//! Omen Core domain types and identity primitives.

pub mod error;
pub mod id;
pub mod resource;
pub mod types;

pub use error::CoreError;
pub use id::{ActionId, ArtifactId, ExecutionId, FactId, ProcessId, ResourceId, ToolId};
pub use resource::ResourceUri;
pub use types::{
    Assurance, BlobState, EnforcementLevel, ExecutionClass, LeaseRights, RetentionClass, StdioMode,
    ValidityState,
};
