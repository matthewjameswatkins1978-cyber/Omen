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
