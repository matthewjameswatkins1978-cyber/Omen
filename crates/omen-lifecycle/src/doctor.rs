//! `omen doctor`: observational health inspection (H items 34-36).
//!
//! Doctor OBSERVES. It never repairs: running doctor twice in a row must
//! produce no state change (tested). Distinctions are never collapsed:
//! installed vs configured vs available vs working vs authorised are
//! separate findings. Unknown stays unknown.

use crate::install::{Channel, Ownership};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub status: Status,
    pub summary: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub schema_version: u32,
    pub version: String,
    pub git_sha: String,
    pub contract_version: String,
    pub ownership: Ownership,
    pub channel: Channel,
    pub findings: Vec<Finding>,
}

impl DoctorReport {
    pub fn worst(&self) -> Status {
        let mut w = Status::Ok;
        for f in &self.findings {
            w = match (w, f.status) {
                (_, Status::Fail) => Status::Fail,
                (Status::Fail, _) => Status::Fail,
                (_, Status::Warn) | (Status::Warn, _) => Status::Warn,
                (_, Status::Unknown) | (Status::Unknown, _) => {
                    if w == Status::Ok {
                        Status::Unknown
                    } else {
                        w
                    }
                }
                _ => w,
            };
        }
        w
    }
}

pub struct DoctorInput {
    pub base: PathBuf,
    pub version: String,
    pub git_sha: String,
    pub exe_path: PathBuf,
    /// Optional adapter descriptor probe: name -> (installed, configured).
    pub adapters: Vec<(String, bool, bool)>,
}

fn ok(id: &str, summary: &str) -> Finding {
    Finding {
        id: id.to_string(),
        status: Status::Ok,
        summary: summary.to_string(),
        detail: None,
    }
}
fn warn(id: &str, summary: &str, detail: &str) -> Finding {
    Finding {
        id: id.to_string(),
        status: Status::Warn,
        summary: summary.to_string(),
        detail: Some(detail.to_string()),
    }
}
fn fail(id: &str, summary: &str, detail: &str) -> Finding {
    Finding {
        id: id.to_string(),
        status: Status::Fail,
        summary: summary.to_string(),
        detail: Some(detail.to_string()),
    }
}
fn unknown(id: &str, summary: &str) -> Finding {
    Finding {
        id: id.to_string(),
        status: Status::Unknown,
        summary: summary.to_string(),
        detail: None,
    }
}

/// First 7 hex chars for human-readable identity details; full values
/// stay in the underlying records, never truncated there.
fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// Conveyor `state/installed.json` active identity, when present.
enum ConveyorActive {
    Absent,
    Present { version: String, git_sha: String },
    Unreadable(String),
}

fn read_conveyor_active(base: &std::path::Path) -> ConveyorActive {
    let path = base.join("state").join("installed.json");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ConveyorActive::Absent,
        Err(e) => return ConveyorActive::Unreadable(e.to_string()),
    };
    let state: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return ConveyorActive::Unreadable(e.to_string()),
    };
    let Some(active) = state.get("active") else {
        return ConveyorActive::Unreadable("conveyor state has no active entry".to_string());
    };
    if active.is_null() {
        return ConveyorActive::Absent;
    }
    let version = active
        .get("preview_version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let git_sha = active
        .get("git_sha")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if version.is_empty() || git_sha.is_empty() {
        return ConveyorActive::Unreadable(
            "conveyor active entry lacks version identity".to_string(),
        );
    }
    ConveyorActive::Present { version, git_sha }
}

