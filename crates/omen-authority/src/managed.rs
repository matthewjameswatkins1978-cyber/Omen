//! Managed Gate companion: Omen resolves ONLY the release-pinned Gate.
//!
//! The Omen release owns a pinned Tethers Gate companion (binary +
//! engine + provenance). Resolution verifies every hash before the
//! companion is used; anything else (PATH lookups, ambient binaries,
//! unpinned files) is refused. No opaque binaries: provenance records
//! the exact Tethers source SHA the companion was built from.

use crate::AuthorityError;
use crate::protocol::{AUTHORITY_PROTOCOL, TETHERS_PRODUCT_VERSION};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;

/// Release-owned companion provenance (written by packaging, verified
/// at resolve time).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanionProvenance {
    pub schema: String,
    pub tethers_source_sha: String,
    pub gate_exe_sha256: String,
    pub engine_exe_sha256: String,
    pub protocol: String,
    pub product_version: String,
    pub gate_exe_name: String,
    pub engine_exe_name: String,
}

/// Resolved companion paths (all absolute, all hash-verified).
#[derive(Debug, Clone)]
pub struct CompanionPaths {
    pub gate_exe: std::path::PathBuf,
    pub engine_exe: std::path::PathBuf,
    pub dir: std::path::PathBuf,
}

/// A resolved, verified companion ready to supervise.
#[derive(Debug, Clone)]
pub struct GateCompanion {
    pub paths: CompanionPaths,
    pub provenance: CompanionProvenance,
}

/// Resolve the managed companion in `companion_dir` (expects
/// `provenance.json` + the pinned binaries directly inside).
/// No fallback: missing or mismatched companion is `Discover` failure.
pub fn resolve_companion(companion_dir: &std::path::Path) -> Result<GateCompanion, AuthorityError> {
    let dir = companion_dir;
    let prov_path = dir.join("provenance.json");
    let text = std::fs::read_to_string(&prov_path)
        .map_err(|e| AuthorityError::Discover(format!("companion.missing: {prov_path:?}: {e}")))?;
    let provenance: CompanionProvenance = serde_json::from_str(&text)
        .map_err(|e| AuthorityError::Discover(format!("companion.provenance_malformed: {e}")))?;
    if provenance.protocol != AUTHORITY_PROTOCOL {
        return Err(AuthorityError::Discover(format!(
            "companion.protocol_mismatch: {}",
            provenance.protocol
        )));
    }
    if provenance.product_version != TETHERS_PRODUCT_VERSION {
        return Err(AuthorityError::Discover(format!(
            "companion.product_mismatch: {}",
            provenance.product_version
        )));
    }
    let gate_exe = dir.join(&provenance.gate_exe_name);
    let engine_exe = dir.join(&provenance.engine_exe_name);
    verify_hash(&gate_exe, &provenance.gate_exe_sha256, "gate")?;
    verify_hash(&engine_exe, &provenance.engine_exe_sha256, "engine")?;
    Ok(GateCompanion {
        paths: CompanionPaths {
            gate_exe,
            engine_exe,
            dir: dir.to_path_buf(),
        },
        provenance,
    })
}

/// Write a provenance file (packaging side). Hashes are computed from
/// the exact binaries shipped.
pub fn write_provenance(
    dir: &std::path::Path,
    tethers_source_sha: &str,
    gate_exe_name: &str,
    engine_exe_name: &str,
) -> Result<CompanionProvenance, AuthorityError> {
    let gate_exe_sha256 = hash_file(&dir.join(gate_exe_name))
        .map_err(|e| AuthorityError::Discover(format!("companion.hash_failed: {e}")))?;
    let engine_exe_sha256 = hash_file(&dir.join(engine_exe_name))
        .map_err(|e| AuthorityError::Discover(format!("companion.hash_failed: {e}")))?;
    let provenance = CompanionProvenance {
        schema: "omen.gate-companion/1".to_string(),
        tethers_source_sha: tethers_source_sha.to_string(),
        gate_exe_sha256,
        engine_exe_sha256,
        protocol: AUTHORITY_PROTOCOL.to_string(),
        product_version: TETHERS_PRODUCT_VERSION.to_string(),
        gate_exe_name: gate_exe_name.to_string(),
        engine_exe_name: engine_exe_name.to_string(),
    };
    std::fs::write(
        dir.join("provenance.json"),
        serde_json::to_string_pretty(&provenance).unwrap(),
    )
    .map_err(|e| AuthorityError::Discover(format!("companion.write_failed: {e}")))?;
    Ok(provenance)
}

fn verify_hash(path: &std::path::Path, expected: &str, what: &str) -> Result<(), AuthorityError> {
    let actual = hash_file(path)
        .map_err(|e| AuthorityError::Discover(format!("companion.{what}_unreadable: {e}")))?;
    if actual != expected {
        return Err(AuthorityError::Discover(format!(
            "companion.{what}_hash_mismatch: {actual} != {expected}"
        )));
    }
    Ok(())
}

fn hash_file(path: &std::path::Path) -> Result<String, std::io::Error> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
