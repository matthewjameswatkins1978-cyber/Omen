//! `omen repair`: explicit, planned, evidenced repair (H items 37-39).
//!
//! Flow: inspect -> proposed repair plan -> explicit apply -> result
//! evidence. Repairs never silently delete history/evidence. When recovery
//! would require guessing (unknown corruption, ambiguous ownership, missing
//! provenance, uncertain version): REFUSE.

use crate::doctor::DoctorReport;
use crate::error::LifecycleError;
use crate::plan::{ActionPlan, PlannedItem};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairReport {
    pub plan_id: String,
    pub completed: Vec<RepairStep>,
    pub refused: Vec<RefusedRepair>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairStep {
    pub id: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefusedRepair {
    pub id: String,
    pub reason: String,
}

/// Inspect a doctor report + live state and propose repairs. Returns
/// (plan, refusals): items that cannot be determined go to refusals, never
/// into the plan as guesses.
pub fn repair_plan(base: &Path, report: &DoctorReport) -> (ActionPlan, Vec<RefusedRepair>) {
    let mut items = Vec::new();
    let mut refused = Vec::new();
    let has = |id: &str| {
        report
            .findings
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.status)
    };
    use crate::doctor::Status;

    // Missing managed directories: recreate (safe, deterministic).
    for dir in ["workspaces", "evidence", "history", "cache", "tmp", "pins"] {
        if !base.join(dir).exists() {
            items.push(PlannedItem {
                identity: format!("mkdir:{dir}"),
                role: "runtime".to_string(),
                reason: "managed directory missing".to_string(),
                size_bytes: None,
                consequence: format!("directory {dir} is created; nothing else changes"),
                protected_at_plan: false,
                fingerprint: None,
            });
        }
    }

    // Broken active pointer with exactly one slot present: re-point (safe).
    // Zero or many slots with a broken pointer: ambiguous -> refuse.
    if has("slot.active") == Some(Status::Fail) {
        let versions = base.join("versions");
        let exe = if cfg!(windows) { "omen.exe" } else { "omen" };
        let mut slots = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&versions) {
            for e in rd.flatten() {
                if e.path().join(exe).is_file() {
                    slots.push(e.file_name().to_string_lossy().to_string());
                }
            }
        }
        slots.sort();
        match slots.len() {
            1 => items.push(PlannedItem {
                identity: format!("repoint-active:{}", slots[0]),
                role: "runtime".to_string(),
                reason: "active slot pointer broken; exactly one healthy slot".to_string(),
                size_bytes: None,
                consequence: format!("active pointer set to {}", slots[0]),
                protected_at_plan: false,
                fingerprint: None,
            }),
            0 => refused.push(RefusedRepair {
                id: "repoint-active".to_string(),
                reason: "active pointer broken and no slot present; refusing to guess".to_string(),
            }),
            _ => refused.push(RefusedRepair {
                id: "repoint-active".to_string(),
                reason: format!(
                    "active pointer broken and {} slots present; ambiguous",
                    slots.len()
                ),
            }),
        }
    }

    // PATH integration: re-register is platform work done by apply with
    // explicit consent (the plan records intent, apply performs it).
    if has("path.integrated") == Some(Status::Warn) {
        items.push(PlannedItem {
            identity: "path:register".to_string(),
            role: "runtime".to_string(),
            reason: "running executable not on PATH".to_string(),
            size_bytes: None,
            consequence: "user PATH registration for the active bin dir (explicit apply = consent)"
                .to_string(),
            protected_at_plan: false,
            fingerprint: None,
        });
    }

    // Interrupted staging state: recoverable only when the transaction
    // record names the stage; otherwise refuse.
    let restarts = crate::update::classify_update_restart(base);
    for r in restarts {
        match r {
            crate::update::UpdateRestart::PreviousStillActive { tx_id } => {
                items.push(PlannedItem {
                    identity: format!("quarantine-clean:{tx_id}"),
                    role: "cache".to_string(),
                    reason: "failed update left quarantined candidate".to_string(),
                    size_bytes: None,
                    consequence: "quarantine dir for the failed transaction is removed".to_string(),
                    protected_at_plan: false,
                    fingerprint: None,
                });
            }
            crate::update::UpdateRestart::RecoveryRequired { tx_id, reason } => {
                refused.push(RefusedRepair {
                    id: format!("update-recover:{tx_id}"),
                    reason,
                });
            }
            other => refused.push(RefusedRepair {
                id: "update-recover".to_string(),
                reason: format!("interrupted update state {other:?} needs explicit update/rollback flow, not repair"),
            }),
        }
    }

    // Corrupted disposable cache: rebuild (safe).
    if has("cache.corrupt") == Some(Status::Fail) {
        items.push(PlannedItem {
            identity: "cache:rebuild".to_string(),
            role: "cache".to_string(),
            reason: "disposable cache corrupt; canonical truth exists elsewhere".to_string(),
            size_bytes: None,
            consequence: "cache directory is cleared and recreated".to_string(),
            protected_at_plan: false,
            fingerprint: None,
        });
    }

    // History/evidence corruption is never auto-planned: preserve + expose.
    // (The classifier for corruption kinds lives in doctor findings
    // `history.corrupt` / `evidence.corrupt`, which repair deliberately
    // refuses to act on.)
    if has("history.corrupt") == Some(Status::Fail) {
        refused.push(RefusedRepair {
            id: "history.corrupt".to_string(),
            reason: "history is not cache: preserve damaged bytes, export/recover explicitly"
                .to_string(),
        });
    }
    if has("evidence.corrupt") == Some(Status::Fail) {
        refused.push(RefusedRepair {
            id: "evidence.corrupt".to_string(),
            reason: "digest mismatch is integrity failure: never bless with a new identity"
                .to_string(),
        });
    }

    (ActionPlan::new("repair", items), refused)
}

