//! Filesystem path provider.
//!
//! **Authority:** [`Authority::Filesystem`]. **TIER 1B**: `readdir` may touch
//! huge directories, removable media, network shares, slow mounts or
//! pathological filesystems. Local does **not** mean cheap. This provider is
//! therefore `BlockingLocal` and the scheduler always dispatches it
//! asynchronously; it never runs inline on the editor thread.
//!
//! Preserves M0 Windows semantics: a bare drive designator (`D:`) is
//! navigation grammar; `D:foo` is **not** rewritten to `D:\foo`; no hard-coded
//! path-separator assumptions.

use std::path::{Path, PathBuf};

use crate::authority::{Authority, AuthorityClass};
use crate::budget::{CostTier, DiscoveryBudget};
use crate::candidate::{Description, DiscoveredCandidate, Display, SafetyAnnotation};
use crate::context::{CommandPosition, ProviderContext};
use crate::identity::{EvidenceIdentity, ProvenanceKey, SemanticKey, SemanticNamespace};
use crate::kind::Kind;
use crate::outcome::{DeclineReason, PartialReason, ProviderError, ProviderOutcome};
use crate::provider::{
    Determinism, DiscoveryProvider, FreshnessPolicy, ProviderId, TriggerDecision,
};

/// Maximum directory entries examined per request.
const MAX_FS_SCAN: usize = 1024;
/// Maximum entries returned per request.
const MAX_FS_ENTRIES: usize = 256;

/// Discovers files and/or directories for a path token.
pub struct FilesystemPathProvider {
    /// Only directories (e.g. `cd <value>`).
    dirs_only: bool,
}

impl FilesystemPathProvider {
    pub fn new() -> Self {
        Self { dirs_only: false }
    }

    /// Restricts output to directories (used for `cd`).
    pub fn directories_only() -> Self {
        Self { dirs_only: true }
    }
}

