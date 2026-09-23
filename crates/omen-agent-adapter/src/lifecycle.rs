//! Explicit adapter lifecycle. Conformance does not install,
//! installation does not activate, activation does not create authority.

use serde::{Deserialize, Serialize};

/// Canonical lifecycle states. The happy path is
/// Inspected -> Validated -> Conformed -> Installed -> Bound -> Active.
/// Side states preserve their distinctions; none is silently merged.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdapterLifecycle {
    Inspected,
    Validated,
    Conformed,
    Installed,
    Bound,
    Active,
    Stale,
    Incompatible,
    Degraded,
    Unavailable,
    Disabled,
    Quarantined,
}

/// Lifecycle events. Every transition is named; illegal ones fail visibly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEvent {
    ManifestValid,
    ManifestInvalid,
    ConformancePass,
    ConformanceFail,
    Install,
    Bind,
    Activate,
    Deactivate,
    MarkStale,
    Refresh,
    VersionMismatch,
    Degrade,
    Recover,
    Disable,
    Enable,
    Quarantine,
    Release,
    DependencyGone,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("illegal adapter lifecycle transition: {from:?} + {event:?}")]
pub struct LifecycleError {
    pub from: AdapterLifecycle,
    pub event: LifecycleEvent,
}

impl AdapterLifecycle {
    pub fn advance(self, event: LifecycleEvent) -> Result<AdapterLifecycle, LifecycleError> {
        use AdapterLifecycle as S;
        use LifecycleEvent as E;
        let next = match (self, event) {
            (S::Inspected, E::ManifestValid) => S::Validated,
            (S::Inspected, E::ManifestInvalid) => S::Incompatible,
            (S::Validated, E::ConformancePass) => S::Conformed,
            (S::Validated, E::ConformanceFail) => S::Quarantined,
            (S::Conformed, E::Install) => S::Installed,
            (S::Installed, E::Bind) => S::Bound,
            (S::Bound, E::Activate) => S::Active,
            (S::Active, E::Deactivate) => S::Bound,
            (S::Bound, E::Deactivate) => S::Installed,
            (S::Active, E::DependencyGone) => S::Unavailable,
            (S::Bound, E::DependencyGone) => S::Unavailable,
            (S::Installed, E::DependencyGone) => S::Unavailable,
            (S::Unavailable, E::Refresh) => S::Installed,
            (S::Active, E::Degrade) => S::Degraded,
            (S::Degraded, E::Recover) => S::Active,
            (S::Degraded, E::Deactivate) => S::Bound,
            (S::Installed, E::MarkStale)
            | (S::Bound, E::MarkStale)
            | (S::Active, E::MarkStale)
            | (S::Conformed, E::MarkStale)
            | (S::Validated, E::MarkStale) => S::Stale,
            (S::Stale, E::Refresh) => S::Inspected,
            (S::Inspected, E::VersionMismatch)
            | (S::Validated, E::VersionMismatch)
            | (S::Conformed, E::VersionMismatch) => S::Incompatible,
            (S::Quarantined, E::Release) => S::Inspected,
            (S::Incompatible, E::Refresh) => S::Inspected,
            (S::Inspected, E::Disable)
            | (S::Validated, E::Disable)
            | (S::Conformed, E::Disable)
            | (S::Installed, E::Disable)
            | (S::Bound, E::Disable)
            | (S::Active, E::Disable)
            | (S::Stale, E::Disable)
            | (S::Degraded, E::Disable)
            | (S::Unavailable, E::Disable) => S::Disabled,
            (S::Disabled, E::Enable) => S::Inspected,
            (S::Inspected, E::Quarantine)
            | (S::Validated, E::Quarantine)
            | (S::Conformed, E::Quarantine)
            | (S::Installed, E::Quarantine)
            | (S::Bound, E::Quarantine)
            | (S::Active, E::Quarantine) => S::Quarantined,
            _ => {
                return Err(LifecycleError { from: self, event });
            }
        };
        Ok(next)
    }

    /// Conformance evidence never counts as installation or activation.
    pub fn is_live(self) -> bool {
        matches!(self, Self::Bound | Self::Active | Self::Degraded)
    }
}
