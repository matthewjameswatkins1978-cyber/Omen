pub struct LinuxProcessGuard {
    pub pgid: Option<i32>,
    pub cgroup_supported: bool,
}

impl LinuxProcessGuard {
    pub fn new() -> Self {
        // Detect cgroups v2 availability
        let cgroup_supported = std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists();
        Self {
            pgid: None,
            cgroup_supported,
        }
    }
}

impl Default for LinuxProcessGuard {
    fn default() -> Self {
        Self::new()
    }
}