impl Default for FilesystemPathProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscoveryProvider for FilesystemPathProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("filesystem-paths")
    }

    fn supported_kinds(&self) -> &'static [Kind] {
        &[Kind::Directory, Kind::File]
    }

    fn triggers(&self, ctx: &ProviderContext) -> TriggerDecision {
        let q = ctx.query();
        match &ctx.command_position {
            CommandPosition::CommandName => {
                // Path-ish tokens only: explicit separators, relative prefix, or
                // a bare Windows drive designator.
                if q.contains('/')
                    || q.contains('\\')
                    || q.starts_with('.')
                    || (crate::providers::drive::is_drive_designator)(q).is_some()
                {
                    TriggerDecision::Apply
                } else {
                    TriggerDecision::Skip
                }
            }
            CommandPosition::ExecArg { .. } | CommandPosition::ActionArg { .. } => {
                TriggerDecision::Apply
            }
            CommandPosition::OptionName { .. } => TriggerDecision::Skip,
        }
    }

    fn cost_tier(&self) -> CostTier {
        // Potentially blocking I/O: never inline.
        CostTier::BlockingLocal
    }

    fn determinism(&self) -> Determinism {
        Determinism::Deterministic
    }

    fn authority_capabilities(&self) -> &'static [AuthorityClass] {
        &[AuthorityClass::Filesystem]
    }

    fn freshness(&self) -> FreshnessPolicy {
        FreshnessPolicy::IdentityBound
    }

    fn discover(&mut self, ctx: &ProviderContext, budget: &DiscoveryBudget) -> ProviderOutcome {
        if budget.is_exhausted() {
            return ProviderOutcome::Declined {
                reason: DeclineReason::BudgetExhausted,
            };
        }

        let query = ctx.query();
        // Drive-relative tokens (`D:foo`) are outside M1 scope and must NOT be
        // rewritten to `D:\foo`. Decline rather than invent.
        if (crate::providers::drive::is_drive_designator)(query).is_some() {
            // A bare designator `D:` is navigation grammar for the drive root.
            // Listing the drive root is legitimate; the prefix stays `D:\` only
            // when the user already typed a separator after the colon.
            // We do not invent `D:\` from `D:`.
        }
        if is_drive_relative(query) {
            return ProviderOutcome::Declined {
                reason: DeclineReason::NotApplicable,
            };
        }

        let dirs_only = self.dirs_only
            || matches!(&ctx.command_position, CommandPosition::ExecArg { command, .. } if command == "cd");

        let entries = match list_dir_bounded(ctx, query, dirs_only, budget) {
            Ok(e) => e,
            Err(err) => {
                return ProviderOutcome::Failed { error: err };
            }
        };

        if entries.is_empty() {
            return ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch,
            };
        }

        let span = ctx.replacement_span;
        let (parent_raw, _) = split_path_prefix(query);
        let sep = preferred_sep(query);
        let truncated = entries.len() >= MAX_FS_ENTRIES;

        let mut candidates = Vec::new();
        for e in entries {
            let mut literal = String::new();
            literal.push_str(&parent_raw);
            if !parent_raw.is_empty() && !parent_raw.ends_with('/') && !parent_raw.ends_with('\\') {
                literal.push(sep);
            }
            literal.push_str(&e.name);
            if e.is_dir {
                literal.push(sep);
            }
            let kind = if e.is_dir {
                Kind::Directory
            } else {
                Kind::File
            };
            let root = ctx.working_dir.clone();
            candidates.push(
                DiscoveredCandidate::new(
                    SemanticKey::new(
                        kind,
                        &literal,
                        SemanticNamespace::Path { root: root.clone() },
                    ),
                    ProvenanceKey::new(
                        "filesystem-paths",
                        AuthorityClass::Filesystem,
                        EvidenceIdentity::Filesystem {
                            root: root.clone(),
                            mtime_unix: None,
                        },
                    ),
                    literal,
                    Authority::Filesystem {
                        root,
                        observed_at_unix: 0,
                    },
                    span,
                )
                .with_display(Display::new(&e.name))
                .with_description(Description::short(if e.is_dir {
                    "directory"
                } else {
                    "file"
                }))
                .with_safety(SafetyAnnotation::SafeReadOnly),
            );
        }

        if truncated {
            return ProviderOutcome::Partial {
                candidates,
                reason: PartialReason::CandidateCap,
            };
        }
        ProviderOutcome::Answered { candidates }
    }
}

struct PathEntry {
    name: String,
    is_dir: bool,
}

/// `D:foo` is drive-relative and out of scope; `D:` alone is a designator.
fn is_drive_relative(q: &str) -> bool {
    let b = q.as_bytes();
    if b.len() >= 3 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        // `D:foo` (no separator after colon) => drive-relative.
        return b[2] != b'/' && b[2] != b'\\';
    }
    false
}

fn preferred_sep(prefix: &str) -> char {
    if prefix.contains('\\') && !prefix.contains('/') {
        '\\'
    } else if prefix.contains('/') {
        '/'
    } else if cfg!(windows) {
        '\\'
    } else {
        '/'
    }
}

/// Splits a typed path into (parent_with_separators, leaf_prefix).
///
/// A bare drive designator `D:` is treated as the drive root prefix `D:\`;
/// the caller decides whether to invent separators. We never rewrite `D:foo`.
fn split_path_prefix(prefix: &str) -> (String, String) {
    if (crate::providers::drive::is_drive_designator)(prefix).is_some() {
        return (format!("{prefix}\\"), String::new());
    }
    let bytes = prefix.as_bytes();
    let mut split_at = None;
    for (i, b) in bytes.iter().enumerate().rev() {
        if *b == b'/' || *b == b'\\' {
            split_at = Some(i);
            break;
        }
    }
    match split_at {
        Some(i) => (prefix[..=i].to_string(), prefix[i + 1..].to_string()),
        None => (String::new(), prefix.to_string()),
    }
}

