pub struct MacOsProcessGuard;

impl MacOsProcessGuard {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MacOsProcessGuard {
    fn default() -> Self {
        Self::new()
    }
}
