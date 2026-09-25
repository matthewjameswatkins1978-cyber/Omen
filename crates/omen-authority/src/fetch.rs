//! Managed companion fetch: release-bound, hash-verified, explicit.
//!
//! The companion ships as a SEPARATE release asset (never inside the
//! main update zip — the archive allowlist forbids nested executables,
//! and the previous updater must keep accepting the main package).
//! Binding comes from `omen-release.json` gate fields (ignored by old
//! readers): exact asset name + zip hash + pinned binary hashes +
//! Tethers source SHA.
//!
//! Fetch is explicit (`omen authority fetch-companion`); `exec` refuses
//! with direction when the companion is absent. Every step fails
//! closed: discovery, asset binding, download digest, zip shape,
//! inner hashes, provenance cross-check.

use crate::AuthorityError;
use crate::managed::CompanionProvenance;
use omen_lifecycle::release::{RELEASE_MANIFEST_ASSET, ReleaseManifest};
use omen_lifecycle::transport::ReleaseTransport;

/// Companion zip shape (strict allowlist; depth 1; size caps).
const GATE_EXE_NAMES: [&str; 2] = ["tethers-gate.exe", "tethers-gate"];
const ENGINE_EXE_NAMES: [&str; 2] = ["tethers-engine.exe", "tethers-engine"];
const MAX_COMPANION_ZIP_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024;

/// What fetch installed (all verified).
#[derive(Debug, Clone)]
pub struct FetchedCompanion {
    pub dir: std::path::PathBuf,
    pub provenance: CompanionProvenance,
}

