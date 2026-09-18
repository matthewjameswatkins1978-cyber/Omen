use omen_core::{Assurance, CoreError, EnforcementLevel, RequiredAssurance};

/// Capabilities discovered and supported by the platform execution backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendCapabilities {
    pub filesystem: EnforcementLevel,
    pub network: EnforcementLevel,
    pub descendants: EnforcementLevel,
    pub symlink_escape: EnforcementLevel,
}

impl BackendCapabilities {
    pub fn filesystem_assurance(&self) -> Assurance {
        match self.filesystem {
            EnforcementLevel::Enforced => Assurance::Enforced,
            _ => Assurance::Observed,
        }
    }

    pub fn network_assurance(&self) -> Assurance {
        match self.network {
            EnforcementLevel::Enforced => Assurance::Enforced,
            _ => Assurance::Observed,
        }
    }

    pub fn descendants_assurance(&self) -> Assurance {
        match self.descendants {
            EnforcementLevel::Enforced => Assurance::Enforced,
            _ => Assurance::Observed,
        }
    }
}

pub trait ExecutionBackend: Send + Sync {
    fn capabilities(&self) -> BackendCapabilities;

    fn preflight(&self, required: &RequiredAssurance) -> Result<(), CoreError> {
        let caps = self.capabilities();

        if required.filesystem == Some(Assurance::Enforced)
            && caps.filesystem != EnforcementLevel::Enforced
        {
            return Err(CoreError::AssuranceNotSatisfied {
                required: "Enforced".into(),
                available: format!("{:?}", caps.filesystem),
            });
        }

        if required.network == Some(Assurance::Enforced)
            && caps.network != EnforcementLevel::Enforced
        {
            return Err(CoreError::AssuranceNotSatisfied {
                required: "Enforced".into(),
                available: format!("{:?}", caps.network),
            });
        }

        if required.descendants == Some(Assurance::Enforced)
            && caps.descendants != EnforcementLevel::Enforced
        {
            return Err(CoreError::AssuranceNotSatisfied {
                required: "Enforced".into(),
                available: format!("{:?}", caps.descendants),
            });
        }

        Ok(())
    }
}

#[cfg(windows)]
pub struct WindowsExecutionBackend;

#[cfg(windows)]
impl ExecutionBackend for WindowsExecutionBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            filesystem: EnforcementLevel::Observed,
            network: EnforcementLevel::Observed,
            descendants: EnforcementLevel::Enforced, // Windows Job Objects guarantee descendant cleanup
            symlink_escape: EnforcementLevel::Prevented,
        }
    }
}

#[cfg(target_os = "linux")]
pub struct LinuxExecutionBackend {
    cgroups_available: bool,
}

#[cfg(target_os = "linux")]
impl LinuxExecutionBackend {
    pub fn new() -> Self {
        let cgroups_available = std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists();
        Self { cgroups_available }
    }
}

#[cfg(target_os = "linux")]
impl Default for LinuxExecutionBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
impl ExecutionBackend for LinuxExecutionBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            filesystem: EnforcementLevel::Observed,
            network: EnforcementLevel::Observed,
            descendants: if self.cgroups_available {
                EnforcementLevel::Enforced
            } else {
                EnforcementLevel::Observed
            },
            symlink_escape: EnforcementLevel::Prevented,
        }
    }
}

pub struct PortableExecutionBackend;

impl ExecutionBackend for PortableExecutionBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            filesystem: EnforcementLevel::Observed,
            network: EnforcementLevel::Observed,
            descendants: EnforcementLevel::Observed,
            symlink_escape: EnforcementLevel::Observed,
        }
    }
}

pub fn create_platform_backend() -> Box<dyn ExecutionBackend> {
    #[cfg(windows)]
    {
        Box::new(WindowsExecutionBackend)
    }

    #[cfg(target_os = "linux")]
    {
        Box::new(LinuxExecutionBackend::new())
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Box::new(PortableExecutionBackend)
    }
}
