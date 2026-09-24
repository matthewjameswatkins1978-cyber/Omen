//! Typed candidate vocabulary.
//!
//! Kind is typed, not free text. A [`Kind`] participates in
//! [`crate::identity::SemanticKey`] and therefore in semantic merge: two
//! candidates with identical text but different kinds are different semantic
//! objects and must not merge.

use serde::{Deserialize, Serialize};

/// What class of discoverable thing a candidate is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Kind {
    /// Executable available on PATH.
    Command,
    /// Shell intrinsic handled by the interactive session.
    Intrinsic,
    /// Subcommand of an external tool's grammar.
    Subcommand,
    /// Command-line option / flag.
    Option,
    /// Value for an option or positional argument.
    ArgumentValue,
    /// Filesystem directory.
    Directory,
    /// Filesystem file.
    File,
    /// Canonical Omen semantic action.
    OmenAction,
    /// Workspace target from composition config.
    Workspace,
    /// Managed service.
    Service,
    /// Capability exposed by Omen.
    Capability,
    /// Typed reference (`@last`, `@fact.x`, ...).
    Reference,
    /// Semantic resource (fact, symbol, package, task).
    Resource,
    /// History-derived item (reserved; M1 carries the variant but no provider emits it).
    HistoryItem,
}

impl Kind {
    /// Whether this kind is a filesystem path the editor may navigate to.
    pub fn is_path_like(self) -> bool {
        matches!(self, Kind::Directory | Kind::File)
    }

    /// Stable short label used in descriptions and telemetry.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Command => "command",
            Kind::Intrinsic => "intrinsic",
            Kind::Subcommand => "subcommand",
            Kind::Option => "option",
            Kind::ArgumentValue => "argument value",
            Kind::Directory => "directory",
            Kind::File => "file",
            Kind::OmenAction => "semantic action",
            Kind::Workspace => "workspace target",
            Kind::Service => "service",
            Kind::Capability => "capability",
            Kind::Reference => "typed reference",
            Kind::Resource => "resource",
            Kind::HistoryItem => "history",
        }
    }
}
