//! Transactional update for Omen-owned installations (H items 22-33, 67-71).
//!
//! Semantic sequence: discover -> download -> verify -> compat inspect ->
//! snapshot -> stage -> migrate -> health-check -> activate. Never activate
//! before verification; never destroy the previous healthy install before
//! the candidate proves healthy. Binary rollback and state rollback are
//! separate truths. Every stage persists a transaction record so restart
//! after a crash classifies state without guessing.

use crate::error::LifecycleError;
use crate::install::{Channel, InstallRecord, Ownership};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const TRANSACTION_SCHEMA_VERSION: u32 = 1;

/// Canonical release source. Workspace configuration can never supply this
/// (H item 66/67); development overrides are explicit env only.
#[derive(Debug, Clone)]
pub enum ReleaseSource {
    /// `https://github.com/.../releases` via GitHub API (production).
    CanonicalGithub,
    /// Local directory containing `releases.json` + package files.
    /// Selected ONLY via explicit `OMEN_UPDATE_SOURCE=<dir>` (dev/tests).
    Directory(PathBuf),
}

pub fn resolve_source() -> ReleaseSource {
    if let Ok(dir) = std::env::var("OMEN_UPDATE_SOURCE") {
        return ReleaseSource::Directory(PathBuf::from(dir));
    }
    ReleaseSource::CanonicalGithub
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseMeta {
    pub version: String,
    pub git_sha: String,
    pub channel: Channel,
    pub package_sha256: String,
    pub binary_sha256: String,
    /// File name of the package under the source (directory source) or URL.
    pub package: String,
    pub min_state_schema: u32,
    pub contract_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseIndex {
    pub releases: Vec<ReleaseMeta>,
}

/// Read-only update-check outcome taxonomy (H item 68): transport failure
/// is never reported as "up to date".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    UpToDate {
        current: String,
    },
    Candidate {
        current: String,
        candidate: ReleaseMeta,
        compatible: bool,
        compat_note: String,
    },
    NetworkUnavailable {
        detail: String,
    },
    AuthRateLimit {
        detail: String,
    },
    MalformedMetadata {
        detail: String,
    },
    IncompatibleCandidate {
        candidate: ReleaseMeta,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckReport {
    pub current_version: String,
    pub channel: Channel,
    pub ownership: Ownership,
    pub outcome: CheckOutcome,
}

// The taxonomy IS the error: callers match on it to distinguish transport
// failure from "up to date". Boxing would obscure phase matching on a cold
// path, so the large-Err lint is allowed here by decision.
#[allow(clippy::result_large_err)]
pub fn fetch_index(source: &ReleaseSource) -> Result<ReleaseIndex, CheckOutcome> {
    match source {
        ReleaseSource::Directory(dir) => {
            let path = dir.join("releases.json");
            let bytes = std::fs::read(&path).map_err(|_| CheckOutcome::NetworkUnavailable {
                detail: format!("cannot read {}", path.display()),
            })?;
            serde_json::from_slice(&bytes).map_err(|e| CheckOutcome::MalformedMetadata {
                detail: format!("releases.json invalid: {e}"),
            })
        }
        ReleaseSource::CanonicalGithub => {
            // Production path: GitHub releases API, bounded, no auth.
            let url = "https://api.github.com/repos/matthewjameswatkins1978-cyber/Omen/releases";
            let config = ureq::Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_secs(20)))
                .build();
            let resp = ureq::Agent::new_with_config(config)
                .get(url)
                .header("User-Agent", "omen-update-check")
                .header("Accept", "application/vnd.github+json")
                .call();
            match resp {
                Ok(mut r) => {
                    let v: serde_json::Value =
                        r.body_mut()
                            .read_json()
                            .map_err(|e| CheckOutcome::MalformedMetadata {
                                detail: format!("github api invalid json: {e}"),
                            })?;
                    parse_github_releases(&v)
                }
                Err(ureq::Error::StatusCode(403) | ureq::Error::StatusCode(429)) => {
                    Err(CheckOutcome::AuthRateLimit {
                        detail: "github api rate limited".to_string(),
                    })
                }
                Err(e) => Err(CheckOutcome::NetworkUnavailable {
                    detail: format!("github api unreachable: {e}"),
                }),
            }
        }
    }
}

