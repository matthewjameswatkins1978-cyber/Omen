//! Injected Omen command/action knowledge.
//!
//! `omen-discovery` must not depend on `omen-interactive`, so it does not
//! import `commands::OMEN_ACTIONS` directly. Instead the interactive layer
//! injects the canonical authority here at construction time. This keeps a
//! single source of truth (no second handwritten action list) while preserving
//! the dependency direction.

/// Canonical Omen command grammar, injected by the owning layer.
pub struct OmenKnowledge {
    /// Canonical `:action` names (from `commands::OMEN_ACTIONS`).
    pub actions: &'static [&'static str],
    /// First-argument subcommands for multi-arity actions.
    pub action_subcommands: fn(&str) -> &'static [&'static str],
    /// Shell intrinsics handled by the session (`cd`, `exit`, `quit`).
    pub shell_intrinsics: &'static [&'static str],
    /// Static subcommand syntax for well-known external tools.
    pub tool_subcommands: fn(&str) -> &'static [&'static str],
    /// Bare Windows drive designator detection (`D:`).
    pub is_drive_designator: fn(&str) -> Option<char>,
}

impl OmenKnowledge {
    /// Empty knowledge for tests and standalone use.
    pub fn empty() -> Self {
        Self {
            actions: &[],
            action_subcommands: |_| &[],
            shell_intrinsics: &[],
            tool_subcommands: |_| &[],
            is_drive_designator: |_| None,
        }
    }
}
