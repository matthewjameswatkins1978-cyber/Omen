//! Omen UI rendering, terminal abstraction, color roles, and semantic blocks.

pub mod appearance;
pub mod blocks;
pub mod color;
pub mod diagnostics;
pub mod humanize;
pub mod prompt;
pub mod terminal;

pub use appearance::{AppearanceChooser, ChooserAction, HumanSettings, PromptDensity, Theme};
pub use blocks::SemanticBlock;
pub use color::ColorRoles;
pub use diagnostics::{DiagnosticLevel, DiagnosticRenderer};
pub use prompt::{PromptRenderer, PromptState};
pub use terminal::TerminalCapabilities;