/// Fetch + verify + install the companion for the CURRENT release channel.
///
/// `release_tag`: exact tag to fetch from (e.g. `v0.9.0-preview.22`) —
/// explicit, never "latest". `install_dir`: the companion dir to write
/// (created fresh; existing content replaced only after full verify).
pub fn fetch_companion<T: ReleaseTransport>(
    transport: &T,
    release_tag: &str,
    install_dir: &std::path::Path,
    work_dir: &std::path::Path,
) -> Result<FetchedCompanion, AuthorityError> {
    use omen_lifecycle::release::validate_update_url;

    // 1. Discover the release + its canonical manifest asset.
    let releases = transport
        .list_releases(20)
        .map_err(|e| AuthorityError::Discover(format!("companion.discover.failed: {e:?}")))?;
    let rel = releases
        .iter()
        .find(|r| r.tag == release_tag)
        .ok_or_else(|| AuthorityError::Discover(format!("companion.tag_missing: {release_tag}")))?;
    let manifests: Vec<_> = rel
        .assets
        .iter()
        .filter(|a| a.name == RELEASE_MANIFEST_ASSET)
        .collect();
    if manifests.len() != 1 {
        return Err(AuthorityError::Discover(format!(
            "companion.manifest_assets: {} (want exactly 1)",
            manifests.len()
        )));
    }
    let manifest_url = validate_update_url(&manifests[0].browser_download_url)
        .map(|u| u.to_string())
        .map_err(|e| AuthorityError::Discover(format!("companion.manifest_url: {e}")))?;
    let bytes = transport
        .fetch_manifest_bytes(&manifest_url)
        .map_err(|e| AuthorityError::Discover(format!("companion.manifest_fetch: {e:?}")))?;
    let manifest = ReleaseManifest::parse_strict(&bytes)
        .map_err(|e| AuthorityError::Discover(format!("companion.manifest_invalid: {e:?}")))?;
    if manifest.version != release_tag.strip_prefix('v').unwrap_or(release_tag) {
        return Err(AuthorityError::Discover(format!(
            "companion.tag_manifest_mismatch: {} vs {}",
            manifest.version, release_tag
        )));
    }

    // 2. Gate binding must be present (all-or-nothing validated by shape).
    let (asset_name, zip_sha, tethers_sha, gate_sha, engine_sha) = match (
        &manifest.gate_companion_asset,
        &manifest.gate_companion_sha256,
        &manifest.gate_tethers_sha,
        &manifest.gate_exe_sha256,
        &manifest.gate_engine_sha256,
    ) {
        (Some(a), Some(z), Some(t), Some(g), Some(e)) => (a, z, t, g, e),
        _ => {
            return Err(AuthorityError::Discover(
                "companion.unbound: release manifest names no gate companion".to_string(),
            ));
        }
    };
    let pkgs: Vec<_> = rel
        .assets
        .iter()
        .filter(|a| &a.name == asset_name)
        .collect();
    if pkgs.len() != 1 {
        return Err(AuthorityError::Discover(format!(
            "companion.asset_count: {} (want exactly 1)",
            pkgs.len()
        )));
    }
    let download_url = validate_update_url(&pkgs[0].browser_download_url)
        .map(|u| u.to_string())
        .map_err(|e| AuthorityError::Discover(format!("companion.asset_url: {e}")))?;

    // 3. Download with running digest; the part renames only on match.
    std::fs::create_dir_all(work_dir)
        .map_err(|e| AuthorityError::Discover(format!("companion.workdir: {e}")))?;
    let part = work_dir.join(format!("{asset_name}.part"));
    let actual = transport
        .download_package(&download_url, &part, MAX_COMPANION_ZIP_BYTES)
        .map_err(|e| AuthorityError::Discover(format!("companion.download: {e:?}")))?;
    if actual.to_lowercase() != zip_sha.to_lowercase() {
        let _ = std::fs::remove_file(&part);
        return Err(AuthorityError::Discover(format!(
            "companion.zip_digest_mismatch: {actual} != {zip_sha}"
        )));
    }
    let final_zip = work_dir.join(asset_name);
    std::fs::rename(&part, &final_zip)
        .map_err(|e| AuthorityError::Discover(format!("companion.rename: {e}")))?;

    // 4. Strict extract into a staging dir (never directly over live).
    let stage = work_dir.join("companion-stage");
    if stage.exists() {
        std::fs::remove_dir_all(&stage)
            .map_err(|e| AuthorityError::Discover(format!("companion.stage_clean: {e}")))?;
    }
    std::fs::create_dir_all(&stage)
        .map_err(|e| AuthorityError::Discover(format!("companion.stage: {e}")))?;
    extract_companion_zip(&final_zip, &stage)?;

    // 5. Inner hashes vs manifest pins + provenance cross-check.
    let gate_name = pick_present(&stage, &GATE_EXE_NAMES)?;
    let engine_name = pick_present(&stage, &ENGINE_EXE_NAMES)?;
    verify_file_sha(&stage.join(&gate_name), gate_sha, "gate")?;
    verify_file_sha(&stage.join(&engine_name), engine_sha, "engine")?;
    let prov_text = std::fs::read_to_string(stage.join("provenance.json"))
        .map_err(|e| AuthorityError::Discover(format!("companion.provenance_missing: {e}")))?;
    let provenance: CompanionProvenance = serde_json::from_str(&prov_text)
        .map_err(|e| AuthorityError::Discover(format!("companion.provenance_malformed: {e}")))?;
    if provenance.tethers_source_sha.to_lowercase() != tethers_sha.to_lowercase()
        || provenance.gate_exe_sha256.to_lowercase() != gate_sha.to_lowercase()
        || provenance.engine_exe_sha256.to_lowercase() != engine_sha.to_lowercase()
        || provenance.gate_exe_name != gate_name
        || provenance.engine_exe_name != engine_name
    {
        return Err(AuthorityError::Discover(
            "companion.provenance_binding_mismatch".to_string(),
        ));
    }

    // 6. Commit: replace the live dir atomically-ish (remove + rename;
    // failure leaves the previous install or nothing — exec refuses
    // either way until a verified companion exists).
    if install_dir.exists() {
        std::fs::remove_dir_all(install_dir)
            .map_err(|e| AuthorityError::Discover(format!("companion.install_clean: {e}")))?;
    }
    if let Some(parent) = install_dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AuthorityError::Discover(format!("companion.install_parent: {e}")))?;
    }
    std::fs::rename(&stage, install_dir)
        .map_err(|e| AuthorityError::Discover(format!("companion.install: {e}")))?;
    let _ = std::fs::remove_file(&final_zip);
    Ok(FetchedCompanion {
        dir: install_dir.to_path_buf(),
        provenance,
    })
}

