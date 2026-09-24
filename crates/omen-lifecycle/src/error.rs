//! Phase-named lifecycle errors. Every failure names where it stopped so an
//! agent never reconstructs the execution path from prose.
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LifecycleError {
    #[error("lifecycle.discover.failed: {0}")]
    Discover(String),
    #[error("lifecycle.resolve.failed: {0}")]
    Resolve(String),
    #[error("lifecycle.download.failed: {0}")]
    Download(String),
    #[error("lifecycle.verify.failed: {0}")]
    Verify(String),
    #[error("lifecycle.compat.failed: {0}")]
    Compat(String),
    #[error("lifecycle.snapshot.failed: {0}")]
    Snapshot(String),
    #[error("lifecycle.stage.failed: {0}")]
    Stage(String),
    #[error("lifecycle.migrate.failed: {0}")]
    Migrate(String),
    #[error("lifecycle.health.failed: {0}")]
    Health(String),
    #[error("lifecycle.activate.failed: {0}")]
    Activate(String),
    #[error("lifecycle.rollback.failed: {0}")]
    Rollback(String),
    #[error("lifecycle.plan.stale: {0}")]
    StalePlan(String),
    #[error("lifecycle.apply.refused: {0}")]
    Refused(String),
    #[error("lifecycle.lock.unavailable: {0}")]
    LockUnavailable(String),
    #[error("lifecycle.io.failed: {0}")]
    Io(String),
    #[error("lifecycle.manifest.invalid: {0}")]
    Manifest(String),
    #[error("lifecycle.ownership.unknown: {0}")]
    UnknownOwnership(String),
}

impl LifecycleError {
    pub fn phase(&self) -> &'static str {
        match self {
            Self::Discover(_) => "discover",
            Self::Resolve(_) => "resolve",
            Self::Download(_) => "download",
            Self::Verify(_) => "verify",
            Self::Compat(_) => "compat",
            Self::Snapshot(_) => "snapshot",
            Self::Stage(_) => "stage",
            Self::Migrate(_) => "migrate",
            Self::Health(_) => "health",
            Self::Activate(_) => "activate",
            Self::Rollback(_) => "rollback",
            Self::StalePlan(_) => "plan",
            Self::Refused(_) => "apply",
            Self::LockUnavailable(_) => "lock",
            Self::Io(_) => "io",
            Self::Manifest(_) => "manifest",
            Self::UnknownOwnership(_) => "ownership",
        }
    }
}
