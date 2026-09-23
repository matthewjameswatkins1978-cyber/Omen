//! Uninstall with retention classes (H items 44-47).
//!
//! Uninstall is NOT "delete everything". Default removes application
//! bytes + runtime integration while preserving valuable user truth
//! (history, evidence, pins) unless explicitly chosen otherwise. Pinned or
//! protected data gets explicit treatment; evidence destruction is never
//! smuggled into a generic `--force`.

use crate::error::LifecycleError;
use crate::plan::{ActionPlan, PlannedItem};
use crate::state::{Disposal, StateRole, classify_tree};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UninstallScope {
    /// Remove app bytes + integration; preserve history/evidence/pins.
    AppOnly,
    /// AppOnly + disposable cache/tmp.
    AppAndCache,
    /// Everything Omen manages under the base, honoring pins/protection
    /// explicitly (pinned items are listed as refused, never deleted).
    Everything,
}

/// Retention class of one uninstall item for the human plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    Application,
    Cache,
    Configuration,
    History,
    Evidence,
    Pinned,
}

pub fn retention_class(identity: &str, role: StateRole) -> RetentionClass {
    if identity.starts_with("pins/") || role == StateRole::Pinned {
        return RetentionClass::Pinned;
    }
    // Workspace state holds durable history/facts/truth: never application
    // bytes, so default uninstall preserves it.
    if identity.starts_with("workspaces/") {
        return RetentionClass::History;
    }
    match role {
        StateRole::Cache => RetentionClass::Cache,
        StateRole::History => RetentionClass::History,
        StateRole::Evidence => RetentionClass::Evidence,
        StateRole::Runtime => {
            if identity.starts_with("install/") || identity.starts_with("state/") {
                RetentionClass::Configuration
            } else {
                RetentionClass::Application
            }
        }
        StateRole::Pinned => RetentionClass::Pinned,
    }
}

/// Build the uninstall plan for a scope. Returns (plan, retained): every
/// item either has a planned removal or an explicit retention entry.
pub fn uninstall_plan(base: &Path, scope: UninstallScope) -> (ActionPlan, Vec<RetainedItem>) {
    let pins = crate::pins::load_pins(base)
        .map(|s| s.pins)
        .unwrap_or_default();
    let mut plan_items = Vec::new();
    let mut retained = Vec::new();
    for item in classify_tree(base) {
        // Pins always win: explicit protection, explicit retention entry.
        let pin_ids = crate::pins::pin_identities_for_state_identity(&item.identity);
        if pin_ids.iter().any(|p| pins.contains(p)) {
            retained.push(RetainedItem {
                identity: item.identity.clone(),
                class: RetentionClass::Pinned,
                reason: "explicitly pinned; use unpin to release".to_string(),
            });
            continue;
        }
        let class = retention_class(&item.identity, item.role);
        let remove = match (scope, class, item.disposal) {
            (_, RetentionClass::Pinned, _) => false,
            (UninstallScope::AppOnly, RetentionClass::Application, _) => true,
            (UninstallScope::AppAndCache, RetentionClass::Application, _) => true,
            (UninstallScope::AppAndCache, RetentionClass::Cache, _) => true,
            (UninstallScope::Everything, _, Disposal::Keep) => false,
            (UninstallScope::Everything, _, _) => true,
            _ => false,
        };
        if remove {
            plan_items.push(PlannedItem::from_state(
                &item,
                &format!("uninstall scope {scope:?} removes {class:?}"),
                "removed by explicit uninstall request",
            ));
        } else {
            retained.push(RetainedItem {
                identity: item.identity.clone(),
                class,
                reason: match scope {
                    UninstallScope::AppOnly | UninstallScope::AppAndCache => {
                        "preserved by default; user truth outlives the application".to_string()
                    }
                    UninstallScope::Everything => {
                        "protected or required; explicit per-class removal only".to_string()
                    }
                },
            });
        }
    }
    // Active binary outside the base (managed bin shim): represent as an
    // item so PATH integration truth is visible.
    plan_items.sort_by(|a, b| a.identity.cmp(&b.identity));
    retained.sort_by(|a, b| a.identity.cmp(&b.identity));
    (ActionPlan::new("uninstall", plan_items), retained)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetainedItem {
    pub identity: String,
    pub class: RetentionClass,
    pub reason: String,
}

/// Apply one uninstall item (file/dir removal with containment).
pub fn apply_uninstall_item(base: &Path, item: &PlannedItem) -> Result<String, LifecycleError> {
    crate::clean::apply_clean_item(base, item)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_uninstall_preserves_truth() {
        let base = tempfile::tempdir().unwrap();
        let b = base.path();
        std::fs::create_dir_all(b.join("bin")).unwrap();
        std::fs::write(b.join("bin").join("omen.exe"), b"bin").unwrap();
        std::fs::create_dir_all(b.join("workspaces").join("ws_a")).unwrap();
        std::fs::write(
            b.join("workspaces").join("ws_a").join("state.sqlite"),
            b"db",
        )
        .unwrap();
        std::fs::create_dir_all(b.join("evidence")).unwrap();
        std::fs::write(b.join("evidence").join("proof.json"), b"proof").unwrap();
        std::fs::create_dir_all(b.join("cache")).unwrap();
        std::fs::write(b.join("cache").join("c.tmp"), b"c").unwrap();

        let (plan, retained) = uninstall_plan(b, UninstallScope::AppOnly);
        let removing: Vec<&str> = plan.items.iter().map(|i| i.identity.as_str()).collect();
        assert!(removing.iter().any(|i| i.contains("bin/omen.exe")));
        assert!(!removing.iter().any(|i| i.contains("state.sqlite")));
        assert!(!removing.iter().any(|i| i.contains("evidence")));
        assert!(!removing.iter().any(|i| i.contains("cache")));
        // Retained entries are explicit.
        assert!(retained.iter().any(|r| r.identity.contains("state.sqlite")));
        assert!(retained.iter().any(|r| r.identity.contains("proof.json")));
    }

    #[test]
    fn everything_scope_still_honors_keep_and_pins() {
        let base = tempfile::tempdir().unwrap();
        let b = base.path();
        std::fs::create_dir_all(b.join("evidence")).unwrap();
        std::fs::write(b.join("evidence").join("proof.json"), b"proof").unwrap();
        let (plan, _retained) = uninstall_plan(b, UninstallScope::Everything);
        let removing: Vec<&str> = plan.items.iter().map(|i| i.identity.as_str()).collect();
        assert!(removing.iter().any(|i| i.contains("proof.json")));
        // Install record is Keep: retained with explicit reason.
        std::fs::create_dir_all(b.join("install")).unwrap();
        std::fs::write(b.join("install").join("record.json"), b"{}").unwrap();
        let (_, retained2) = uninstall_plan(b, UninstallScope::Everything);
        assert!(retained2.iter().any(|r| r.identity.contains("record.json")));
    }
}
