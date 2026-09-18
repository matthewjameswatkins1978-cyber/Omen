//! Omen interactive shell core, Reedline boundary, input parsing, and completion.

pub mod child;
pub mod prompt_adapter;
pub mod session;

pub use child::ChildHandoff;
pub use prompt_adapter::OmenPrompt;
pub use session::InteractiveSession;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
