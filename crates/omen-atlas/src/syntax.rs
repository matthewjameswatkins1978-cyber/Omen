use serde::{Deserialize, Serialize};

/// Command syntax metadata (e.g. from Carapace or Fig).
///
/// Crucial Rule (Section 32):
/// Completion specifications provide syntax knowledge only.
/// They do NOT create trusted effect semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSyntaxSpecification {
    pub name: String,
    pub description: Option<String>,
    pub subcommands: Vec<CommandSyntaxSpecification>,
    pub flags: Vec<FlagSyntaxSpecification>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlagSyntaxSpecification {
    pub name: String,
    pub shorthand: Option<String>,
    pub description: Option<String>,
    pub takes_value: bool,
}
