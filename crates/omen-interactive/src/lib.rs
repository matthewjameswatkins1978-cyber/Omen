//! Omen interactive shell core, Reedline boundary, input parsing, and completion.

pub mod actions;
pub mod child;
pub mod completion;
pub mod grammar;
pub mod prompt_adapter;
pub mod session;

pub use actions::SemanticDispatcher;
pub use child::ChildHandoff;
pub use completion::{CompletionContext, OmenCompleter, OmenHinter};
pub use grammar::{GrammarScanner, InputLane, TypedReference};
pub use prompt_adapter::OmenPrompt;
pub use session::InteractiveSession;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