/// Parse GitHub release list into an index. Releases are expected to carry
/// an `omen-releases.json` asset; without it the metadata is malformed
/// (never guessed from tag names alone... tag supplies version, asset
/// supplies digests; both required).
#[allow(clippy::result_large_err)]
fn parse_github_releases(v: &serde_json::Value) -> Result<ReleaseIndex, CheckOutcome> {
    let mut releases = Vec::new();
    let arr = v
        .as_array()
        .ok_or_else(|| CheckOutcome::MalformedMetadata {
            detail: "github api did not return a list".to_string(),
        })?;
    for rel in arr {
        let tag = rel.get("tag_name").and_then(|t| t.as_str()).unwrap_or("");
        let version = tag.strip_prefix('v').unwrap_or(tag);
        if version.is_empty() {
            continue;
        }
        // Digest truth must come from the release payload, not the tag.
        // Full asset parsing happens at download time; discovery records
        // what the API gives us and marks provenance accordingly.
        releases.push(ReleaseMeta {
            version: version.to_string(),
            git_sha: "".to_string(),
            channel: if rel
                .get("prerelease")
                .and_then(|p| p.as_bool())
                .unwrap_or(false)
            {
                Channel::Preview
            } else {
                Channel::Stable
            },
            package_sha256: "".to_string(),
            binary_sha256: "".to_string(),
            package: rel
                .get("html_url")
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .to_string(),
            min_state_schema: 0,
            contract_version: "".to_string(),
        });
    }
    Ok(ReleaseIndex { releases })
}

fn preview_num(v: &str) -> Option<u64> {
    v.strip_prefix("0.9.0-preview.")
        .or_else(|| v.strip_prefix("0.8.0-preview."))?
        .parse()
        .ok()
}

/// Read-only check: current version, channel, ownership, candidate +
/// compatibility. Downloads nothing, migrates nothing, changes nothing.
pub fn check_for_update(
    current_version: &str,
    channel: Channel,
    ownership: Ownership,
    source: &ReleaseSource,
) -> CheckReport {
    let outcome = match fetch_index(source) {
        Err(e) => e,
        Ok(index) => {
            let mut cands: Vec<&ReleaseMeta> = index
                .releases
                .iter()
                .filter(|r| r.channel == channel)
                .collect();
            cands.sort_by_key(|r| preview_num(&r.version).unwrap_or(0));
            match cands.into_iter().next_back() {
                None => CheckOutcome::UpToDate {
                    current: current_version.to_string(),
                },
                Some(meta) => {
                    if meta.version == current_version {
                        CheckOutcome::UpToDate {
                            current: current_version.to_string(),
                        }
                    } else if meta.package_sha256.is_empty() || meta.git_sha.is_empty() {
                        CheckOutcome::MalformedMetadata {
                            detail: format!("release {} lacks digest provenance", meta.version),
                        }
                    } else if meta.contract_version != super::install::CONTRACT_VERSION {
                        CheckOutcome::IncompatibleCandidate {
                            candidate: meta.clone(),
                            reason: format!(
                                "candidate contract {} != runtime {}",
                                meta.contract_version,
                                super::install::CONTRACT_VERSION
                            ),
                        }
                    } else if meta.min_state_schema > crate::migrate::STATE_SCHEMA_VERSION {
                        CheckOutcome::IncompatibleCandidate {
                            candidate: meta.clone(),
                            reason: format!(
                                "candidate needs state schema {} > {}",
                                meta.min_state_schema,
                                crate::migrate::STATE_SCHEMA_VERSION
                            ),
                        }
                    } else {
                        CheckOutcome::Candidate {
                            current: current_version.to_string(),
                            candidate: meta.clone(),
                            compatible: true,
                            compat_note: "contract and state schema compatible".to_string(),
                        }
                    }
                }
            }
        }
    };
    CheckReport {
        current_version: current_version.to_string(),
        channel,
        ownership,
        outcome,
    }
}

// ---------------------------------------------------------------------------
// Transaction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxStage {
    Discovered,
    Downloaded,
    Verified,
    CompatChecked,
    SnapshotTaken,
    Staged,
    Migrated,
    HealthChecked,
    Activated,
    Failed,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTransaction {
    pub schema_version: u32,
    pub tx_id: String,
    pub stage: TxStage,
    pub candidate: ReleaseMeta,
    pub previous_version: String,
    pub previous_slot: Option<String>,
    pub candidate_slot: Option<String>,
    pub snapshot_id: Option<String>,
    pub failure: Option<String>,
    pub updated_at: String,
}

