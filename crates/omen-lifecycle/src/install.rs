//! Install ownership, install record, and release channels (H items 14-22).
//!
//! Omen manages its own lifecycle only when Omen actually owns that
//! lifecycle. Ownership semantics:
//! - `Omen`: placed by the conveyor / self-update with exact provenance.
//! - `PackageManager`: owned by WinGet/Homebrew/etc — Omen reports, never
//!   seizes the binary behind the manager's back.
//! - `Development`: cargo/local build — never masquerades as managed.
//! - `Unknown`: self-update disabled/refused, never assumed.
//!
//! Channel selection (Stable/Preview) belongs to the USER installation
//! context. A workspace `Omen.toml` MUST NOT redefine it (hostile input).

use crate::error::LifecycleError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const INSTALL_RECORD_SCHEMA_VERSION: u32 = 1;
pub const CHANNEL_SCHEMA_VERSION: u32 = 1;
pub const CONTRACT_VERSION: &str = "0.8";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ownership {
    Omen,
    PackageManager,
    Development,
    Unknown,
}

impl Ownership {
    /// Whether `omen update` may mutate the active installation.
    pub fn self_update_allowed(&self) -> bool {
        matches!(self, Ownership::Omen)
    }

    pub fn describe(&self) -> &'static str {
        match self {
            Ownership::Omen => "installed by Omen (self-managed)",
            Ownership::PackageManager => "installed by an external package manager",
            Ownership::Development => "development/cargo build (unmanaged)",
            Ownership::Unknown => "unknown ownership (unmanaged)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Preview,
}

impl Channel {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "stable" => Some(Channel::Stable),
            "preview" => Some(Channel::Preview),
            _ => None,
        }
    }
}

/// Canonical managed-installation record. No secrets, ever.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallRecord {
    pub schema_version: u32,
    pub owner: Ownership,
    /// Package-manager name when owner is PackageManager.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_name: Option<String>,
    pub channel: Channel,
    pub active_slot: Option<String>,
    pub previous_slot: Option<String>,
    pub version: String,
    pub git_sha: String,
    /// SHA-256 of the final installed ARTIFACT bytes. Never a payload
    /// digest under this name: payload identity travels in `payload_sha256`.
    pub package_sha256: Option<String>,
    /// Deterministic payload/content identity of the installed package
    /// (embedded manifest `payload_sha256`, or the legacy v1 field value).
    /// Optional and additive: old records without it still parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_sha256: Option<String>,
    pub binary_sha256: Option<String>,
    pub installed_at: String,
    pub state_schema_version: u32,
}

