//! Minimal deterministic pinning (H item 12).
//!
//! Pin identity refers to canonical Omen identity (`digest:<hex>`,
//! `slot:<id>`, `fact:<id>`), never a fragile path string. Pinning is
//! retention protection only: not authority, not execution approval, not a
//! backup guarantee.
use crate::error::LifecycleError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const PINS_SCHEMA_VERSION: u32 = 1;
pub const PINS_FILE_NAME: &str = "pins.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinStore {
    pub schema_version: u32,
    pub pins: BTreeSet<String>,
}

impl Default for PinStore {
    fn default() -> Self {
        Self {
            schema_version: PINS_SCHEMA_VERSION,
            pins: BTreeSet::new(),
        }
    }
}

pub fn pins_path(base: &Path) -> PathBuf {
    base.join("pins").join(PINS_FILE_NAME)
}

/// Validate a pin identity: `<namespace>:<value>` with a known namespace.
pub fn validate_pin_identity(identity: &str) -> Result<(), LifecycleError> {
    let (ns, value) = identity.split_once(':').ok_or_else(|| {
        LifecycleError::Refused(format!(
            "pin identity must be <namespace>:<value>: {identity}"
        ))
    })?;
    match ns {
        "digest" | "slot" | "fact" | "receipt" => {
            if value.is_empty() || value.len() > 256 {
                return Err(LifecycleError::Refused(format!(
                    "pin identity value out of range: {identity}"
                )));
            }
            if value.contains('/') || value.contains('\\') || value.contains("..") {
                return Err(LifecycleError::Refused(format!(
                    "pin identity must not contain path separators: {identity}"
                )));
            }
            Ok(())
        }
        _ => Err(LifecycleError::Refused(format!(
            "unknown pin namespace {ns:?}; expected digest|slot|fact|receipt"
        ))),
    }
}

pub fn load_pins(base: &Path) -> Result<PinStore, LifecycleError> {
    let path = pins_path(base);
    if !path.exists() {
        return Ok(PinStore::default());
    }
    let bytes = std::fs::read(&path).map_err(|e| LifecycleError::Io(e.to_string()))?;
    let store: PinStore =
        serde_json::from_slice(&bytes).map_err(|e| LifecycleError::Manifest(e.to_string()))?;
    if store.schema_version != PINS_SCHEMA_VERSION {
        return Err(LifecycleError::Compat(format!(
            "pins schema {} unsupported (expected {PINS_SCHEMA_VERSION})",
            store.schema_version
        )));
    }
    Ok(store)
}

fn save_pins(base: &Path, store: &PinStore) -> Result<(), LifecycleError> {
    let path = pins_path(base);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LifecycleError::Io(e.to_string()))?;
    }
    // Atomic write: temp file + rename, so a crash never leaves half JSON.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(store).unwrap())
        .map_err(|e| LifecycleError::Io(e.to_string()))?;
    std::fs::rename(&tmp, &path).map_err(|e| LifecycleError::Io(e.to_string()))?;
    Ok(())
}

pub fn pin(base: &Path, identity: &str) -> Result<bool, LifecycleError> {
    validate_pin_identity(identity)?;
    let mut store = load_pins(base)?;
    let inserted = store.pins.insert(identity.to_string());
    save_pins(base, &store)?;
    Ok(inserted)
}

pub fn unpin(base: &Path, identity: &str) -> Result<bool, LifecycleError> {
    let mut store = load_pins(base)?;
    let removed = store.pins.remove(identity);
    save_pins(base, &store)?;
    Ok(removed)
}

pub fn is_pinned(base: &Path, identity: &str) -> bool {
    load_pins(base)
        .map(|s| s.pins.contains(identity))
        .unwrap_or(false)
}

/// Map a state identity to the pin namespaces that could protect it.
/// CAS blob paths (`.../cas/sha256/ab/<hex>`) map to `digest:<hex>`.
pub fn pin_identities_for_state_identity(state_identity: &str) -> Vec<String> {
    let mut out = Vec::new();
    let norm = state_identity.replace('\\', "/");
    if let Some(idx) = norm.find("/cas/") {
        let tail = norm[idx + 5..].trim_start_matches("sha256/");
        let hex: String = tail.split('/').collect();
        if !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            out.push(format!("digest:{hex}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_identity_validation() {
        assert!(validate_pin_identity("digest:abc123").is_ok());
        assert!(validate_pin_identity("slot:preview-15").is_ok());
        assert!(validate_pin_identity("fact:x").is_ok());
        assert!(validate_pin_identity("receipt:x").is_ok());
        assert!(validate_pin_identity("path:C:\\x").is_err());
        assert!(validate_pin_identity("nocolon").is_err());
        assert!(validate_pin_identity("digest:../escape").is_err());
        assert!(validate_pin_identity("nope:x").is_err());
    }

    #[test]
    fn cas_path_maps_to_digest_pin() {
        let ids = pin_identities_for_state_identity("workspaces/ws_a/cas/sha256/ab/cdef01");
        assert_eq!(ids, vec!["digest:abcdef01"]);
    }
}
