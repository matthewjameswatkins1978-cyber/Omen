//! Omen interactive shell core, Reedline boundary, input parsing, and completion.

pub mod actions;
pub mod ai_lane;
pub mod child;
pub mod commands;
pub mod completion;
pub mod grammar;
pub mod preflight;
pub mod prompt_adapter;
pub mod resolver;
pub mod semantic_service;
pub mod services;
pub mod session;

pub use actions::SemanticDispatcher;
pub use ai_lane::{AiLaneDispatchStats, AiLaneDispatcher, AiLaneOutput};
pub use child::ChildHandoff;
pub use commands::{OMEN_ACTIONS, SHELL_INTRINSICS};
pub use completion::{
    CandidateKind, CandidateSource, CompletionCandidate, CompletionContext, CompletionEdit,
    CompletionEngine, HotSemanticIndex, OmenCompleter, OmenHinter,
};
pub use grammar::{GrammarScanner, InputLane, ScannedWord, TypedReference};
pub use preflight::{BlastPreflight, BlastRadiusReport, BlastSeverity, PasteGuard, PasteReview};
pub use prompt_adapter::OmenPrompt;
pub use resolver::ReferenceResolver;
pub use services::{ManagedService, ServiceRegistry, ServiceState};
pub use session::InteractiveSession;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
