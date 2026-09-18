//! Omen physical process execution engine.

pub mod backend;
pub mod platform;
pub mod supervisor;

pub use backend::{BackendCapabilities, ExecutionBackend, create_platform_backend};
pub use supervisor::{DEFAULT_INLINE_BUDGET, ExecutionOutput, ExecutionRequest, ProcessSupervisor};
