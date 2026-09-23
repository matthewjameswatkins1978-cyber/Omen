//! IDO No. 2 — Omen Compat.
//!
//! Portable measurement machinery for compatibility ground truth.
//! Observations record what happened; invariants state what must be true;
//! results judge whether observations satisfy invariants.
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