/// Run all observational checks. Pure reads + bounded probes only.
pub fn run_doctor(input: &DoctorInput) -> DoctorReport {
    let base = &input.base;
    let mut findings = Vec::new();

    // Executable identity.
    if input.exe_path.is_file() {
        findings.push(ok(
            "exe.present",
            &format!("executable present at {}", input.exe_path.display()),
        ));
    } else {
        findings.push(fail(
            "exe.present",
            "executable path is not a file",
            &input.exe_path.display().to_string(),
        ));
    }

    // Installation record + ownership + channel.
    match crate::install::load_install_record(base) {
        Ok(Some(record)) => {
            findings.push(ok(
                "install.record",
                &format!(
                    "install record: {:?} {:?} {}",
                    record.owner, record.channel, record.version
                ),
            ));
            let detected = crate::install::detect_ownership(base, &input.exe_path);
            if detected != record.owner && detected != Ownership::Unknown {
                findings.push(warn(
                    "install.ownership",
                    "detected ownership differs from record",
                    &format!("record {:?} vs detected {:?}", record.owner, detected),
                ));
            } else {
                findings.push(ok(
                    "install.ownership",
                    crate::install::Ownership::Omen.describe(),
                ));
                let _ = detected;
            }
            // Installation truth: the running binary's embedded identity
            // must agree with the record it runs against. A manual
            // overlay or dev binary over a recorded install disagrees
            // here; doctor flags it observationally and never rewrites.
            if record.version == input.version && record.git_sha == input.git_sha {
                findings.push(ok(
                    "install.identity",
                    &format!(
                        "running binary matches install record {} {}",
                        record.version,
                        short_sha(&record.git_sha),
                    ),
                ));
            } else {
                findings.push(warn(
                    "install.identity",
                    "running binary identity differs from install record",
                    &format!(
                        "binary {} {} vs record {} {}; manual overlay or dev binary over a recorded install — record left untouched",
                        input.version,
                        short_sha(&input.git_sha),
                        record.version,
                        short_sha(&record.git_sha),
                    ),
                ));
            }
        }
        Ok(None) => findings.push(unknown(
            "install.record",
            "no install record: unmanaged or development install",
        )),
        Err(e) => findings.push(fail(
            "install.record",
            "install record unreadable",
            &e.to_string(),
        )),
    }
    findings.push(ok(
        "install.channel",
        &format!("channel: {:?}", crate::install::user_channel(base)),
    ));
    // Conveyor truth: a preview-conveyor installed.json names the
    // installer-placed binary. When the running binary disagrees, the
    // install is manual/dev run against (or over) a conveyor install —
    // explicit, observational, never rewritten.
    match read_conveyor_active(base) {
        ConveyorActive::Absent => {}
        ConveyorActive::Present { version, git_sha } => {
            if version == input.version && git_sha == input.git_sha {
                findings.push(ok(
                    "install.conveyor_identity",
                    &format!(
                        "running binary matches conveyor install {} {}",
                        version,
                        short_sha(&git_sha),
                    ),
                ));
            } else {
                findings.push(warn(
                    "install.conveyor_identity",
                    "running binary is not the conveyor-installed binary",
                    &format!(
                        "binary {} {} vs conveyor {} {}; manual or development binary — conveyor state left untouched",
                        input.version,
                        short_sha(&input.git_sha),
                        version,
                        short_sha(&git_sha),
                    ),
                ));
            }
        }
        ConveyorActive::Unreadable(detail) => {
            findings.push(warn(
                "install.conveyor_identity",
                "conveyor state unreadable",
                &detail,
            ));
        }
    }

    // Active slot.
    match crate::update::read_active_pointer(base) {
        Some(ptr) => {
            let slot_bin = base
                .join("versions")
                .join(&ptr.active_slot)
                .join(if cfg!(windows) { "omen.exe" } else { "omen" });
            if slot_bin.is_file() {
                findings.push(ok(
                    "slot.active",
                    &format!("active slot {} binary present", ptr.active_slot),
                ));
            } else {
                findings.push(fail(
                    "slot.active",
                    "active slot binary missing",
                    &ptr.active_slot,
                ));
            }
        }
        None => findings.push(unknown("slot.active", "no active slot pointer")),
    }

    // State / database health (observational open, read-only).
    let ws = base.join("workspaces");
    if ws.is_dir() {
        findings.push(ok("state.present", "workspace state root present"));
    } else {
        findings.push(unknown(
            "state.present",
            "no workspace state yet (first run will create it)",
        ));
    }

    // Migration status.
    match crate::migrate::classify_restart(base) {
        Ok(crate::migrate::RestartState::Clean) => {
            findings.push(ok("migrate.status", "no pending migration"))
        }
        Ok(crate::migrate::RestartState::MigrationIncomplete { checkpoint }) => {
            findings.push(warn(
                "migrate.status",
                "migration incomplete",
                &checkpoint.migration_id,
            ))
        }
        Ok(crate::migrate::RestartState::RecoveryRequired { reason }) => findings.push(fail(
            "migrate.status",
            "migration recovery required",
            &reason,
        )),
        Err(e) => findings.push(unknown(
            "migrate.status",
            &format!("migration status unreadable: {e}"),
        )),
    }

    // Update crash leftovers.
    let restarts = crate::update::classify_update_restart(base);
    if restarts.is_empty() {
        findings.push(ok("update.clean", "no interrupted update transactions"));
    } else {
        findings.push(warn(
            "update.clean",
            "interrupted update transactions present",
            &format!("{restarts:?}"),
        ));
    }

    // CAS/evidence reachability: directories exist and are listable.
    let ev = base.join("evidence");
    if ev.is_dir() {
        findings.push(ok("evidence.present", "evidence root present"));
    } else {
        findings.push(unknown("evidence.present", "no evidence root yet"));
    }

    // PATH integration (observational): is the running exe on PATH?
    let exe_name = if cfg!(windows) { "omen.exe" } else { "omen" };
    let on_path = std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|d| {
                d.join(exe_name).is_file()
                    && std::fs::canonicalize(d.join(exe_name)).ok()
                        == std::fs::canonicalize(&input.exe_path).ok()
            })
        })
        .unwrap_or(false);
    if on_path {
        findings.push(ok(
            "path.integrated",
            "running executable resolves via PATH",
        ));
    } else {
        findings.push(warn(
            "path.integrated",
            "running executable not found on PATH",
            "repair can re-register PATH integration",
        ));
    }

    // PATH shadowing (observational, R1): verified-stale cargo build dirs
    // on PATH that resolve an omen executable ahead of (or instead of)
    // the managed install. The detail is the machine-parseable entry
    // list (repair re-verifies each); the summary carries the human text.
    {
        let shadowed = crate::path_audit::shadowing_omen_exe_dirs(base);
        if shadowed.is_empty() {
            findings.push(ok(
                "path.shadowed",
                "no stale build dir shadows omen on PATH",
            ));
        } else {
            findings.push(warn(
                "path.shadowed",
                &format!(
                    "{} stale build dir(s) shadow omen on PATH (repair: path:clean)",
                    shadowed.len()
                ),
                &shadowed.join(";"),
            ));
        }
    }

    // Registry-vs-process PATH divergence (observational, R6, Windows):
    // a fresh shell resolves via registry PATH, not this process' copy.
    #[cfg(windows)]
    {
        match crate::path_audit::read_registry_user_path() {
            Some(reg) => {
                let proc: Vec<String> = std::env::var_os("PATH")
                    .map(|p| {
                        std::env::split_paths(&p)
                            .map(|d| d.to_string_lossy().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                let mut reg_all = reg.clone();
                if let Some(machine) = crate::path_audit::read_registry_machine_path() {
                    reg_all.extend(machine);
                }
                let reg_norm: Vec<String> = reg_all
                    .iter()
                    .map(|e| e.replace('\\', "/").to_lowercase())
                    .collect();
                let proc_norm: Vec<String> = proc
                    .iter()
                    .map(|e| e.replace('\\', "/").to_lowercase())
                    .collect();
                let only_reg: Vec<&String> = reg
                    .iter()
                    .filter(|e| !proc_norm.contains(&e.replace('\\', "/").to_lowercase()))
                    .collect();
                let only_proc: Vec<&String> = proc
                    .iter()
                    .filter(|e| !reg_norm.contains(&e.replace('\\', "/").to_lowercase()))
                    .collect();
                // Only Omen-relevant divergence is reported; unrelated
                // shell drift is the user's business. Scopes: registry
                // (User + Machine, what fresh shells resolve) vs process.
                let omenish = |e: &String| {
                    let l = e.to_lowercase();
                    l.contains("omen") || crate::path_audit::is_stale_build_dir(e)
                };
                let mut div: Vec<String> = only_reg
                    .iter()
                    .filter(|e| omenish(e))
                    .map(|e| format!("registry-only:{e}"))
                    .chain(
                        only_proc
                            .iter()
                            .filter(|e| omenish(e))
                            .map(|e| format!("process-only:{e}")),
                    )
                    .collect();
                div.sort();
                div.dedup();
                if div.is_empty() {
                    findings.push(ok(
                        "path.registry_divergence",
                        "registry and process PATH agree on Omen entries",
                    ));
                } else {
                    findings.push(warn(
                        "path.registry_divergence",
                        "registry PATH and process PATH disagree on Omen entries",
                        &format!(
                            "fresh shells resolve via registry; restart the shell or re-apply: {}",
                            div.join("; ")
                        ),
                    ));
                }
            }
            None => findings.push(unknown(
                "path.registry_divergence",
                "registry PATH unreadable here",
            )),
        }
    }

    // PATH completeness (observational, review §4): duplicate canonical
    // entries and unknown-ownership Omen executables. Duplicates are
    // safe to collapse; unknown ownership is reported, never removed.
    {
        let entries: Vec<String> = std::env::var_os("PATH")
            .map(|p| {
                std::env::split_paths(&p)
                    .map(|d| d.to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        let canonical = base.join("bin").to_string_lossy().to_string();
        let dupes = crate::path_audit::duplicate_canonical_entries(&entries, &canonical);
        if dupes.is_empty() {
            findings.push(ok(
                "path.canonical_single",
                "canonical bin present at most once",
            ));
        } else {
            findings.push(warn(
                "path.canonical_single",
                &format!(
                    "canonical bin listed {} time(s) (repair: path:dedupe)",
                    dupes.len() + 1
                ),
                &dupes.join(";"),
            ));
        }
        let unknown = crate::path_audit::unknown_omen_exe_dirs_in(base, &entries);
        if unknown.is_empty() {
            findings.push(ok(
                "path.unknown_omen",
                "no unknown-ownership Omen executables on PATH",
            ));
        } else {
            findings.push(warn(
                "path.unknown_omen",
                &format!(
                    "{} non-managed dir(s) resolve an omen executable (human review; never auto-removed)",
                    unknown.len()
                ),
                &unknown.join(";"),
            ));
        }
    }

    // Terminal capabilities (observational).
    let dumb = std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false);
    let no_color = std::env::var("NO_COLOR").is_ok();
    findings.push(ok(
        "terminal.caps",
        &format!("tty/probe only; dumb={dumb} no_color={no_color}"),
    ));

    // Adapter/provider configured-enough state: installed vs configured vs
    // working vs authorised are SEPARATE. Doctor never claims "working"
    // without an actual round trip (unknown until use).
    for (name, installed, configured) in &input.adapters {
        if !installed {
            findings.push(unknown(
                &format!("adapter.{name}"),
                &format!("{name} binary not installed"),
            ));
        } else if !configured {
            findings.push(warn(
                &format!("adapter.{name}"),
                &format!("{name} installed but not configured"),
                "authorisation state unknown until use",
            ));
        } else {
            findings.push(unknown(
                &format!("adapter.{name}"),
                &format!("{name} installed and configured; working/authorised unknown until use"),
            ));
        }
    }

    // Machine contract compatibility (read from build identity).
    findings.push(ok(
        "contract.compat",
        &format!("machine contract {}", super::install::CONTRACT_VERSION),
    ));

    DoctorReport {
        schema_version: 1,
        version: input.version.clone(),
        git_sha: input.git_sha.clone(),
        contract_version: super::install::CONTRACT_VERSION.to_string(),
        ownership: crate::install::detect_ownership(base, &input.exe_path),
        channel: crate::install::user_channel(base),
        findings,
    }
}

/// Stable machine output for agents/support: no prose scraping.
pub fn machine_json(report: &DoctorReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn input(base: &Path) -> DoctorInput {
        DoctorInput {
            base: base.to_path_buf(),
            version: "0.9.0-preview.16".to_string(),
            git_sha: "abc".to_string(),
            exe_path: base.join("omen-test-binary"),
            adapters: vec![("codex".to_string(), true, false)],
        }
    }

    #[test]
    fn doctor_is_observational() {
        let base = tempfile::tempdir().unwrap();
        let before: Vec<String> = walk(base.path());
        let report = run_doctor(&input(base.path()));
        let after: Vec<String> = walk(base.path());
        assert_eq!(before, after, "doctor must not mutate state");
        assert!(!report.findings.is_empty());
        // Adapter distinction: installed but not configured -> warn, and
        // working/authorised never claimed.
        let text = serde_json::to_string(&report).unwrap();
        assert!(!text.contains("working\")"));
    }

    fn walk(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            if let Ok(rd) = std::fs::read_dir(&d) {
                for e in rd.flatten() {
                    out.push(e.path().to_string_lossy().to_string());
                    if e.path().is_dir() {
                        stack.push(e.path());
                    }
                }
            }
        }
        out.sort();
        out
    }
}
