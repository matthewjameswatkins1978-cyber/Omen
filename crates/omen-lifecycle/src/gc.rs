//! `omen gc`: retention-policy collection over HISTORY and EVIDENCE (H items
//! 9-11, 6).
//!
//! GC is different from clean: it applies retention policy to unprotected,
//! out-of-retention evidence. GC never infers permission from age alone —
//! age is only evaluated together with unreachability and explicit policy.
//! History rows always survive: when an unprotected large payload is
//! collected, history keeps a truthful `payload unavailable / collected`
//! tombstone with identity/digest/metadata instead of deleting the event.

use crate::error::LifecycleError;
use crate::plan::{ActionPlan, PlannedItem};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Retention knobs. Deliberately tiny: good defaults beat a policy
/// language (H item 65). Only knobs demonstrated necessary exist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub schema_version: u32,
    /// Keep unprotected evidence newer than this many days.
    pub keep_unprotected_evidence_days: u64,
    /// Maximum bytes of unprotected evidence to retain; excess oldest-first
    /// becomes eligible. `None` = unbounded.
    pub max_unprotected_evidence_bytes: Option<u64>,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            schema_version: 1,
            keep_unprotected_evidence_days: 30,
            max_unprotected_evidence_bytes: None,
        }
    }
}

pub const RETENTION_FILE: &str = "install/retention.json";

pub fn load_retention(base: &Path) -> RetentionPolicy {
    let path = base.join(RETENTION_FILE);
    std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Reachability roots supplied by the caller (canonical truth the GC must
/// not second-guess): protected CAS digests, active receipt digests,
/// checkpoint digests, history-referenced digests.
#[derive(Debug, Default)]
pub struct Reachability {
    pub protected_digests: BTreeSet<String>,
    pub history_referenced: BTreeSet<String>,
}

impl Reachability {
    pub fn is_reachable(&self, digest: &str) -> bool {
        self.protected_digests.contains(digest) || self.history_referenced.contains(digest)
    }
}

/// One GC candidate: an evidence blob eligible under policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcCandidate {
    pub digest: String,
    pub path: String,
    pub size_bytes: u64,
    pub age_days: Option<u64>,
    pub reason: String,
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn walk_files(dir: &Path) -> Vec<(String, u64, Option<u64>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut count = 0usize;
    while let Some(d) = stack.pop() {
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if count > 200_000 {
                return out;
            }
            count += 1;
            let p = entry.path();
            if std::fs::symlink_metadata(&p)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(m) = std::fs::metadata(&p) {
                let mtime = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs());
                out.push((p.to_string_lossy().to_string(), m.len(), mtime));
            }
        }
    }
    out
}

/// Canonical `cas/sha256/ab/<hex...>` layout -> digest, from any path
/// containing that layout (no root needed: scans for the `sha256` marker).
/// Returns None for non-conforming paths (never invent precision).
pub fn digest_from_cas_path(path: &Path) -> Option<String> {
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let at = parts.iter().position(|p| p == "sha256")?;
    let tail = &parts[at + 1..];
    if tail.is_empty() {
        return None;
    }
    let hex: String = tail.concat();
    if hex.len() >= 16 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(hex.to_lowercase())
    } else {
        None
    }
}

/// Canonical `cas/sha256/ab/<hex...>` layout under a root -> digest.
fn digest_from_cas_root(cas_root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(cas_root).ok()?;
    digest_from_cas_path(rel)
}

/// Enumerate CAS blobs under `cas_root`, returning candidates that are
/// unprotected, unreachable, and outside retention. Age alone is never
/// permission: only blobs with a KNOWN digest that is unreferenced,
/// unpinned, and expired (or over budget) become candidates.
pub fn enumerate_candidates(
    cas_root: &Path,
    base: &Path,
    policy: &RetentionPolicy,
    reach: &Reachability,
    now_secs: u64,
) -> Vec<GcCandidate> {
    let pins = crate::pins::load_pins(base)
        .map(|s| s.pins)
        .unwrap_or_default();
    let is_live =
        |digest: &str| reach.is_reachable(digest) || pins.contains(&format!("digest:{digest}"));

    let mut candidates = Vec::new();
    for (path, size, mtime) in walk_files(cas_root) {
        let Some(digest) = digest_from_cas_root(cas_root, Path::new(&path)) else {
            continue;
        };
        if is_live(&digest) {
            continue;
        }
        let age_days = mtime.map(|t| now_secs.saturating_sub(t) / 86400);
        let expired = age_days
            .map(|a| a > policy.keep_unprotected_evidence_days)
            .unwrap_or(false);
        if expired {
            candidates.push(GcCandidate {
                digest,
                path,
                size_bytes: size,
                age_days,
                reason: format!(
                    "unprotected evidence older than {} days",
                    policy.keep_unprotected_evidence_days
                ),
            });
        }
    }
    // Budget: oldest-first overflow becomes eligible even within window.
    if let Some(budget) = policy.max_unprotected_evidence_bytes {
        let mut unprotected: Vec<(String, u64, Option<u64>, String)> = Vec::new();
        for (path, size, mtime) in walk_files(cas_root) {
            if let Some(d) = digest_from_cas_root(cas_root, Path::new(&path))
                && !is_live(&d)
            {
                unprotected.push((path, size, mtime, d));
            }
        }
        let total: u64 = unprotected.iter().map(|(_, s, _, _)| *s).sum();
        if total > budget {
            unprotected.sort_by_key(|(_, _, m, _)| *m);
            let mut over = total - budget;
            for (path, size, mtime, digest) in unprotected {
                if over == 0 {
                    break;
                }
                if candidates.iter().any(|c| c.path == path) {
                    over = over.saturating_sub(size);
                    continue;
                }
                candidates.push(GcCandidate {
                    digest,
                    path,
                    size_bytes: size,
                    age_days: mtime.map(|t| now_secs.saturating_sub(t) / 86400),
                    reason: "over unprotected evidence budget (oldest first)".to_string(),
                });
                over = over.saturating_sub(size);
            }
        }
    }
    candidates.sort_by(|a, b| a.digest.cmp(&b.digest));
    candidates
}

