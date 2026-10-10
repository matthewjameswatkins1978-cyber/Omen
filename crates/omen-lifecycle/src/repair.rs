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

/// Reprojection analysis (R2): is the conveyor projection consistent
/// with the authoritative record + active pointer?
enum ReprojectPlan {
    /// Everything agrees; nothing to do.
    Agreed,
    /// Record + pointer agree on `slot`, slot manifest exists, but the
    /// projection disagrees (or is absent): safe to re-project.
    Ready { slot: String },
    /// Disagreement the mechanism must not guess through.
    Refused(String),
}

fn state_reproject_plan(base: &Path) -> ReprojectPlan {
    // The install record is the authority root: without one, pointer
    // repair owns the field (repoint-active) and reproject stays silent
    // so previously-clean plans are unchanged.
    let record = match crate::install::load_install_record(base).ok().flatten() {
        Some(r) => r,
        None => return ReprojectPlan::Agreed,
    };
    let Some(slot) = record.active_slot.clone() else {
        return ReprojectPlan::Refused("install record names no active slot".to_string());
    };
    let pointer = match crate::update::read_active_pointer(base) {
        Some(p) => p,
        None => {
            return ReprojectPlan::Refused("active pointer absent; use repoint flow".to_string());
        }
    };
    if pointer.active_slot != slot {
        return ReprojectPlan::Refused(format!(
            "record slot {slot} != pointer slot {}; refusing to guess",
            pointer.active_slot
        ));
    }
    // Full authoritative identity: the slot manifest must confirm the
    // record's version AND git SHA (not just the slot string). Any
    // authoritative disagreement refuses; only a stale projection is
    // safe to rewrite.
    let manifest: serde_json::Value =
        match std::fs::read(base.join("versions").join(&slot).join("manifest.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
        {
            Some(m) => m,
            None => {
                return ReprojectPlan::Refused(format!(
                    "slot {slot} manifest absent or unreadable; reinstall, not reproject"
                ));
            }
        };
    let m_version = manifest.get("preview_version").and_then(|v| v.as_str());
    let m_sha = manifest.get("git_sha").and_then(|v| v.as_str());
    if m_version != Some(record.version.as_str()) || m_sha != Some(record.git_sha.as_str()) {
        return ReprojectPlan::Refused(format!(
            "record ({} {}) != slot manifest ({} {}); authoritative disagreement",
            record.version,
            record.git_sha.get(..7).unwrap_or(&record.git_sha),
            m_version.unwrap_or("?"),
            m_sha.and_then(|s| s.get(..7)).unwrap_or("?")
        ));
    }
    let projected: Option<serde_json::Value> =
        std::fs::read(base.join("state").join("installed.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
    let agrees = projected.as_ref().is_some_and(|v| {
        v.get("active_slot").and_then(|s| s.as_str()) == Some(slot.as_str())
            && v.pointer("/active/preview_version")
                .and_then(|s| s.as_str())
                == Some(record.version.as_str())
            && v.pointer("/active/git_sha").and_then(|s| s.as_str())
                == Some(record.git_sha.as_str())
    });
    if agrees {
        ReprojectPlan::Agreed
    } else {
        ReprojectPlan::Ready { slot }
    }
}

/// Slots eligible for retirement: present under versions/, carrying a
/// parseable Omen slot manifest, and neither the active nor the previous
/// slot. Active + previous are rollback material and never retireable.
fn retireable_slots(base: &Path) -> Vec<String> {
    let record = match crate::install::load_install_record(base).ok().flatten() {
        Some(r) => r,
        None => return Vec::new(),
    };
    let keep: Vec<&str> = [
        record.active_slot.as_deref(),
        record.previous_slot.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(base.join("versions")) else {
        return out;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if keep.contains(&name.as_str()) {
            continue;
        }
        // Omen-managed proof: a parseable slot manifest. Anything else
        // (foreign dirs, half-written staging) stays for human review.
        let manifest_ok = std::fs::read(e.path().join("manifest.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .is_some_and(|v| {
                v.get("preview_version").and_then(|s| s.as_str()).is_some()
                    && v.get("git_sha").and_then(|s| s.as_str()).is_some()
            });
        if manifest_ok {
            out.push(name);
        }
    }
    out.sort();
    out
}

/// Retire one superseded slot. Removes only the slot directory; a locked
/// (running) slot fails loudly and stays for human review.
fn apply_slot_retire(base: &Path, slot: &str) -> Result<String, LifecycleError> {
    // Re-verify eligibility at apply time (plan never equals apply).
    if !retireable_slots(base).iter().any(|s| s == slot) {
        return Err(LifecycleError::Refused(format!(
            "slot {slot} is not retireable now (became active/previous or unmanaged)"
        )));
    }
    match std::fs::remove_dir_all(base.join("versions").join(slot)) {
        Ok(()) => Ok(format!("retired superseded slot {slot}")),
        Err(e) => Err(LifecycleError::Io(format!(
            "slot {slot} could not be removed (active or locked? report, not delete): {e}"
        ))),
    }
}

/// Rewrite the conveyor projection from the slot manifest. Merges into
/// the existing installed.json value (history keys preserved); sets
/// `active_slot` + `active` from the authoritative slot manifest.
fn apply_state_reproject(base: &Path, slot: &str) -> Result<String, LifecycleError> {
    // Re-verify the plan at apply time (plan never equals apply).
    match state_reproject_plan(base) {
        ReprojectPlan::Ready { slot: s } if s == slot => {}
        ReprojectPlan::Agreed => return Ok("projection already agrees".to_string()),
        _ => {
            return Err(LifecycleError::Refused(
                "reproject preconditions changed; re-plan".to_string(),
            ));
        }
    }
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(base.join("versions").join(slot).join("manifest.json"))
            .map_err(|e| LifecycleError::Io(e.to_string()))?,
    )
    .map_err(|e| LifecycleError::Manifest(e.to_string()))?;
    let path = base.join("state").join("installed.json");
    let mut current: serde_json::Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::json!({}));
    current["active_slot"] = serde_json::Value::String(slot.to_string());
    current["active"] = manifest;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&current).unwrap())
        .map_err(|e| LifecycleError::Io(e.to_string()))?;
    std::fs::rename(&tmp, &path).map_err(|e| LifecycleError::Io(e.to_string()))?;
    Ok(format!("projection re-written from slot {slot}"))
}

/// Remove one verified-stale entry from the registry User PATH (Windows).
/// Files are untouched; only the PATH value changes.
#[cfg(windows)]
fn clean_path_entry(entry: &str) -> Result<String, LifecycleError> {
    let current = crate::path_audit::read_registry_user_path()
        .ok_or_else(|| LifecycleError::Io("registry User PATH unreadable".to_string()))?;
    let next = crate::path_audit::entries_after_clean(&current, &[entry.to_string()]);
    if next.len() == current.len() {
        return Ok(format!("already absent from registry PATH: {entry}"));
    }
    crate::path_audit::write_registry_user_path(&next).map_err(LifecycleError::Io)?;
    Ok(format!(
        "removed {entry} from user PATH (takes effect in new shells)"
    ))
}

#[cfg(not(windows))]
fn clean_path_entry(entry: &str) -> Result<String, LifecycleError> {
    Ok(format!(
        "remove {entry} from PATH in your shell profile (automatic cleanup is Windows-only)"
    ))
}

/// Collapse duplicate registry PATH entries (case-insensitive), keeping
/// first occurrences. Re-verified at apply: no-op when already single.
#[cfg(windows)]
fn dedupe_path() -> Result<String, LifecycleError> {
    let current = crate::path_audit::read_registry_user_path()
        .ok_or_else(|| LifecycleError::Io("registry User PATH unreadable".to_string()))?;
    let next = crate::path_audit::entries_after_dedupe(&current);
    if next.len() == current.len() {
        return Ok("registry PATH already duplicate-free".to_string());
    }
    crate::path_audit::write_registry_user_path(&next).map_err(LifecycleError::Io)?;
    Ok(format!(
        "collapsed {} duplicate PATH entries (takes effect in new shells)",
        current.len() - next.len()
    ))
}

#[cfg(not(windows))]
fn dedupe_path() -> Result<String, LifecycleError> {
    Ok(
        "collapse duplicate PATH entries in your shell profile (automatic cleanup is Windows-only)"
            .to_string(),
    )
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

    // PATH shadowing (R1): remove verified-stale build dirs that resolve
    // an omen executable. Entries come from the doctor finding detail
    // (machine list) — never from a fresh env scan — so planning is
    // deterministic under parallel tests. Each entry is re-verified
    // (stale pattern + omen exe present) before planning; anything
    // unrecognized stays a doctor warning for human review.
    if has("path.shadowed") == Some(Status::Warn) {
        let exe = if cfg!(windows) { "omen.exe" } else { "omen" };
        let mut listed = Vec::new();
        if let Some(f) = report.findings.iter().find(|f| f.id == "path.shadowed")
            && let Some(detail) = f.detail.as_deref()
        {
            for e in detail.split(';').map(str::trim).filter(|e| !e.is_empty()) {
                if crate::path_audit::is_stale_build_dir(e)
                    && std::path::Path::new(e).join(exe).is_file()
                    && !listed.contains(&e.to_string())
                {
                    listed.push(e.to_string());
                }
            }
        }
        for e in listed {
            items.push(PlannedItem {
                identity: format!("path:clean:{e}"),
                role: "runtime".to_string(),
                reason: "verified-stale build dir shadows omen on PATH".to_string(),
                size_bytes: None,
                consequence: format!("removes {e} from user PATH; files untouched"),
                protected_at_plan: false,
                fingerprint: None,
            });
        }
    }

    // Canonical-bin duplicates: collapse to a single entry (safe:
    // identical value, first occurrence kept).
    if has("path.canonical_single") == Some(Status::Warn) {
        items.push(PlannedItem {
            identity: "path:dedupe".to_string(),
            role: "runtime".to_string(),
            reason: "canonical bin listed more than once".to_string(),
            size_bytes: None,
            consequence: "collapses duplicate canonical entries; order preserved".to_string(),
            protected_at_plan: false,
            fingerprint: None,
        });
    }
    // Unknown-ownership Omen executables: reported for human review only.
    // Unknown ownership means refusal, never automatic deletion.
    if has("path.unknown_omen") == Some(Status::Warn) {
        refused.push(RefusedRepair {
            id: "path:unknown-omen".to_string(),
            reason: "non-managed dir resolves an omen executable; human review required"
                .to_string(),
        });
    }

    // Record/projection agreement (R2): when the install record and the
    // active pointer agree on slot S, the slot manifest exists, but the
    // conveyor projection (state/installed.json) disagrees, re-project
    // from the authoritative sources. Anything ambiguous is refused.
    match state_reproject_plan(base) {
        ReprojectPlan::Ready { slot } => items.push(PlannedItem {
            identity: format!("state:reproject:{slot}"),
            role: "runtime".to_string(),
            reason: "conveyor projection disagrees with record + active pointer".to_string(),
            size_bytes: None,
            consequence: format!(
                "rewrites state/installed.json projection from slot {slot} manifest"
            ),
            protected_at_plan: false,
            fingerprint: None,
        }),
        ReprojectPlan::Agreed => {}
        ReprojectPlan::Refused(reason) => refused.push(RefusedRepair {
            id: "state:reproject".to_string(),
            reason,
        }),
    }

    // One installed build (review §3): retire version slots that are
    // neither active nor previous. Only Omen-managed slots (parseable
    // manifest.json) are planned; anything else stays for human review.
    // Rollback material (active + previous) is always preserved.
    for slot in retireable_slots(base) {
        items.push(PlannedItem {
            identity: format!("versions:retire:{slot}"),
            role: "runtime".to_string(),
            reason: "superseded install slot; single-build policy".to_string(),
            size_bytes: None,
            consequence: format!("removes versions/{slot} (rollback keeps active + previous)"),
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
    if let Some(entry) = item.identity.strip_prefix("path:clean:") {
        // Ownership proof at apply time: verified-stale pattern AND an
        // omen executable present AND the binary proves Omen identity
        // via its version banner AND the entry is currently registered.
        // Hand-written plans for anything else are refused. Files are
        // never deleted — only the PATH value changes.
        if !crate::path_audit::is_stale_build_dir(entry) {
            return Err(LifecycleError::Refused(format!(
                "path:clean refused for non-stale entry: {entry}"
            )));
        }
        let dir = std::path::Path::new(entry);
        let exe = dir.join(if cfg!(windows) { "omen.exe" } else { "omen" });
        if !exe.is_file() {
            return Err(LifecycleError::Refused(format!(
                "path:clean refused: no omen executable in {entry}"
            )));
        }
        if !crate::path_audit::omen_owned_exe_dir(dir) {
            return Err(LifecycleError::Refused(format!(
                "path:clean refused: {entry} does not resolve an Omen-owned executable"
            )));
        }
        return clean_path_entry(entry);
    }
    if item.identity == "path:dedupe" {
        return dedupe_path();
    }
    if let Some(slot) = item.identity.strip_prefix("versions:retire:") {
        if slot.contains('/') || slot.contains('\\') || slot.contains("..") {
            return Err(LifecycleError::Refused("repair slot escape".to_string()));
        }
        return apply_slot_retire(base, slot);
    }
    if let Some(slot) = item.identity.strip_prefix("state:reproject:") {
        if slot.contains('/') || slot.contains('\\') || slot.contains("..") {
            return Err(LifecycleError::Refused("repair slot escape".to_string()));
        }
        return apply_state_reproject(base, slot);
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
    // via the registry (no setx 1024-char truncation). Bounded: one
    // entry, no duplicates. Reads REGISTRY truth, never the inheriting
    // process copy.
    let bin = base.join("bin");
    let bin_s = bin.to_string_lossy().to_string();
    let current = crate::path_audit::read_registry_user_path()
        .ok_or_else(|| LifecycleError::Io("registry User PATH unreadable".to_string()))?;
    let next = crate::path_audit::entries_after_register(&current, &bin_s);
    if next.len() == current.len() {
        return Ok("already on PATH".to_string());
    }
    crate::path_audit::write_registry_user_path(&next).map_err(LifecycleError::Io)?;
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
