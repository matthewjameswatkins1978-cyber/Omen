//! `omen clean`: remove definitely disposable debris (H items 7-8).
//!
//! Clean is NOT a retention-policy engine. It removes only [`Disposal::CleanSafe`]
//! items: temp files, stale scratch, abandoned staging, reproducible caches,
//! old failed downloads (`.part`). When uncertain: leave it. Consequential
//! deletion always goes through a plan first (`clean --plan`).

use crate::error::LifecycleError;
use crate::plan::{ActionPlan, PlannedItem};
use crate::state::{Disposal, StateItem, classify_tree};
use std::path::Path;

/// Build the clean plan: every `CleanSafe` item found under `base`.
/// History, evidence, pins, runtime, and unknown items are never listed.
/// Items sort deepest-first so children are removed before parents.
pub fn clean_plan(base: &Path) -> ActionPlan {
    let mut items: Vec<PlannedItem> = classify_tree(base)
        .iter()
        .filter(|i| i.disposal == Disposal::CleanSafe)
        .map(|i| {
            PlannedItem::from_state(
                i,
                "definitely disposable debris (temp/scratch/cache/failed download)",
                "file or directory is removed; reproducible from canonical truth",
            )
        })
        .collect();
    items.sort_by(|a, b| {
        b.identity
            .len()
            .cmp(&a.identity.len())
            .then_with(|| a.identity.cmp(&b.identity))
    });
    ActionPlan::new("clean", items)
}

/// Apply one clean item. Refuses directories that became non-empty in a
/// surprising way? No — removes files, removes dirs recursively; the
/// classifier already restricted the set. Returns a detail string.
pub fn apply_clean_item(base: &Path, item: &PlannedItem) -> Result<String, LifecycleError> {
    // Containment: never act outside the state base (symlink/junction
    // surprise guard). `base.join` on an absolute identity would escape,
    // so reject absolute identities outright, plus `..` escapes.
    if std::path::Path::new(&item.identity).is_absolute() {
        return Err(LifecycleError::Refused(format!(
            "clean refuses absolute identity: {}",
            item.identity
        )));
    }
    let rel = Path::new(&item.identity);
    if rel
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(LifecycleError::Refused(format!(
            "clean refuses escaping identity: {}",
            item.identity
        )));
    }
    let target = base.join(rel);
    if !target.exists() && !target.is_symlink() {
        return Ok("already gone".to_string());
    }
    // Never follow symlinks: remove the link itself only.
    if target.is_symlink() {
        std::fs::remove_file(&target).map_err(|e| LifecycleError::Io(e.to_string()))?;
        return Ok("removed symlink".to_string());
    }
    if target.is_dir() {
        std::fs::remove_dir_all(&target).map_err(|e| LifecycleError::Io(e.to_string()))?;
        Ok("removed directory".to_string())
    } else {
        std::fs::remove_file(&target).map_err(|e| LifecycleError::Io(e.to_string()))?;
        Ok("removed file".to_string())
    }
}

/// Fingerprint for stale-plan detection: size + mtime where available.
pub fn fingerprint(base: &Path, identity: &str) -> Option<String> {
    let meta = std::fs::metadata(base.join(identity)).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(format!("{}:{}", meta.len(), mtime))
}

/// Attach fingerprints to every plan item (call at plan time). Files get
/// size+mtime fingerprints; directories get none (their mtime legitimately
/// moves as children are removed — the classifier path rule remains the
/// guard for the conservative clean set).
pub fn attach_fingerprints(base: &Path, plan: &mut ActionPlan) {
    for item in &mut plan.items {
        let target = base.join(&item.identity);
        item.fingerprint = if target.is_file() {
            fingerprint(base, &item.identity)
        } else {
            None
        };
    }
}

/// Revalidate one clean item against live truth: refuse when the identity
/// no longer classifies CleanSafe, is protected, or its fingerprint moved.
pub fn revalidate_clean_item(
    base: &Path,
    protected: &std::collections::BTreeSet<String>,
    item: &PlannedItem,
) -> crate::plan::Revalidate {
    use crate::plan::Revalidate;
    use crate::state::classify;
    if protected.contains(&item.identity) {
        return Revalidate::Refuse {
            reason: format!("{} is protected now", item.identity),
        };
    }
    let rel = Path::new(&item.identity);
    let target = base.join(rel);
    // Idempotent mutations: absence at apply time is the goal already
    // achieved, not staleness. Fingerprint checks apply only when the
    // target is present.
    if !target.exists() && !target.is_symlink() {
        return Revalidate::Proceed;
    }
    let is_dir = target.is_dir();
    let size = if is_dir {
        None
    } else {
        std::fs::metadata(&target).map(|m| m.len()).ok()
    };
    let now = classify(rel, is_dir, size);
    if now.disposal != Disposal::CleanSafe {
        return Revalidate::Refuse {
            reason: format!("{} no longer classifies CleanSafe", item.identity),
        };
    }
    if let Some(want) = item.fingerprint.as_ref() {
        match fingerprint(base, &item.identity) {
            Some(have) if &have == want => {}
            _ => {
                return Revalidate::Refuse {
                    reason: format!("{} changed after plan", item.identity),
                };
            }
        }
    }
    Revalidate::Proceed
}

/// Convenience: plan + apply clean in one call with lock + revalidation.
/// Still PLAN != APPLY internally: the plan is built, fingerprinted, then
/// each item revalidated before removal. Returns the apply report.
pub fn clean_apply(base: &Path) -> Result<crate::plan::ApplyReport, LifecycleError> {
    let _guard = crate::lock::acquire(base)?;
    let mut plan = clean_plan(base);
    attach_fingerprints(base, &mut plan);
    let protected = crate::pins::load_pins(base)
        .map(|s| s.pins)
        .unwrap_or_default();
    let report = crate::plan::apply_plan(
        &plan,
        &|item| Ok(revalidate_clean_item(base, &protected, item)),
        &|item| apply_clean_item(base, item),
    );
    Ok(report)
}

/// Summarize a classified item list for human output.
pub fn summarize(items: &[StateItem]) -> (usize, u64, usize) {
    let clean: Vec<_> = items
        .iter()
        .filter(|i| i.disposal == Disposal::CleanSafe)
        .collect();
    let bytes: u64 = clean.iter().map(|i| i.size_bytes.unwrap_or(0)).sum();
    let unknown_sizes = clean.iter().filter(|i| i.size_bytes.is_none()).count();
    (clean.len(), bytes, unknown_sizes)
}