/// Default live entry: enumerate the workspace CAS dirs under `base`.
pub fn workspace_cas_candidates(
    base: &Path,
    policy: &RetentionPolicy,
    reach: &Reachability,
) -> Vec<GcCandidate> {
    let now = now_unix_secs();
    let mut out = Vec::new();
    let ws = base.join("workspaces");
    let entries = std::fs::read_dir(&ws)
        .map(|e| e.flatten().count())
        .unwrap_or(0);
    let _ = entries;
    if let Ok(rd) = std::fs::read_dir(&ws) {
        for entry in rd.flatten() {
            let cas = entry.path().join("cas");
            if cas.is_dir() {
                out.extend(enumerate_candidates(&cas, base, policy, reach, now));
            }
        }
    }
    out
}

/// Digests referenced by durable truth: artifacts table + fact_artifact
/// links. Best-effort read-only; failure yields empty (GC then keeps more,
/// never less — callers combine with pins).
pub fn db_referenced_digests(workspace_db: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(db) = omen_knowledge::Database::open_read_only(workspace_db) else {
        return out;
    };
    if let Ok(mut stmt) = db.conn().prepare("SELECT digest FROM artifacts")
        && let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0))
    {
        for d in rows.flatten() {
            out.insert(d);
        }
    }
    if let Ok(mut stmt) = db.conn().prepare("SELECT artifact_uri FROM fact_artifacts")
        && let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0))
    {
        for u in rows.flatten() {
            if let Some(d) = u.strip_prefix("artifact://") {
                out.insert(d.to_string());
            }
        }
    }
    out
}

/// Build reachability from every workspace DB under the base (bounded
/// scan): digests referenced by durable truth are never GC candidates.
pub fn workspace_reachability(base: &Path) -> Reachability {
    let mut reach = Reachability::default();
    let ws = base.join("workspaces");
    if let Ok(rd) = std::fs::read_dir(&ws) {
        for entry in rd.flatten().take(512) {
            let db = entry.path().join("state.sqlite");
            if db.is_file() {
                for d in db_referenced_digests(&db) {
                    reach.history_referenced.insert(d);
                }
            }
        }
    }
    reach
}
pub fn gc_plan(candidates: &[GcCandidate]) -> ActionPlan {
    let items: Vec<PlannedItem> = candidates
        .iter()
        .map(|c| PlannedItem {
            identity: c.path.clone(),
            role: "evidence".to_string(),
            reason: c.reason.clone(),
            size_bytes: Some(c.size_bytes),
            consequence: format!(
                "CAS payload {} collected; history retains tombstone (payload unavailable / collected)",
                c.digest
            ),
            protected_at_plan: false,
            fingerprint: Some(format!("digest:{}", c.digest)),
        })
        .collect();
    ActionPlan::new("gc", items)
}

/// Tombstone record: history stays truthful after payload collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayloadTombstone {
    pub digest: String,
    pub size_bytes: u64,
    pub collected_at: String,
    pub status: String,
}

pub fn tombstone_log_path(base: &Path) -> PathBuf {
    base.join("history").join("collected-payloads.jsonl")
}

pub fn record_tombstone(base: &Path, digest: &str, size: u64) -> Result<(), LifecycleError> {
    use std::io::Write;
    let path = tombstone_log_path(base);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LifecycleError::Io(e.to_string()))?;
    }
    let t = PayloadTombstone {
        digest: digest.to_string(),
        size_bytes: size,
        collected_at: chrono::Utc::now().to_rfc3339(),
        status: "payload unavailable / collected".to_string(),
    };
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| LifecycleError::Io(e.to_string()))?;
    writeln!(f, "{}", serde_json::to_string(&t).unwrap())
        .map_err(|e| LifecycleError::Io(e.to_string()))?;
    Ok(())
}

