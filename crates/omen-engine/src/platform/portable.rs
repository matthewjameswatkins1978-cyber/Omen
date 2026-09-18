pub struct PortableProcessGuard;

impl PortableProcessGuard {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PortableProcessGuard {
    fn default() -> Self {
        Self::new()
    }
}
