//! POSIX PTY / job-control ground-truth measurement (IDO No. 2 M0-D/G).
//!
//! Measurement only: this module never repairs production Omen behaviour.
//! Every wait is bounded; PTY reads are hostile I/O with explicit deadlines.

mod harness;
mod judge;
mod observe;

#[cfg(target_os = "linux")]
mod linux_proc;

pub use harness::*;
pub use judge::*;
pub use observe::*;

#[cfg(target_os = "linux")]
pub use linux_proc::*;