fn list_dir_bounded(
    ctx: &ProviderContext,
    prefix: &str,
    dirs_only: bool,
    budget: &DiscoveryBudget,
) -> Result<Vec<PathEntry>, ProviderError> {
    let (parent_raw, leaf) = split_path_prefix(prefix);
    let parent_path: PathBuf = if parent_raw.is_empty() {
        PathBuf::from(&ctx.working_dir)
    } else {
        let p = Path::new(&parent_raw);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            Path::new(&ctx.working_dir).join(p)
        }
    };

    let read_dir = std::fs::read_dir(&parent_path).map_err(|e| {
        ProviderError::new(
            "discovery.provider.filesystem-paths.readdir",
            format!("readdir failed: {e}"),
        )
    })?;

    let mut scanned: Vec<PathEntry> = Vec::new();
    for (i, entry) in read_dir.flatten().enumerate() {
        if i >= MAX_FS_SCAN || budget.is_exhausted() {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if dirs_only && !is_dir {
            continue;
        }
        if leaf.is_empty() || name.to_lowercase().starts_with(&leaf.to_lowercase()) {
            scanned.push(PathEntry { name, is_dir });
        }
    }

    scanned.sort_by(|a, b| a.name.cmp(&b.name).then(a.is_dir.cmp(&b.is_dir)));
    scanned.truncate(MAX_FS_ENTRIES);
    Ok(scanned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(q: &str, cwd: &str) -> ProviderContext {
        ProviderContext {
            buffer: q.to_string(),
            cursor: q.len(),
            active_token: crate::context::ActiveToken {
                span: crate::candidate::TextSpan::new(0, q.len()),
                decoded_prefix: q.to_string(),
                decoded_suffix: String::new(),
                literal: q.to_string(),
                raw_prefix: q.to_string(),
                raw_suffix: String::new(),
                split_unsafe: false,
                quote_unclosed: false,
            },
            replacement_span: crate::candidate::TextSpan::new(0, q.len()),
            command_position: CommandPosition::ExecArg {
                command: "cd".into(),
                chain: vec![],
                option_value_of: None,
            },
            known_executable: None,
            working_dir: cwd.to_string(),
            project: Default::default(),
            env: Default::default(),
            depth: crate::budget::DiscoveryDepth::Normal,
        }
    }

    #[test]
    fn tier_is_blocking_local_and_never_inline() {
        let p = FilesystemPathProvider::new();
        assert_eq!(p.cost_tier(), CostTier::BlockingLocal);
        assert!(p.cost_tier().must_be_async());
    }

    #[test]
    fn drive_relative_token_declines_rather_than_inventing() {
        let mut p = FilesystemPathProvider::new();
        let out = p.discover(&ctx("D:foo", "."), &DiscoveryBudget::allowing_subprocess());
        assert!(matches!(
            out,
            ProviderOutcome::Declined {
                reason: DeclineReason::NotApplicable
            }
        ));
    }

    #[test]
    fn discovers_directories_only_for_cd() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("OmenWork")).unwrap();
        std::fs::write(tmp.path().join("OmenFile.txt"), b"x").unwrap();

        let mut p = FilesystemPathProvider::directories_only();
        let out = p.discover(
            &ctx("Om", tmp.path().to_str().unwrap()),
            &DiscoveryBudget::allowing_subprocess(),
        );
        let names: Vec<String> = out
            .candidates()
            .iter()
            .map(|c| c.value.insert.clone())
            .collect();
        assert!(names.iter().any(|n| n.starts_with("OmenWork")));
        assert!(
            names.iter().all(|n| !n.ends_with(".txt")),
            "cd must not offer files"
        );
    }

    #[test]
    fn replacement_span_is_exact() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("alpha")).unwrap();
        let q = "al";
        let mut p = FilesystemPathProvider::directories_only();
        let out = p.discover(
            &ctx(q, tmp.path().to_str().unwrap()),
            &DiscoveryBudget::allowing_subprocess(),
        );
        assert!(!out.candidates().is_empty());
        assert_eq!(
            out.candidates()[0].replacement_span,
            crate::candidate::TextSpan::new(0, 2)
        );
    }
}
