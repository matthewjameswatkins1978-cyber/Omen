//! Omen UI rendering, terminal abstraction, color roles, and semantic blocks.

pub mod color;
pub mod diagnostics;
pub mod prompt;
pub mod terminal;

pub use color::ColorRoles;
pub use diagnostics::{DiagnosticLevel, DiagnosticRenderer};
pub use prompt::{PromptRenderer, PromptState};
pub use terminal::TerminalCapabilities;