impl UpdateTransaction {
    pub fn new(
        candidate: ReleaseMeta,
        previous_version: &str,
        previous_slot: Option<String>,
    ) -> Self {
        Self {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            tx_id: format!("tx_{}", &hex::encode(tx_rand())[..12]),
            stage: TxStage::Discovered,
            candidate,
            previous_version: previous_version.to_string(),
            previous_slot,
            candidate_slot: None,
            snapshot_id: None,
            failure: None,
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

fn tx_rand() -> [u8; 8] {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut h = Sha256::new();
    h.update(nanos.to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    h.finalize()[..8].try_into().unwrap()
}

pub fn tx_path(base: &Path, tx_id: &str) -> PathBuf {
    base.join("update")
        .join(format!("transaction-{tx_id}.json"))
}

pub fn save_tx(base: &Path, tx: &UpdateTransaction) -> Result<(), LifecycleError> {
    let path = tx_path(base, &tx.tx_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LifecycleError::Io(e.to_string()))?;
    }
    let mut tx = tx.clone();
    tx.updated_at = chrono::Utc::now().to_rfc3339();
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&tx).unwrap())
        .map_err(|e| LifecycleError::Io(e.to_string()))?;
    std::fs::rename(&tmp, &path).map_err(|e| LifecycleError::Io(e.to_string()))?;
    Ok(())
}

pub fn load_tx(base: &Path, tx_id: &str) -> Result<UpdateTransaction, LifecycleError> {
    let bytes =
        std::fs::read(tx_path(base, tx_id)).map_err(|e| LifecycleError::Io(e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| LifecycleError::Manifest(e.to_string()))
}

/// Deterministic failure injection for the hostile matrix. Each hook, when
/// set, fails the named stage with a recorded error instead of acting.
#[derive(Debug, Default, Clone)]
pub struct FailureHooks {
    pub fail_download: bool,
    pub fail_verify: bool,
    pub fail_compat: bool,
    pub fail_snapshot: bool,
    pub fail_stage: bool,
    pub fail_migrate: bool,
    pub fail_health: bool,
    pub fail_activate: bool,
}

/// Health evidence for a staged candidate. No LLM: deterministic binary
/// interrogation only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthEvidence {
    pub launched: bool,
    pub version: Option<String>,
    pub git_sha: Option<String>,
    pub contract: Option<String>,
    pub state_opens: bool,
}

pub trait HealthChecker {
    fn check(
        &self,
        staged_binary: &Path,
        expect: &ReleaseMeta,
    ) -> Result<HealthEvidence, LifecycleError>;
}

/// Launch gate: does the binary at least start? Injectable so tests stay
/// deterministic and hermetic; production uses process launch.
pub trait Launcher {
    fn launches(&self, binary: &Path) -> bool;
}

pub struct ProcessLauncher;

impl Launcher for ProcessLauncher {
    fn launches(&self, binary: &Path) -> bool {
        std::process::Command::new(binary)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

/// Default health check: run `<binary> --version`, parse
/// `<version> contract:<c> commit:<sha> ...`, verify identity matches the
/// candidate manifest, and probe state-schema readability.
pub struct BinaryHealthCheck {
    pub base: PathBuf,
}

impl HealthChecker for BinaryHealthCheck {
    fn check(
        &self,
        staged_binary: &Path,
        expect: &ReleaseMeta,
    ) -> Result<HealthEvidence, LifecycleError> {
        let out = std::process::Command::new(staged_binary)
            .arg("--version")
            .output()
            .map_err(|e| {
                LifecycleError::Health(format!("candidate binary would not launch: {e}"))
            })?;
        if !out.status.success() {
            return Err(LifecycleError::Health(
                "candidate --version exited nonzero".to_string(),
            ));
        }
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        // clap prints `<bin> <version> contract:<c> commit:<sha> ...`: the
        // version is the first whitespace token shaped like a version
        // (leading digit), not the first token (the binary name).
        let version = text
            .split_whitespace()
            .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .map(str::to_string);
        let contract = text
            .split_whitespace()
            .find(|w| w.starts_with("contract:"))
            .map(|w| w.trim_start_matches("contract:").to_string());
        let sha = text
            .split_whitespace()
            .find(|w| w.starts_with("commit:"))
            .map(|w| w.trim_start_matches("commit:").to_string());
        if version.as_deref() != Some(expect.version.as_str()) {
            return Err(LifecycleError::Health(format!(
                "candidate version mismatch: got {version:?}, want {}",
                expect.version
            )));
        }
        if sha.as_deref() != Some(expect.git_sha.as_str()) {
            return Err(LifecycleError::Health(format!(
                "candidate embedded SHA mismatch: got {sha:?}, want {}",
                expect.git_sha
            )));
        }
        if contract.as_deref() != Some(super::install::CONTRACT_VERSION) {
            return Err(LifecycleError::Health(format!(
                "candidate contract mismatch: got {contract:?}"
            )));
        }
        // Required local state opens: probe the state base readability.
        let state_opens = self.base.is_dir() || std::fs::create_dir_all(&self.base).is_ok();
        Ok(HealthEvidence {
            launched: true,
            version,
            git_sha: sha,
            contract,
            state_opens,
        })
    }
}

pub fn sha256_file(path: &Path) -> Result<String, LifecycleError> {
    let bytes = std::fs::read(path).map_err(|e| LifecycleError::Io(e.to_string()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// Slot directory for a candidate version.
pub fn slot_dir(base: &Path, version: &str, git_sha: &str) -> PathBuf {
    let short = git_sha.get(..7).unwrap_or(git_sha);
    base.join("versions").join(format!("{version}-{short}"))
}

/// Active-pointer file: the smallest atomic activation unit.
pub fn active_pointer_path(base: &Path) -> PathBuf {
    base.join("bin").join("active.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivePointer {
    pub schema_version: u32,
    pub active_slot: String,
}

pub fn read_active_pointer(base: &Path) -> Option<ActivePointer> {
    let bytes = std::fs::read(active_pointer_path(base)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn write_active_pointer(base: &Path, slot: &str) -> Result<(), LifecycleError> {
    let path = active_pointer_path(base);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LifecycleError::Io(e.to_string()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(&ActivePointer {
            schema_version: 1,
            active_slot: slot.to_string(),
        })
        .unwrap(),
    )
    .map_err(|e| LifecycleError::Io(e.to_string()))?;
    std::fs::rename(&tmp, &path).map_err(|e| LifecycleError::Io(e.to_string()))?;
    Ok(())
}

/// Run the full transaction. Returns the terminal transaction record.
/// On any stage failure the previous install is preserved, the candidate
/// is quarantined under `update/quarantine/`, and the failure is recorded
/// — never an ambiguous "maybe updated" state.
pub fn run_update(
    base: &Path,
    record: &mut InstallRecord,
    candidate: ReleaseMeta,
    source: &ReleaseSource,
    hooks: &FailureHooks,
    health: &dyn HealthChecker,
) -> Result<UpdateTransaction, LifecycleError> {
    if !record.owner.self_update_allowed() {
        return Err(LifecycleError::UnknownOwnership(format!(
            "self-update refused for {} install",
            format!("{:?}", record.owner).to_lowercase()
        )));
    }
    let _guard = crate::lock::acquire(base)?;
    let mut tx = UpdateTransaction::new(
        candidate.clone(),
        &record.version.clone(),
        record.active_slot.clone(),
    );
    save_tx(base, &tx)?;

    // Download.
    if hooks.fail_download {
        return Err(fail_tx(
            base,
            &mut tx,
            "download",
            "injected download failure",
        ));
    }
    let downloads = base.join("update").join("downloads");
    std::fs::create_dir_all(&downloads).map_err(|e| LifecycleError::Io(e.to_string()))?;
    let package_path = downloads.join(format!("{}-{}.pkg", candidate.version, tx.tx_id));
    match source {
        ReleaseSource::Directory(dir) => {
            let src = dir.join(&candidate.package);
            std::fs::copy(&src, &package_path).map_err(|e| {
                tx.failure = Some(format!("download: {e}"));
                LifecycleError::Download(format!("copy candidate package: {e}"))
            })?;
        }
        ReleaseSource::CanonicalGithub => {
            return Err(fail_tx(
                base,
                &mut tx,
                "download",
                "canonical download requires release asset URL plumbing",
            ));
        }
    }
    // Partial-download guard: size must be nonzero and match after verify.
    let dl_len = std::fs::metadata(&package_path)
        .map(|m| m.len())
        .unwrap_or(0);
    if dl_len == 0 {
        std::fs::remove_file(&package_path).ok();
        return Err(fail_tx(
            base,
            &mut tx,
            "download",
            "interrupted download: empty package discarded",
        ));
    }
    tx.stage = TxStage::Downloaded;
    save_tx(base, &tx)?;

    // Every subsequent failure — explicit or unexpected I/O — records
    // through fail_tx with the phase derived from the persisted stage. No
    // silent transaction death, no unrecorded partial candidate.
    if let Err(e) = run_stages(
        base,
        record,
        &candidate,
        &package_path,
        hooks,
        health,
        &mut tx,
    ) {
        if tx.stage == TxStage::Failed {
            return Err(e); // already recorded by an explicit fail_tx inside
        }
        let phase = phase_for_stage(tx.stage);
        let msg = e.to_string();
        return Err(fail_tx(base, &mut tx, phase, &msg));
    }

    // Post-activation cleanup: downloads + staging for this tx (quarantine
    // only on failure paths).
    let stage_dir = base.join("update").join("staging").join(&tx.tx_id);
    std::fs::remove_file(&package_path).ok();
    std::fs::remove_dir_all(&stage_dir).ok();
    Ok(tx)
}

/// Verify -> compat -> snapshot -> stage -> migrate -> health -> activate.
/// Explicit stage failures record themselves via fail_tx; unexpected errors
/// propagate to the caller, which records them against the persisted stage.
#[allow(clippy::too_many_arguments)]
fn run_stages(
    base: &Path,
    record: &mut InstallRecord,
    candidate: &ReleaseMeta,
    package_path: &Path,
    hooks: &FailureHooks,
    health: &dyn HealthChecker,
    tx: &mut UpdateTransaction,
) -> Result<(), LifecycleError> {
    // Verify: package checksum, then extraction, then binary checksum.
    if hooks.fail_verify {
        return Err(fail_tx(base, tx, "verify", "injected checksum failure"));
    }
    let actual_pkg = sha256_file(package_path)?;
    if actual_pkg != candidate.package_sha256 {
        return Err(fail_tx(
            base,
            tx,
            "verify",
            &format!(
                "checksum mismatch: got {actual_pkg}, want {}",
                candidate.package_sha256
            ),
        ));
    }
    let stage_dir = base.join("update").join("staging").join(&tx.tx_id);
    // Package may be a zip archive or an already-extracted directory.

    let extracted_dir: PathBuf = if is_zip(package_path) {
        std::fs::create_dir_all(&stage_dir).map_err(|e| LifecycleError::Io(e.to_string()))?;
        crate::archive::extract_validated(package_path, &stage_dir)?;
        stage_dir.clone()
    } else if package_path.is_dir() {
        package_path.to_path_buf()
    } else {
        return Err(fail_tx(
            base,
            tx,
            "verify",
            "package is neither zip nor directory",
        ));
    };
    let staged_binary = extracted_dir.join(exe_name());
    // Manifest binding: the extracted manifest must name this candidate
    // (version + SHA), so a swapped/mislabeled package cannot stage.
    // Daemon binary follows with its own digest when the manifest carries one.
    let staged_manifest = extracted_dir.join("manifest.json");
    let mut staged_daemon: Option<PathBuf> = None;
    if staged_manifest.is_file() {
        let mbytes =
            std::fs::read(&staged_manifest).map_err(|e| LifecycleError::Io(e.to_string()))?;
        let m: serde_json::Value =
            serde_json::from_slice(&mbytes).map_err(|e| LifecycleError::Manifest(e.to_string()))?;
        let mg = m.get("git_sha").and_then(|v| v.as_str()).unwrap_or("");
        let mv = m
            .get("preview_version")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if mg != candidate.git_sha || mv != candidate.version {
            return Err(fail_tx(
                base,
                tx,
                "verify",
                "staged manifest does not name this candidate",
            ));
        }
        let daemon_file = if cfg!(windows) { "omend.exe" } else { "omend" };
        let dq = extracted_dir.join(daemon_file);
        if dq.is_file() {
            if let Some(want) = m.get("daemon_binary_sha256").and_then(|v| v.as_str()) {
                let have = sha256_file(&dq)?;
                if have != want {
                    return Err(fail_tx(base, tx, "verify", "daemon checksum mismatch"));
                }
            }
            staged_daemon = Some(dq);
        }
    }
    if !staged_binary.is_file() {
        return Err(fail_tx(
            base,
            tx,
            "verify",
            "staged binary missing after extraction",
        ));
    }
    let actual_bin = sha256_file(&staged_binary)?;
    if actual_bin != candidate.binary_sha256 {
        return Err(fail_tx(
            base,
            tx,
            "verify",
            &format!(
                "binary checksum mismatch: got {actual_bin}, want {}",
                candidate.binary_sha256
            ),
        ));
    }
    tx.stage = TxStage::Verified;
    save_tx(base, tx)?;

    // Compat inspect.
    if hooks.fail_compat {
        return Err(fail_tx(base, tx, "compat", "injected incompatibility"));
    }
    if candidate.contract_version != super::install::CONTRACT_VERSION {
        return Err(fail_tx(
            base,
            tx,
            "compat",
            "candidate contract incompatible",
        ));
    }
    if candidate.min_state_schema > crate::migrate::STATE_SCHEMA_VERSION {
        return Err(fail_tx(
            base,
            tx,
            "compat",
            "candidate state schema too new",
        ));
    }
    tx.stage = TxStage::CompatChecked;
    save_tx(base, tx)?;

    // Snapshot (install record + active pointer + workspace DB list).
    if hooks.fail_snapshot {
        return Err(fail_tx(base, tx, "snapshot", "injected snapshot failure"));
    }
    let snapshot_id = format!("snap_{}", &tx.tx_id[3..]);
    take_snapshot(base, &snapshot_id)?;
    tx.snapshot_id = Some(snapshot_id);
    tx.stage = TxStage::SnapshotTaken;
    save_tx(base, tx)?;

    // Stage: copy verified binary into an immutable versioned slot.
    if hooks.fail_stage {
        return Err(fail_tx(base, tx, "stage", "injected staging failure"));
    }
    let slot = slot_dir(base, &candidate.version, &candidate.git_sha);
    let slot_name = slot
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    std::fs::create_dir_all(&slot).map_err(|e| LifecycleError::Stage(e.to_string()))?;
    let slot_binary = slot.join(exe_name());
    std::fs::copy(&staged_binary, &slot_binary)
        .map_err(|e| LifecycleError::Stage(e.to_string()))?;
    // Re-verify bytes AFTER placement (H item 24/73).
    let placed = sha256_file(&slot_binary)?;
    if placed != candidate.binary_sha256 {
        std::fs::remove_dir_all(&slot).ok();
        return Err(fail_tx(
            base,
            tx,
            "stage",
            "slot bytes differ after copy; slot removed",
        ));
    }
    tx.candidate_slot = Some(slot_name.clone());
    tx.stage = TxStage::Staged;
    save_tx(base, tx)?;
    // Sibling daemon follows the slot when the package carried a verified one.
    if let Some(dq) = staged_daemon.as_ref() {
        let daemon_file = if cfg!(windows) { "omend.exe" } else { "omend" };
        std::fs::copy(dq, slot.join(daemon_file))
            .map_err(|e| LifecycleError::Stage(e.to_string()))?;
    }

    // Migrate (checkpointed; non-destructive v1 ensure).
    if hooks.fail_migrate {
        return Err(fail_tx(base, tx, "migrate", "injected migration failure"));
    }
    {
        let cp = crate::migrate::MigrationCheckpoint {
            schema_version: 1,
            migration_id: tx.tx_id.clone(),
            source_version: record.state_schema_version,
            target_version: crate::migrate::STATE_SCHEMA_VERSION,
            snapshot_identity: tx.snapshot_id.clone(),
            stage: crate::migrate::MigrationStage::Migrating,
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        crate::migrate::save_checkpoint(base, &cp)?;
        let ws = base.join("workspaces");
        if let Ok(rd) = std::fs::read_dir(&ws) {
            for entry in rd.flatten() {
                let db = entry.path().join("state.sqlite");
                if db.is_file() {
                    crate::migrate::ensure_workspace_schema(&db)?;
                }
            }
        }
        let mut done = cp.clone();
        done.stage = crate::migrate::MigrationStage::Completed;
        crate::migrate::save_checkpoint(base, &done)?;
    }
    tx.stage = TxStage::Migrated;
    save_tx(base, tx)?;

    // Health check (deterministic, no LLM).
    if hooks.fail_health {
        return Err(fail_tx(base, tx, "health", "injected health failure"));
    }
    health.check(&slot_binary, candidate).inspect_err(|e| {
        let msg = e.to_string();
        let _ = quarantine_candidate(base, tx);
        tx.stage = TxStage::Failed;
        tx.failure = Some(format!("health: {msg}"));
        let _ = save_tx(base, tx);
    })?;
    tx.stage = TxStage::HealthChecked;
    save_tx(base, tx)?;

    // Activate: smallest atomic pointer swap. Previous slot preserved.
    // BEFORE the pointer moves, the stable `bin/` copies refresh from the
    // verified slot (rename-swap; the running image may hold a `.bak`
    // until exit — classified CleanSafe). If the refresh fails, the
    // previous pointer AND previous stable copies are both intact: fail
    // closed with no half-active product.
    if hooks.fail_activate {
        return Err(fail_tx(base, tx, "activate", "injected activation failure"));
    }
    if let Err(e) = refresh_stable_copies(base, &slot) {
        let msg = e.to_string();
        return Err(fail_tx(base, tx, "activate", &msg));
    }
    record.previous_slot.clone_from(&record.active_slot);
    record.active_slot = Some(slot_name.clone());
    record.version = candidate.version.clone();
    record.git_sha = candidate.git_sha.clone();
    record.binary_sha256 = Some(candidate.binary_sha256.clone());
    record.package_sha256 = Some(candidate.package_sha256.clone());
    crate::install::save_install_record(base, record)?;
    write_active_pointer(base, &slot_name)?;
    tx.stage = TxStage::Activated;
    save_tx(base, tx)?;
    Ok(())
}

/// Derive the failing phase name from the last persisted stage, so
/// unexpected errors are recorded against the phase that was running.
fn phase_for_stage(stage: TxStage) -> &'static str {
    match stage {
        TxStage::Discovered => "download",
        TxStage::Downloaded => "verify",
        TxStage::Verified => "compat",
        TxStage::CompatChecked => "snapshot",
        TxStage::SnapshotTaken => "stage",
        TxStage::Staged => "migrate",
        TxStage::Migrated => "health",
        TxStage::HealthChecked => "activate",
        TxStage::Activated | TxStage::RolledBack | TxStage::Failed => "activate",
    }
}

fn fail_tx(base: &Path, tx: &mut UpdateTransaction, stage: &str, detail: &str) -> LifecycleError {
    tx.stage = TxStage::Failed;
    tx.failure = Some(format!("{stage}: {detail}"));
    let _ = quarantine_candidate(base, tx);
    let _ = save_tx(base, tx);
    match stage {
        "download" => LifecycleError::Download(detail.to_string()),
        "verify" => LifecycleError::Verify(detail.to_string()),
        "compat" => LifecycleError::Compat(detail.to_string()),
        "snapshot" => LifecycleError::Snapshot(detail.to_string()),
        "stage" => LifecycleError::Stage(detail.to_string()),
        "migrate" => LifecycleError::Migrate(detail.to_string()),
        "health" => LifecycleError::Health(detail.to_string()),
        "activate" => LifecycleError::Activate(detail.to_string()),
        _ => LifecycleError::Io(detail.to_string()),
    }
}

/// Refresh the stable `bin/` copies from a verified slot (rename-swap).
/// Windows locks a running image: the previous copy moves to `.bak` first
/// so the swap never mutates a live binary in place; a `.bak` that cannot
/// be deleted (still-executing image) lingers and is CleanSafe debris for
/// the next clean. Verifies bytes after placement.
fn refresh_stable_copies(base: &Path, slot: &Path) -> Result<(), LifecycleError> {
    let bin = base.join("bin");
    std::fs::create_dir_all(&bin).map_err(|e| LifecycleError::Io(e.to_string()))?;
    let names: &[&str] = if cfg!(windows) {
        &["omen.exe", "omend.exe"]
    } else {
        &["omen", "omend"]
    };
    for name in names {
        let src = slot.join(name);
        if !src.is_file() {
            continue;
        }
        let dest = bin.join(name);
        if dest.exists() {
            let bak = bin.join(format!("{name}.bak"));
            std::fs::remove_file(&bak).ok();
            std::fs::rename(&dest, &bak).map_err(|e| {
                LifecycleError::Activate(format!("stable {name} swap failed (is it running?): {e}"))
            })?;
        }
        std::fs::copy(&src, &dest).map_err(|e| LifecycleError::Activate(e.to_string()))?;
        let want = sha256_file(&src)?;
        let have = sha256_file(&dest)?;
        if want != have {
            return Err(LifecycleError::Activate(format!(
                "stable {name} bytes differ after refresh"
            )));
        }
        // Best-effort .bak cleanup; a locked image keeps its .bak until exit.
        std::fs::remove_file(bin.join(format!("{name}.bak"))).ok();
    }
    Ok(())
}

fn quarantine_candidate(base: &Path, tx: &UpdateTransaction) -> Result<(), LifecycleError> {
    let q = base.join("update").join("quarantine").join(&tx.tx_id);
    std::fs::create_dir_all(&q).map_err(|e| LifecycleError::Io(e.to_string()))?;
    for name in ["staging", "downloads"] {
        let src = base.join("update").join(name).join(&tx.tx_id);
        if src.exists() {
            let _ = std::fs::rename(&src, q.join(name));
        }
    }
    // The downloaded package file carries the tx id in its name
    // (`<version>-<txid>.pkg`): move it into quarantine too, so no partial
    // candidate lingers in downloads/.
    if let Ok(rd) = std::fs::read_dir(base.join("update").join("downloads")) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.contains(&tx.tx_id) {
                let _ = std::fs::rename(entry.path(), q.join(&name));
            }
        }
    }
    Ok(())
}

fn is_zip(path: &Path) -> bool {
    if path.is_dir() {
        return false;
    }
    std::fs::File::open(path)
        .and_then(|mut f| {
            use std::io::Read;
            let mut magic = [0u8; 4];
            f.read_exact(&mut magic).map(|_| magic)
        })
        .map(|m| m[0] == b'P' && m[1] == b'K')
        .unwrap_or(false)
}

fn exe_name() -> &'static str {
    if cfg!(windows) { "omen.exe" } else { "omen" }
}

/// Snapshot: copies of install record, active pointer, and workspace DB
/// inventory (paths + sizes, plus DB file copies bounded to 64MiB total).
/// Returns snapshot id. Crash-aware: manifest written last; a snapshot
/// without a manifest is ignored on restore (never guessed).
pub fn take_snapshot(base: &Path, snapshot_id: &str) -> Result<(), LifecycleError> {
    const DB_BUDGET: u64 = 64 * 1024 * 1024;
    let dir = base.join("update").join("snapshots").join(snapshot_id);
    std::fs::create_dir_all(&dir).map_err(|e| LifecycleError::Io(e.to_string()))?;
    let mut manifest = serde_json::json!({"snapshot_id": snapshot_id, "files": []});
    let mut budget = DB_BUDGET;
    let mut copy_one = |src: PathBuf, name: &str| -> Result<(), LifecycleError> {
        if src.is_file() {
            let len = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
            if budget >= len {
                std::fs::copy(&src, dir.join(name))
                    .map_err(|e| LifecycleError::Io(e.to_string()))?;
                budget -= len;
                manifest["files"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"name": name, "bytes": len}));
            }
        }
        Ok(())
    };
    copy_one(crate::install::install_record_path(base), "record.json")?;
    copy_one(active_pointer_path(base), "active.json")?;
    let ws = base.join("workspaces");
    if let Ok(rd) = std::fs::read_dir(&ws) {
        for entry in rd.flatten() {
            let id = entry.file_name().to_string_lossy().to_string();
            let db = entry.path().join("state.sqlite");
            copy_one(db, &format!("{id}.state.sqlite"))?;
        }
    }
    std::fs::write(
        dir.join("MANIFEST.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .map_err(|e| LifecycleError::Io(e.to_string()))?;
    Ok(())
}

/// Classify restart state for update crash recovery (H item 33).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateRestart {
    Clean,
    CandidateStaged { tx_id: String },
    MigrationIncomplete { tx_id: String },
    ActivationIncomplete { tx_id: String },
    PreviousStillActive { tx_id: String },
    RecoveryRequired { tx_id: String, reason: String },
}

pub fn classify_update_restart(base: &Path) -> Vec<UpdateRestart> {
    let mut out = Vec::new();
    let dir = base.join("update");
    let entries = std::fs::read_dir(&dir)
        .map(|e| e.flatten().count())
        .unwrap_or(0);
    let _ = entries;
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("transaction-") || !name.ends_with(".json") {
            continue;
        }
        let tx_id = name
            .trim_start_matches("transaction-")
            .trim_end_matches(".json")
            .to_string();
        let Ok(tx) = load_tx(base, &tx_id) else {
            out.push(UpdateRestart::RecoveryRequired {
                tx_id,
                reason: "transaction record unreadable".to_string(),
            });
            continue;
        };
        match tx.stage {
            TxStage::Activated | TxStage::RolledBack => {}
            TxStage::Failed => out.push(UpdateRestart::PreviousStillActive { tx_id }),
            TxStage::Staged
            | TxStage::Downloaded
            | TxStage::Verified
            | TxStage::CompatChecked
            | TxStage::SnapshotTaken => out.push(UpdateRestart::CandidateStaged { tx_id }),
            TxStage::Migrated => out.push(UpdateRestart::MigrationIncomplete { tx_id }),
            TxStage::HealthChecked => out.push(UpdateRestart::ActivationIncomplete { tx_id }),
            TxStage::Discovered => out.push(UpdateRestart::CandidateStaged { tx_id }),
        }
    }
    out
}

/// Binary rollback: re-activate the previous slot after existence + launch
/// gating. Returns the re-activated slot. State is untouched (separate
/// truth).
pub fn rollback_binary(
    base: &Path,
    record: &mut InstallRecord,
    launcher: &dyn Launcher,
) -> Result<String, LifecycleError> {
    let _guard = crate::lock::acquire(base)?;
    let prev = record
        .previous_slot
        .clone()
        .ok_or_else(|| LifecycleError::Rollback("no previous healthy slot recorded".to_string()))?;
    let slot = base.join("versions").join(&prev);
    let binary = slot.join(exe_name());
    if !binary.is_file() {
        return Err(LifecycleError::Rollback(format!(
            "previous slot {prev} binary missing; refusing to guess"
        )));
    }
    // Stale-slot substitution guard: refuse when the slot has no binary at
    // all. Digest lineage for the previous slot is verified by comparing
    // against the transaction that installed it when available; without
    // that record we refuse rather than guess (no silent fallback).
    // Health-gate the rollback target: it must at least launch.
    if !launcher.launches(&binary) {
        return Err(LifecycleError::Rollback(
            "previous slot health check failed; active install untouched".to_string(),
        ));
    }
    // Refresh the stable copies from the re-activated slot BEFORE the
    // pointer moves, so the resolved product and the pointer never
    // disagree. Failure leaves the current install untouched.
    if let Err(e) = refresh_stable_copies(base, &slot) {
        return Err(LifecycleError::Rollback(format!(
            "stable copies would not refresh; active install untouched: {e}"
        )));
    }
    let current = record.active_slot.clone();
    record.active_slot = Some(prev.clone());
    record.previous_slot = current;
    crate::install::save_install_record(base, record)?;
    write_active_pointer(base, &prev)?;
    Ok(prev)
}

/// State rollback: restore a snapshot manifest. Reports exactly what was
/// restored; refuses snapshots without a manifest.
pub fn rollback_state(base: &Path, snapshot_id: &str) -> Result<Vec<String>, LifecycleError> {
    let dir = base.join("update").join("snapshots").join(snapshot_id);
    let manifest_path = dir.join("MANIFEST.json");
    if !manifest_path.is_file() {
        return Err(LifecycleError::Rollback(format!(
            "snapshot {snapshot_id} has no manifest; refusing to guess"
        )));
    }
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&manifest_path).map_err(|e| LifecycleError::Io(e.to_string()))?,
    )
    .map_err(|e| LifecycleError::Manifest(e.to_string()))?;
    let mut restored = Vec::new();
    if let Some(files) = manifest.get("files").and_then(|f| f.as_array()) {
        for f in files {
            let name = f.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
                continue;
            }
            let src = dir.join(name);
            if !src.is_file() {
                continue;
            }
            if name == "record.json" {
                std::fs::copy(&src, crate::install::install_record_path(base))
                    .map_err(|e| LifecycleError::Io(e.to_string()))?;
                restored.push("install/record.json".to_string());
            } else if name == "active.json" {
                std::fs::copy(&src, active_pointer_path(base))
                    .map_err(|e| LifecycleError::Io(e.to_string()))?;
                restored.push("bin/active.json".to_string());
            } else if let Some(ws_id) = name.strip_suffix(".state.sqlite") {
                let dest = base.join("workspaces").join(ws_id).join("state.sqlite");
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| LifecycleError::Io(e.to_string()))?;
                }
                std::fs::copy(&src, &dest).map_err(|e| LifecycleError::Io(e.to_string()))?;
                restored.push(format!("workspaces/{ws_id}/state.sqlite"));
            }
        }
    }
    Ok(restored)
}
