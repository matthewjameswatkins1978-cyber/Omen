//! Windows ConPTY / console-control ground-truth measurement (IDO No. 2 M0-W).
//!
//! Measurement only: this module never repairs production Omen behaviour.
//! Compat owns an independent ConPTY control harness; omen-engine's ConPTY is
//! the product under test (D2-020). Every wait, read, write, and cleanup is
//! bounded.

mod harness;
mod judge;
mod observe;

pub use harness::*;
pub use judge::*;
pub use observe::*;
