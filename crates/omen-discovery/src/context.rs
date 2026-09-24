//! Bounded semantic context handed to providers.
//!
//! A provider receives **semantic facts**, never raw terminal soup. Omen owns
//! semantic state; Reedline owns interaction state. The context is built once
//! per request by one parser/semantic authority and shared by every provider,
//! so no provider maintains its own duplicate command grammar.

use serde::{Deserialize, Serialize};

use crate::budget::DiscoveryDepth;

/// A byte span in the request buffer.
pub use crate::candidate::TextSpan;

/// The token under the cursor, decoded by the single input grammar.
///
/// Producers outside this crate map their own tokenization into this type;
/// providers only ever read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveToken {
    /// Byte range of the raw token in the buffer.
    pub span: TextSpan,
    /// Decoded literal text before the cursor within the token.
    pub decoded_prefix: String,
    /// Decoded literal text after the cursor within the token.
    pub decoded_suffix: String,
    /// Whole decoded literal of the token.
    pub literal: String,
    /// Raw (still-quoted) text before the cursor.
    pub raw_prefix: String,
    /// Raw (still-quoted) text after the cursor.
    pub raw_suffix: String,
    /// Whether the token cannot be safely split/replaced.
    pub split_unsafe: bool,
    /// Whether an unterminated quote opens at the cursor.
    pub quote_unclosed: bool,
}

impl ActiveToken {
    /// A zero-width synthetic token at `pos` (empty input / non-token cursor).
    pub fn synthetic(pos: usize) -> Self {
        Self {
            span: TextSpan::at(pos),
            decoded_prefix: String::new(),
            decoded_suffix: String::new(),
            literal: String::new(),
            raw_prefix: String::new(),
            raw_suffix: String::new(),
            split_unsafe: false,
            quote_unclosed: false,
        }
    }
}

/// Where in the command line the cursor sits, semantically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandPosition {
    /// Cursor is in the first word (command / action name).
    CommandName,
    /// Cursor is an argument of a canonical Omen `:action`.
    ActionArg { action: String },
    /// Cursor is an argument of an external executable.
    ExecArg {
        command: String,
        /// Subcommand chain already typed after the tool name.
        chain: Vec<String>,
        /// Whether the cursor is in an option's value rather than a flag name.
        option_value_of: Option<String>,
    },
    /// Cursor is in a bare option name (e.g. `git --<Tab>`).
    OptionName { command: String, chain: Vec<String> },
}

/// A resolved executable identity for the head of the command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutableIdentity {
    pub name: String,
    /// Resolved absolute path when known.
    pub path: Option<String>,
    /// Version string when known.
    pub version: Option<String>,
    /// Content digest when known (binds cache identity).
    pub digest: Option<String>,
}

/// Bounded environment facts. Fingerprinted for cache identity; never raw soup.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EnvFacts {
    pub platform: String,
    pub path_var: Option<String>,
    pub shell: Option<String>,
}

impl EnvFacts {
    /// Stable fingerprint used to bind environment-derived cache entries.
    pub fn fingerprint(&self) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |bytes: &[u8]| {
            for b in bytes {
                hash ^= u64::from(*b);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        mix(self.platform.as_bytes());
        mix(&[0x1f]);
        mix(self.path_var.as_deref().unwrap_or("").as_bytes());
        mix(&[0x1f]);
        mix(self.shell.as_deref().unwrap_or("").as_bytes());
        hash
    }
}

/// Project / workspace identity where available.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProjectContext {
    pub root: Option<String>,
    pub manifest_digest: Option<String>,
}

/// The single semantic context shared by every provider for one request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderContext {
    /// Full input buffer.
    pub buffer: String,
    /// Cursor byte offset into `buffer`.
    pub cursor: usize,
    /// The active token under the cursor.
    pub active_token: ActiveToken,
    /// Byte range a candidate must cover.
    pub replacement_span: TextSpan,
    /// Semantic position of the cursor.
    pub command_position: CommandPosition,
    /// Resolved head executable, when known.
    pub known_executable: Option<ExecutableIdentity>,
    /// Working directory.
    pub working_dir: String,
    /// Project identity where available.
    pub project: ProjectContext,
    /// Bounded environment facts.
    pub env: EnvFacts,
    /// Requested discovery depth.
    pub depth: DiscoveryDepth,
}

impl ProviderContext {
    /// The decoded query prefix of the active token (what matching runs on).
    pub fn query(&self) -> &str {
        &self.active_token.decoded_prefix
    }

    /// The tool in scope, if the cursor is inside an external tool's grammar.
    pub fn tool_in_scope(&self) -> Option<&str> {
        match &self.command_position {
            CommandPosition::ExecArg { command, .. }
            | CommandPosition::OptionName { command, .. } => Some(command.as_str()),
            CommandPosition::ActionArg { .. } => None,
            CommandPosition::CommandName => None,
        }
    }

    /// The Omen action in scope, if the cursor is inside `:action` arguments.
    pub fn action_in_scope(&self) -> Option<&str> {
        match &self.command_position {
            CommandPosition::ActionArg { action } => Some(action.as_str()),
            _ => None,
        }
    }

    /// Whether the cursor is completing an option *name* (e.g. `--<Tab>`).
    pub fn is_option_name_position(&self) -> bool {
        matches!(self.command_position, CommandPosition::OptionName { .. })
            || (matches!(self.command_position, CommandPosition::ExecArg { .. })
                && self.query().starts_with('-'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ProviderContext {
        ProviderContext {
            buffer: "git --ver".into(),
            cursor: 9,
            active_token: ActiveToken {
                span: TextSpan::new(4, 9),
                decoded_prefix: "--ver".into(),
                decoded_suffix: String::new(),
                literal: "--ver".into(),
                raw_prefix: "--ver".into(),
                raw_suffix: String::new(),
                split_unsafe: false,
                quote_unclosed: false,
            },
            replacement_span: TextSpan::new(4, 9),
            command_position: CommandPosition::OptionName {
                command: "git".into(),
                chain: vec![],
            },
            known_executable: None,
            working_dir: ".".into(),
            project: ProjectContext::default(),
            env: EnvFacts {
                platform: "windows".into(),
                ..Default::default()
            },
            depth: DiscoveryDepth::Normal,
        }
    }

    #[test]
    fn tool_in_scope_reads_command_position() {
        assert_eq!(ctx().tool_in_scope(), Some("git"));
    }

    #[test]
    fn option_name_position_detected() {
        assert!(ctx().is_option_name_position());
    }

    #[test]
    fn env_fingerprint_is_deterministic() {
        let e = EnvFacts::default();
        assert_eq!(e.fingerprint(), e.fingerprint());
    }
}
