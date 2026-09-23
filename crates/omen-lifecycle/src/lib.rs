//! Omen 0.9-H product lifecycle.
//!
//! Turns the proven runtime into a finished product on a real machine:
//! explicit local-state roles, conservative clean, retention GC with
//! plan/apply separation, install ownership, Stable/Preview channels,
//! transactional update with crash recovery, observational doctor, explicit
//! repair, redacted diagnostics, retention-aware uninstall, and degraded
//! terminal rendering. Convenience never erases truth: unknown disposal
//! status means KEEP, plan never equals apply, doctor never repairs.

pub mod archive;
pub mod clean;
pub mod diagnostics;
pub mod doctor;
pub mod error;
pub mod gc;
pub mod health;
pub mod install;
pub mod lock;
pub mod migrate;
pub mod pins;
pub mod plan;
pub mod release;
pub mod render;
pub mod repair;
pub mod state;
pub mod transport;
pub mod uninstall;
pub mod update;

pub use error::LifecycleError;