impl InstallRecord {
    pub fn new(owner: Ownership, channel: Channel, version: &str, git_sha: &str) -> Self {
        Self {
            schema_version: INSTALL_RECORD_SCHEMA_VERSION,
            owner,
            owner_name: None,
            channel,
            active_slot: None,
            previous_slot: None,
            version: version.to_string(),
            git_sha: git_sha.to_string(),
            package_sha256: None,
            payload_sha256: None,
            binary_sha256: None,
            installed_at: chrono::Utc::now().to_rfc3339(),
            state_schema_version: crate::migrate::STATE_SCHEMA_VERSION,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChannelFile {
    schema_version: u32,
    channel: Channel,
}

pub fn install_record_path(base: &Path) -> PathBuf {
    base.join("install").join("record.json")
}

pub fn channel_path(base: &Path) -> PathBuf {
    base.join("install").join("channel.json")
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), LifecycleError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LifecycleError::Io(e.to_string()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| LifecycleError::Io(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| LifecycleError::Io(e.to_string()))?;
    Ok(())
}

pub fn load_install_record(base: &Path) -> Result<Option<InstallRecord>, LifecycleError> {
    let path = install_record_path(base);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).map_err(|e| LifecycleError::Io(e.to_string()))?;
    let record: InstallRecord =
        serde_json::from_slice(&bytes).map_err(|e| LifecycleError::Manifest(e.to_string()))?;
    if record.schema_version != INSTALL_RECORD_SCHEMA_VERSION {
        return Err(LifecycleError::Compat(format!(
            "install record schema {} unsupported",
            record.schema_version
        )));
    }
    Ok(Some(record))
}

pub fn save_install_record(base: &Path, record: &InstallRecord) -> Result<(), LifecycleError> {
    atomic_write(
        &install_record_path(base),
        &serde_json::to_vec_pretty(record).unwrap(),
    )
}

/// Read the user's channel selection. ONLY the user install context is
/// consulted: `install/channel.json`, then the install record. Workspace
/// configuration is deliberately never read here (H item 20/66).
pub fn user_channel(base: &Path) -> Channel {
    if let Ok(bytes) = std::fs::read(channel_path(base))
        && let Ok(cf) = serde_json::from_slice::<ChannelFile>(&bytes)
        && cf.schema_version == CHANNEL_SCHEMA_VERSION
    {
        return cf.channel;
    }
    if let Ok(Some(record)) = load_install_record(base) {
        return record.channel;
    }
    Channel::Preview
}

pub fn set_user_channel(base: &Path, channel: Channel) -> Result<(), LifecycleError> {
    atomic_write(
        &channel_path(base),
        &serde_json::to_vec_pretty(&ChannelFile {
            schema_version: CHANNEL_SCHEMA_VERSION,
            channel,
        })
        .unwrap(),
    )
}

/// Detect installation ownership for the running executable.
///
/// Precedence (first match wins):
/// 1. `OMEN_INSTALL_OWNER` explicit override (tests/support; values
///    omen|package-manager:<name>|development|unknown).
/// 2. Install record present + exe inside the managed base -> Omen
///    (or the record's declared owner).
/// 3. Exe path under a cargo target dir, or `OMEN_DEV=1` -> Development.
/// 4. Well-known package-manager prefixes -> PackageManager.
/// 5. Otherwise Unknown (self-update refused).
pub fn detect_ownership(base: &Path, exe_path: &Path) -> Ownership {
    if let Ok(raw) = std::env::var("OMEN_INSTALL_OWNER") {
        let v = raw.to_lowercase();
        if v == "omen" {
            return Ownership::Omen;
        }
        if v == "development" {
            return Ownership::Development;
        }
        if v == "unknown" {
            return Ownership::Unknown;
        }
        if let Some(name) = v.strip_prefix("package-manager:") {
            let _ = name;
            return Ownership::PackageManager;
        }
    }
    let exe_norm = exe_path.to_string_lossy().replace('\\', "/").to_lowercase();
    // Development: cargo target layouts never masquerade as managed.
    if exe_norm.contains("/target/debug/")
        || exe_norm.contains("/target/release/")
        || exe_norm.ends_with("/target/debug/omen")
        || exe_norm.ends_with("/target/release/omen")
        || exe_norm.ends_with("/target/debug/omen.exe")
        || exe_norm.ends_with("/target/release/omen.exe")
        || std::env::var("OMEN_DEV").is_ok()
    {
        return Ownership::Development;
    }
    if let Ok(Some(record)) = load_install_record(base) {
        // Record exists: trust its declared owner when the exe lives under
        // managed state, else the record is stale -> Unknown.
        let base_norm = base.to_string_lossy().replace('\\', "/").to_lowercase();
        if exe_norm.starts_with(&base_norm) {
            return record.owner;
        }
        return Ownership::Unknown;
    }
    // Well-known external-manager prefixes (represent the truth, do not
    // invent a universal mechanism).
    #[cfg(windows)]
    {
        if exe_norm.contains("program files") || exe_norm.contains("winget") {
            return Ownership::PackageManager;
        }
    }
    #[cfg(not(windows))]
    {
        if exe_norm.starts_with("/opt/homebrew/")
            || exe_norm.starts_with("/usr/local/bin/")
            || exe_norm.starts_with("/usr/bin/")
        {
            return Ownership::PackageManager;
        }
    }
    Ownership::Unknown
}

/// Resolve the effective ownership: record-aware detection. When no record
/// exists but detection says Omen-owned layout, still Unknown — ownership
/// requires the record (provenance), not just a path.
pub fn effective_ownership(base: &Path, exe_path: &Path) -> Ownership {
    detect_ownership(base, exe_path)
}

/// Package-manager guidance for update/uninstall when Omen must not act.
pub fn manager_guidance(owner_name: Option<&str>) -> String {
    match owner_name {
        Some(name) => format!("installed via {name}; update or uninstall using {name}"),
        None => "installed via an external package manager; update or uninstall using that manager"
            .to_string(),
    }
}

/// Scan workspace `Omen.toml` text for lifecycle-owned keys. Returns the
/// offending keys found. Workspaces may never set: channel, install owner,
/// update source, uninstall behavior, global retention, trusted provenance.
pub fn workspace_lifecycle_violations(toml_text: &str) -> Vec<String> {
    const FORBIDDEN: &[&str] = &[
        "channel",
        "install_owner",
        "update_source",
        "uninstall_behavior",
        "global_retention",
        "trusted_provenance",
        "package_manager",
    ];
    let mut found = Vec::new();
    // Parse defensively: malformed TOML is a config error elsewhere; here
    // we do a bounded line scan so hostile content cannot hide behind a
    // parse failure.
    for line in toml_text.lines().take(2000) {
        let t = line.trim().trim_start_matches('[').trim_end_matches(']');
        for key in FORBIDDEN {
            let bare = format!("{key} =");
            let quoted = format!("\"{key}\" =");
            if (t.starts_with(&bare) || t.starts_with(&quoted)) && !found.contains(&key.to_string())
            {
                found.push(key.to_string());
            }
        }
        // [lifecycle] / [update] / [install] tables are product-owned.
        let section = t.trim().to_lowercase();
        for table in ["lifecycle", "update", "install", "channel", "retention"] {
            if section == *table || section.starts_with(&format!("{table}.")) {
                let marker = format!("table:{table}");
                if !found.contains(&marker) {
                    found.push(marker);
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_binary_never_managed() {
        let base = PathBuf::from("D:/state-test");
        for p in [
            "D:/repo/target/debug/omen.exe",
            "/home/u/repo/target/release/omen",
        ] {
            assert_eq!(
                detect_ownership(&base, Path::new(p)),
                Ownership::Development,
                "{p}"
            );
        }
    }

    #[test]
    fn unknown_by_default() {
        let base = PathBuf::from("D:/no-record-here");
        assert_eq!(
            detect_ownership(&base, Path::new("D:/random/omen.exe")),
            Ownership::Unknown
        );
        assert!(!Ownership::Unknown.self_update_allowed());
        assert!(Ownership::Omen.self_update_allowed());
    }

    #[test]
    fn channel_parse() {
        assert_eq!(Channel::parse("Preview"), Some(Channel::Preview));
        assert_eq!(Channel::parse("stable"), Some(Channel::Stable));
        assert_eq!(Channel::parse("nightly"), None);
    }

    #[test]
    fn workspace_channel_attack_detected() {
        let evil =
            "[workspace]\nchannel = \"stable\"\nupdate_source = \"https://evil.example/x\"\n";
        let v = workspace_lifecycle_violations(evil);
        assert!(v.contains(&"channel".to_string()));
        assert!(v.contains(&"update_source".to_string()));
    }

    #[test]
    fn workspace_table_attack_detected() {
        let evil = "[lifecycle]\nkeep_days = 3\n";
        let v = workspace_lifecycle_violations(evil);
        assert!(v.contains(&"table:lifecycle".to_string()));
    }

    #[test]
    fn benign_workspace_config_clean() {
        let ok = "[workspace]\nname = \"demo\"\n\n[actions.build]\nrun = [\"cargo\", \"build\"]\n";
        assert!(workspace_lifecycle_violations(ok).is_empty());
    }
}
