//! IDO No. 2 — Omen Compat.
//!
//! Portable measurement machinery for compatibility ground truth.
//!
//! ## Bounded I/O contract
//!
//! Every external-process wait, stdin delivery, stdout/stderr drain, and
//! cleanup phase is bounded by an explicit scenario deadline, non-waiting
//! root kill initiation + bounded reap, and **one shared** post-root I/O
//! completion window for stdin/stdout/stderr. `BOUNDED_WAIT_NO_HANG` checks
//! actual elapsed wall-clock time against [`runner::declared_max_wall_ms`].
//!
//! ## Secret-safe durable evidence
//!
//! Execution may transiently hold environment values and stdin bytes.
//! Durable structures ([`StructuredFailure`], [`ReplayDescriptor`],
//! [`CommandEvidence`], [`Observation`]) retain structure and truth without
//! automatically retaining those values. Stream summaries never include raw
//! textual previews by default.
//!
//! Production Omen crates must never depend on this crate.

pub mod core;
pub mod failure;
pub mod invariant;
pub mod observation;
pub mod runner;

pub use core::*;
pub use failure::*;
pub use invariant::*;
pub use observation::*;
pub use runner::*;