/// Apply one repair item. Returns a detail string for evidence.
pub fn apply_repair_item(base: &Path, item: &PlannedItem) -> Result<String, LifecycleError> {
    if let Some(dir) = item.identity.strip_prefix("mkdir:") {
        if dir.contains('/') || dir.contains('\\') || dir.contains("..") {
            return Err(LifecycleError::Refused("repair mkdir escape".to_string()));
        }
        std::fs::create_dir_all(base.join(dir)).map_err(|e| LifecycleError::Io(e.to_string()))?;
        return Ok(format!("created {dir}"));
    }
    if let Some(slot) = item.identity.strip_prefix("repoint-active:") {
        if slot.contains('/') || slot.contains('\\') || slot.contains("..") {
            return Err(LifecycleError::Refused("repair slot escape".to_string()));
        }
        crate::update::write_active_pointer(base, slot)?;
        return Ok(format!("active pointer -> {slot}"));
    }
    if item.identity == "path:register" {
        return register_path(base);
    }
    if let Some(tx) = item.identity.strip_prefix("quarantine-clean:") {
        if tx.contains('/') || tx.contains('\\') || tx.contains("..") {
            return Err(LifecycleError::Refused("repair tx escape".to_string()));
        }
        let q = base.join("update").join("quarantine").join(tx);
        if q.exists() {
            std::fs::remove_dir_all(&q).map_err(|e| LifecycleError::Io(e.to_string()))?;
            return Ok(format!("quarantine {tx} removed"));
        }
        return Ok("already gone".to_string());
    }
    if item.identity == "cache:rebuild" {
        let c = base.join("cache");
        if c.exists() {
            std::fs::remove_dir_all(&c).map_err(|e| LifecycleError::Io(e.to_string()))?;
        }
        std::fs::create_dir_all(&c).map_err(|e| LifecycleError::Io(e.to_string()))?;
        return Ok("cache rebuilt".to_string());
    }
    Err(LifecycleError::Refused(format!(
        "unknown repair item: {}",
        item.identity
    )))
}

#[cfg(windows)]
fn register_path(base: &Path) -> Result<String, LifecycleError> {
    // Explicit apply = consent. Adds the active bin dir to the USER PATH
    // via setx when missing. Bounded: one entry, no duplicates.
    let bin = base.join("bin");
    let current = std::env::var_os("PATH")
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    if current
        .split(';')
        .any(|d| d.eq_ignore_ascii_case(&bin.to_string_lossy()))
    {
        return Ok("already on PATH".to_string());
    }
    let status = std::process::Command::new("setx")
        .args(["PATH", &format!("{current};{}", bin.display())])
        .output()
        .map_err(|e| LifecycleError::Io(format!("setx failed: {e}")))?;
    if !status.status.success() {
        return Err(LifecycleError::Io("setx PATH update failed".to_string()));
    }
    Ok(format!(
        "registered {} on user PATH (takes effect in new shells)",
        bin.display()
    ))
}

#[cfg(not(windows))]
fn register_path(base: &Path) -> Result<String, LifecycleError> {
    let bin = base.join("bin");
    Ok(format!(
        "add {} to PATH in your shell profile (automatic registration is Windows-only)",
        bin.display()
    ))
}
