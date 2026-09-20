//! Omen session knowledge model, SQLite Fact Registry, and CAS store.

pub mod cas;
pub mod db;
pub mod history;
pub mod registry;
pub mod workspace;

pub use cas::{ArtifactMetadata, ContentAddressedStore, GcReport};
pub use db::Database;
pub use history::{ExecutionHistory, ExecutionRecord, InteractiveSessionRecord};
pub use registry::{
    DependencyRecord, FactProvenance, FactRecord, FactRegistry, PublishFactRequest,
};
pub use workspace::{
    CANONICAL_DB_FILE_NAME, RequestReceiptRecord, ServiceRecord, WorkspacePersistence,
    WorkspaceRecord, canonical_workspace_db_path, canonical_workspace_db_path_readonly,
    deterministic_workspace_id, resolve_workspace_dir, workspace_state_dir_path,
};
