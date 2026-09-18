use omen_core::Assurance;

/// Capabilities discovered by the platform execution backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendCapabilities {
    pub filesystem_containment: Assurance,
    pub network_containment: Assurance,
    pub descendant_containment: Assurance,
}

/// Trait implemented by platform-specific backends.
pub trait ExecutionBackend: Send + Sync {
    fn capabilities(&self) -> BackendCapabilities;
}
