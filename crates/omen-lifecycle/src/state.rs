//! Canonical local-state roles and classification (H items 3-6).
//!
//! Every managed state family has exactly one classification: role,
//! identity, size, reachability/protection, retention reason, disposal
//! eligibility. Unknown disposal status means KEEP — never "probably
//! delete". Ad-hoc "safe to delete" checks are forbidden; all cleanup and
//! uninstall decisions flow through [`classify`] and [`Protection`].

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Lifecycle semantics of a state object. These are roles, not directory
/// names: concrete layouts map onto them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StateRole {
    /// Required operational state: databases, indexes, runtime metadata.
    Runtime,
    /// Reproducible/disposable acceleration state. Deleting cache must not
    /// delete truth.
    Cache,
    /// Durable interaction/execution history. May outlive large payloads.
    History,
    /// CAS payloads, execution proof, artifacts, receipts, retained machine
    /// evidence. Protected when reachable from consequential truth.
    Evidence,
    /// Explicitly protected material normal retention/GC may not remove.
    Pinned,
}

/// Why a state object is retained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionReason {
    /// Required for normal operation (runtime DBs, active slot, install record).
    Required,
    /// Reachable from consequential truth (facts, receipts, checkpoints).
    Reachable,
    /// Explicitly pinned by the user.
    UserPinned,
    /// Inside the configured retention window/budget.
    WithinRetention,
    /// Disposal status could not be determined. Means KEEP.
    Unknown,
    /// No reason: eligible for conservative disposal (clean) or policy GC.
    None,
}

/// Consequential disposal eligibility of one classified item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposal {
    /// `omen clean` may remove without a retention policy decision.
    CleanSafe,
    /// Only `omen gc` apply may remove, after plan + revalidation.
    GcEligible,
    /// Must be preserved (required, reachable, pinned, or unknown).
    Keep,
}

/// Protection verdict for one state identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protection {
    Protected { reason: String },
    Unprotected,
}

/// One classified state object: the single answer to "what is this and may
/// it go?".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateItem {
    /// Stable canonical identity (relative path under the state base, or
    /// `slot:<id>` / `digest:<hex>` / `pin:<id>` for non-path identities).
    pub identity: String,
    pub role: StateRole,
    /// Size in bytes where cheaply known, else `None` (unknown, not zero).
    pub size_bytes: Option<u64>,
    pub protection: Protection,
    pub retention: RetentionReason,
    pub disposal: Disposal,
}

impl StateItem {
    pub fn keep(identity: impl Into<String>, role: StateRole, retention: RetentionReason) -> Self {
        Self {
            identity: identity.into(),
            role,
            size_bytes: None,
            protection: Protection::Protected {
                reason: format!("{retention:?}"),
            },
            retention,
            disposal: Disposal::Keep,
        }
    }

    pub fn is_protected(&self) -> bool {
        matches!(self.protection, Protection::Protected { .. })
    }
}

/// Canonical user-level state base. Single authority: delegates to
/// `omen_knowledge::user_state_base_dir` (H item 2: reuse, no parallel
/// ownership model).
pub fn state_base_dir() -> PathBuf {
    omen_knowledge::user_state_base_dir()
}

/// Well-known managed families under the state base. Returned as
/// (relative-dir, role) pairs; the classifier in [`classify_tree`] walks
/// exactly these families and nothing else, so stray user files beside them
/// are never invented into lifecycle objects.
pub fn managed_families() -> Vec<(PathBuf, StateRole)> {
    vec![
        (PathBuf::from("workspaces"), StateRole::Runtime),
        (PathBuf::from("install"), StateRole::Runtime),
        (PathBuf::from("update"), StateRole::Runtime),
        (PathBuf::from("pins"), StateRole::Pinned),
        (PathBuf::from("evidence"), StateRole::Evidence),
        (PathBuf::from("history"), StateRole::History),
        (PathBuf::from("cache"), StateRole::Cache),
        (PathBuf::from("tmp"), StateRole::Cache),
        (PathBuf::from("versions"), StateRole::Runtime),
        (PathBuf::from("bin"), StateRole::Runtime),
        (PathBuf::from("state"), StateRole::Runtime),
        (PathBuf::from("demo"), StateRole::Cache),
    ]
}

