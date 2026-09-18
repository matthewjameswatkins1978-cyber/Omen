//! Omen interactive shell core, Reedline boundary, input parsing, and completion.

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
