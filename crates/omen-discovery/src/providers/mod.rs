//! Deterministic discovery providers.
//!
//! Every source of candidates is a [`DiscoveryProvider`](crate::provider::DiscoveryProvider)
//! producing [`DiscoveredCandidate`](crate::candidate::DiscoveredCandidate).
//! There is no per-source completion system.
//!
//! ## Cost tiers
//!
//! | Provider | Tier | Authority |
//! |---|---|---|
//! | [`omen_actions::OmenActionProvider`] | 0 MemoryOnly | `OmenFact` |
//! | [`intrinsics::IntrinsicsProvider`] | 0 MemoryOnly | `Static` |
//! | [`references::ReferenceProvider`] | 0 MemoryOnly | `Static` |
//! | [`hot_index::HotIndexProvider`] | 0 MemoryOnly | `OmenFact` / `Environment` |
//! | [`subcommands::SubcommandProvider`] | 0 MemoryOnly | `Static` |
//! | [`tool_spec::ToolSpecProvider`] | 1A CheapLocal | `ToolNative` / `InstalledSpec` / `OmenFact` |
//! | [`path_commands::PathCommandsProvider`] | 1A CheapLocal | `Filesystem` |
//! | [`filesystem_paths::FilesystemPathProvider`] | 1B BlockingLocal | `Filesystem` |
//! | [`help_harvest::HelpHarvestProvider`] | 2 Subprocess | `HelpHarvest` |
//!
//! TIER 1B and TIER 2 are dispatched asynchronously and never block the
//! editor loop. TIER 3 (network) is declared for architecture only and never
//! runs on ordinary Tab in M1.
//!
//! ## Foreign completion safety
//!
//! Nothing in this module executes installed bash/zsh/fish/PowerShell
//! completion scripts, closures, `eval` or dynamic shell conditions. Only
//! inert structured data and strictly bounded deterministic help harvest feed
//! candidates.

pub mod drive;
pub mod filesystem_paths;
pub mod help_harvest;
pub mod hot_index;
pub mod intrinsics;
pub mod knowledge;
pub mod omen_actions;
pub mod path_commands;
pub mod references;
pub mod subcommands;
pub mod tool_spec;

pub use filesystem_paths::FilesystemPathProvider;
pub use help_harvest::{
    HarvestedOption, HelpHarvestCache, HelpHarvestProvider, ToolIdentity, parse_help_options,
};
pub use hot_index::{HotFact, HotIndexProvider, HotIndexSnapshot};
pub use intrinsics::IntrinsicsProvider;
pub use knowledge::OmenKnowledge;
pub use omen_actions::OmenActionProvider;
pub use path_commands::{PathCommandCache, PathCommandsProvider};
pub use references::ReferenceProvider;
pub use subcommands::SubcommandProvider;
pub use tool_spec::{
    Arity, OptionSpec, SpecOrigin, SubcommandSpec, ToolSpec, ToolSpecProvider, ToolSpecRegistry,
    ValueHint, default_tool_specs,
};
