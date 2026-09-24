//! Windows ConPTY / console-control ground-truth measurement (IDO No. 2 M0-W).
//!
//! Measurement only: this module never repairs production Omen behaviour.
//! Compat owns an independent ConPTY control harness; omen-engine's ConPTY is
//! the product under test (D2-020). Every wait, read, write, and cleanup is
//! bounded.
//!
//! D2-022: synchronous ConPTY I/O runs on dedicated workers. The caller never
//! performs potentially blocking `WriteFile` or `ClosePseudoConsole` directly.

mod harness;
mod judge;
mod observe;

pub use harness::*;
pub use judge::*;
pub use observe::*;