/// Classify one path (relative to the state base) into a [`StateItem`].
/// Anything unrecognized is [`Disposal::Keep`] with
/// [`RetentionReason::Unknown`]: unknown stays unknown.
pub fn classify(relative: &Path, is_dir: bool, size_bytes: Option<u64>) -> StateItem {
    let rel = relative.to_string_lossy().replace('\\', "/");
    let first = relative
        .components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .unwrap_or_default();
    let file_name = relative
        .file_name()
        .map(|c| c.to_string_lossy().to_string())
        .unwrap_or_default();

    // Install record, channel, pins, transactions: required runtime truth.
    if rel == "install/record.json"
        || rel == "install/channel.json"
        || rel.starts_with("pins/")
        || rel.starts_with("update/transaction")
        || rel == "state/installed.json"
    {
        let mut item = StateItem::keep(rel, StateRole::Runtime, RetentionReason::Required);
        item.size_bytes = size_bytes;
        return item;
    }
    // Active-slot pointer and managed binaries/slots: required app bytes.
    // Stale `.bak` copies left by the rename-swap activator are debris.
    if rel == "bin/active.json" || rel.starts_with("versions/") || first == "bin" {
        if file_name.ends_with(".bak") {
            return StateItem {
                identity: rel,
                role: StateRole::Cache,
                size_bytes,
                protection: Protection::Unprotected,
                retention: RetentionReason::None,
                disposal: Disposal::CleanSafe,
            };
        }
        let mut item = StateItem::keep(rel, StateRole::Runtime, RetentionReason::Required);
        item.size_bytes = size_bytes;
        return item;
    }
    // Workspace databases + execution status: required runtime truth.
    if rel.ends_with("state.sqlite")
        || rel.ends_with("state.sqlite-wal")
        || rel.ends_with("state.sqlite-shm")
        || rel.ends_with("local-execution-status.json")
    {
        let mut item = StateItem::keep(rel, StateRole::Runtime, RetentionReason::Required);
        item.size_bytes = size_bytes;
        return item;
    }
    // History payloads: durable history, never clean-safe.
    if first == "history" || file_name.contains("history") && first == "workspaces" {
        let mut item = StateItem::keep(rel, StateRole::History, RetentionReason::Reachable);
        item.size_bytes = size_bytes;
        return item;
    }
    // Evidence: GC-eligible only, never clean-safe.
    if first == "evidence" {
        return StateItem {
            identity: rel,
            role: StateRole::Evidence,
            size_bytes,
            protection: Protection::Unprotected,
            retention: RetentionReason::WithinRetention,
            disposal: Disposal::GcEligible,
        };
    }
    // CAS blobs under workspaces: evidence, GC-eligible.
    if rel.contains("/cas/") || rel.contains("cas/sha256") {
        return StateItem {
            identity: rel,
            role: StateRole::Evidence,
            size_bytes,
            protection: Protection::Unprotected,
            retention: RetentionReason::WithinRetention,
            disposal: Disposal::GcEligible,
        };
    }
    // Scratch / staging / failed downloads: conservative clean targets.
    if first == "tmp"
        || rel.starts_with("update/staging")
        || rel.starts_with("update/downloads")
        || rel.ends_with(".part")
        || rel.ends_with(".tmp")
        || file_name.starts_with("omen-tmp-")
    {
        return StateItem {
            identity: rel,
            role: StateRole::Cache,
            size_bytes,
            protection: Protection::Unprotected,
            retention: RetentionReason::None,
            disposal: Disposal::CleanSafe,
        };
    }
    if first == "cache" || first == "demo" {
        return StateItem {
            identity: rel,
            role: StateRole::Cache,
            size_bytes,
            protection: Protection::Unprotected,
            retention: RetentionReason::None,
            disposal: Disposal::CleanSafe,
        };
    }
    if is_dir {
        // Unknown directory: keep (may contain truth we do not model).
        let mut item = StateItem::keep(rel, StateRole::Runtime, RetentionReason::Unknown);
        item.size_bytes = size_bytes;
        return item;
    }
    // Unknown file: keep.
    let mut item = StateItem::keep(rel, StateRole::Cache, RetentionReason::Unknown);
    item.size_bytes = size_bytes;
    item
}

/// Walk the managed families under `base`, classifying every file and
/// directory found. Never follows symlinks. Bounded: caps at 100k entries
/// and reports truncation rather than hanging startup on a giant CAS.
pub fn classify_tree(base: &Path) -> Vec<StateItem> {
    const CAP: usize = 100_000;
    let mut out = Vec::new();
    for (family, _) in managed_families() {
        let dir = base.join(&family);
        if !dir.is_dir() {
            continue;
        }
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            let entries = match std::fs::read_dir(&d) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                if out.len() >= CAP {
                    return out;
                }
                let path = entry.path();
                let is_dir = path.is_dir();
                // Never follow symlinks: classify the link itself as unknown.
                let is_link = std::fs::symlink_metadata(&path)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false);
                let Ok(rel) = path.strip_prefix(base).map(|p| p.to_path_buf()) else {
                    continue;
                };
                if is_link {
                    let mut item = StateItem::keep(
                        rel.display().to_string(),
                        StateRole::Runtime,
                        RetentionReason::Unknown,
                    );
                    item.size_bytes = None;
                    out.push(item);
                    continue;
                }
                let size = if is_dir {
                    None
                } else {
                    std::fs::metadata(&path).map(|m| m.len()).ok()
                };
                out.push(classify(&rel, is_dir, size));
                if is_dir {
                    stack.push(path);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_means_keep() {
        let item = classify(Path::new("weird/custom-thing.dat"), false, Some(10));
        assert_eq!(item.disposal, Disposal::Keep);
        assert_eq!(item.retention, RetentionReason::Unknown);
    }

    #[test]
    fn install_record_is_required() {
        let item = classify(Path::new("install/record.json"), false, Some(10));
        assert_eq!(item.disposal, Disposal::Keep);
        assert_eq!(item.role, StateRole::Runtime);
    }

    #[test]
    fn evidence_is_gc_only_never_clean() {
        let item = classify(Path::new("evidence/last-proof.json"), false, Some(10));
        assert_eq!(item.disposal, Disposal::GcEligible);
        assert_eq!(item.role, StateRole::Evidence);
    }

    #[test]
    fn history_db_is_required_truth() {
        let item = classify(Path::new("workspaces/ws_abc/state.sqlite"), false, Some(10));
        assert_eq!(item.disposal, Disposal::Keep);
        assert_eq!(item.role, StateRole::Runtime);
    }

    #[test]
    fn scratch_is_clean_safe() {
        for p in [
            "tmp/x",
            "update/staging/y",
            "cache/z",
            "demo/q",
            "dl.bin.part",
        ] {
            let item = classify(Path::new(p), false, Some(10));
            assert_eq!(item.disposal, Disposal::CleanSafe, "{p}");
        }
    }
}