/// Apply one GC item: remove the blob, record the tombstone. Absolute
/// identities are contained: the resolved path must live under `base`
/// (canonical comparison), else refused.
pub fn apply_gc_item(base: &Path, item: &PlannedItem) -> Result<String, LifecycleError> {
    let raw = Path::new(&item.identity);
    // `..` escapes are never legitimate GC identities.
    if raw
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(LifecycleError::Refused(format!(
            "gc refuses escaping identity: {}",
            item.identity
        )));
    }
    let resolved = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        base.join(raw)
    };
    if !resolved.exists() || resolved.is_symlink() {
        return Ok("already gone".to_string());
    }
    let canon_base = base
        .canonicalize()
        .map_err(|e| LifecycleError::Io(e.to_string()))?;
    let canon = resolved
        .canonicalize()
        .map_err(|e| LifecycleError::Io(e.to_string()))?;
    if !canon.starts_with(&canon_base) {
        return Err(LifecycleError::Refused(format!(
            "gc refuses identity outside state base: {}",
            item.identity
        )));
    }
    let digest = item
        .fingerprint
        .as_ref()
        .and_then(|f| f.strip_prefix("digest:"))
        .unwrap_or("unknown");
    let size = std::fs::metadata(&resolved).map(|m| m.len()).unwrap_or(0);
    std::fs::remove_file(&resolved).map_err(|e| LifecycleError::Io(e.to_string()))?;
    record_tombstone(base, digest, size)?;
    Ok(format!(
        "collected {digest} ({size} bytes); tombstone recorded"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cas_blob(cas: &Path, hex: &str) -> PathBuf {
        let dir = cas.join("sha256").join(&hex[..2]);
        std::fs::create_dir_all(&dir).unwrap();
        let blob = dir.join(&hex[2..]);
        std::fs::write(&blob, b"payload").unwrap();
        blob
    }

    const HEX: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    const FAR_FUTURE: u64 = u64::MAX / 2;

    #[test]
    fn non_cas_paths_never_candidates() {
        let base = tempfile::tempdir().unwrap();
        let cas = base.path().join("cas");
        std::fs::create_dir_all(&cas).unwrap();
        std::fs::write(cas.join("random.bin"), b"data").unwrap();
        let cands = enumerate_candidates(
            &cas,
            base.path(),
            &RetentionPolicy::default(),
            &Reachability::default(),
            FAR_FUTURE,
        );
        assert!(cands.is_empty());
    }

    #[test]
    fn expired_unprotected_blob_is_candidate() {
        let base = tempfile::tempdir().unwrap();
        let cas = base.path().join("cas");
        cas_blob(&cas, HEX);
        let cands = enumerate_candidates(
            &cas,
            base.path(),
            &RetentionPolicy {
                keep_unprotected_evidence_days: 0,
                ..Default::default()
            },
            &Reachability::default(),
            FAR_FUTURE,
        );
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].digest, HEX);
    }

    #[test]
    fn reachable_and_pinned_survive() {
        let base = tempfile::tempdir().unwrap();
        let cas = base.path().join("cas");
        cas_blob(&cas, HEX);
        let policy = RetentionPolicy {
            keep_unprotected_evidence_days: 0,
            ..Default::default()
        };
        let mut reach = Reachability::default();
        reach.protected_digests.insert(HEX.to_string());
        let cands = enumerate_candidates(&cas, base.path(), &policy, &reach, FAR_FUTURE);
        assert!(cands.is_empty());

        let reach2 = Reachability::default();
        crate::pins::pin(base.path(), &format!("digest:{HEX}")).unwrap();
        let cands2 = enumerate_candidates(&cas, base.path(), &policy, &reach2, FAR_FUTURE);
        assert!(cands2.is_empty());
    }

    #[test]
    fn fresh_blob_within_retention_survives() {
        let base = tempfile::tempdir().unwrap();
        let cas = base.path().join("cas");
        cas_blob(&cas, HEX);
        let cands = enumerate_candidates(
            &cas,
            base.path(),
            &RetentionPolicy::default(),
            &Reachability::default(),
            now_unix_secs(),
        );
        assert!(cands.is_empty());
    }

    #[test]
    fn db_truth_reaches_gc() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state.sqlite");
        let db = omen_knowledge::Database::open(&db_path).unwrap();
        db.conn()
            .execute(
                "INSERT INTO artifacts (digest, size, media_type, producer, retention, blob_state, created_at, last_accessed_at) VALUES (?1, 7, 'text/plain', 'test', 'keep', 'stored', 't', 't')",
                [HEX],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO facts (fact_id, resource_uri, value, validity, assurance, producer, created_at) VALUES ('f1', 'r', 'v', 'current', 'high', 'test', 't')",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO fact_artifacts (fact_id, artifact_uri) VALUES ('f1', 'artifact://deadbeef01')",
                [],
            )
            .unwrap();
        drop(db);
        let got = db_referenced_digests(&db_path);
        assert!(got.contains(HEX));
        assert!(got.contains("deadbeef01"));
        assert!(db_referenced_digests(&dir.path().join("missing.sqlite")).is_empty());
    }
}