fn pick_present(dir: &std::path::Path, names: &[&str]) -> Result<String, AuthorityError> {
    for n in names {
        if dir.join(n).is_file() {
            return Ok(n.to_string());
        }
    }
    Err(AuthorityError::Discover(format!(
        "companion.missing_binary: one of {names:?}"
    )))
}

fn verify_file_sha(
    path: &std::path::Path,
    expected: &str,
    what: &str,
) -> Result<(), AuthorityError> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path)
        .map_err(|e| AuthorityError::Discover(format!("companion.{what}_unreadable: {e}")))?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| AuthorityError::Discover(format!("companion.{what}_read: {e}")))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    let actual = format!("{:x}", h.finalize());
    if actual.to_lowercase() != expected.to_lowercase() {
        return Err(AuthorityError::Discover(format!(
            "companion.{what}_hash_mismatch: {actual} != {expected}"
        )));
    }
    Ok(())
}

/// Strict companion-zip extraction: exactly the two binaries +
/// provenance.json at top level, no traversal, no nesting, size caps.
fn extract_companion_zip(
    zip_path: &std::path::Path,
    dest: &std::path::Path,
) -> Result<(), AuthorityError> {
    let file = std::fs::File::open(zip_path)
        .map_err(|e| AuthorityError::Discover(format!("companion.zip_open: {e}")))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| AuthorityError::Discover(format!("companion.zip_invalid: {e}")))?;
    let mut seen = std::collections::BTreeSet::new();
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| AuthorityError::Discover(format!("companion.zip_entry: {e}")))?;
        let name = entry.name().to_string();
        if name.contains('\\') || name.starts_with('/') || name.starts_with("..") {
            return Err(AuthorityError::Discover(format!(
                "companion.zip_escaped: {name}"
            )));
        }
        if name.contains('/') {
            return Err(AuthorityError::Discover(format!(
                "companion.zip_nested: {name}"
            )));
        }
        let allowed = name == "provenance.json"
            || GATE_EXE_NAMES.contains(&name.as_str())
            || ENGINE_EXE_NAMES.contains(&name.as_str());
        if !allowed {
            return Err(AuthorityError::Discover(format!(
                "companion.zip_unexpected: {name}"
            )));
        }
        if entry.size() > MAX_ENTRY_BYTES {
            return Err(AuthorityError::Discover(format!(
                "companion.zip_oversize: {name}"
            )));
        }
        if !seen.insert(name.clone()) {
            return Err(AuthorityError::Discover(format!(
                "companion.zip_duplicate: {name}"
            )));
        }
        let out_path = dest.join(&name);
        let mut out = std::fs::File::create(&out_path)
            .map_err(|e| AuthorityError::Discover(format!("companion.zip_write: {e}")))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| AuthorityError::Discover(format!("companion.zip_copy: {e}")))?;
    }
    for required in ["provenance.json"] {
        if !seen.contains(required) {
            return Err(AuthorityError::Discover(format!(
                "companion.zip_missing: {required}"
            )));
        }
    }
    if !GATE_EXE_NAMES.iter().any(|n| seen.contains(*n)) {
        return Err(AuthorityError::Discover(
            "companion.zip_missing: gate executable".to_string(),
        ));
    }
    if !ENGINE_EXE_NAMES.iter().any(|n| seen.contains(*n)) {
        return Err(AuthorityError::Discover(
            "companion.zip_missing: engine executable".to_string(),
        ));
    }
    Ok(())
}
