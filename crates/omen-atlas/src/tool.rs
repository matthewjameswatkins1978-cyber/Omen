use crate::profile::RuntimeProfile;
use omen_core::{CoreError, ToolId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ValidationState {
    Unvalidated,
    Validated,
    StaleValidation,
    Incompatible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ToolUnderstanding {
    Native,
    Adapted,
    Observed,
    Opaque,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInstance {
    pub tool_id: ToolId,
    pub binary_path: PathBuf,
    pub binary_fingerprint: String,
    pub version: Option<String>,
    pub platform: String,
    pub validation_state: ValidationState,
    pub understanding: ToolUnderstanding,
    pub profile: Option<RuntimeProfile>,
    pub last_validated_at: Option<String>,
}

impl ToolInstance {
    /// Checks whether the binary has changed on disk, transitioning to STALE_VALIDATION if so.
    pub fn check_stale(&mut self) -> Result<bool, CoreError> {
        let current_fingerprint = compute_binary_fingerprint(&self.binary_path)?;
        if current_fingerprint != self.binary_fingerprint {
            self.binary_fingerprint = current_fingerprint;
            self.validation_state = ValidationState::StaleValidation;
            return Ok(true);
        }
        Ok(false)
    }
}

pub fn compute_binary_fingerprint(path: &Path) -> Result<String, CoreError> {
    let bytes = fs::read(path).map_err(|e| {
        CoreError::NotFound(format!(
            "Failed to read binary for fingerprinting {:?}: {e}",
            path
        ))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}

pub fn find_binary_on_path(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
            .split(';')
            .map(|s| s.to_lowercase())
            .collect()
    } else {
        vec!["".into()]
    };

    for dir in env::split_paths(&path_var) {
        let direct = dir.join(name);
        if direct.is_file() {
            return Some(direct);
        }

        if cfg!(windows) {
            for ext in &extensions {
                let with_ext = dir.join(format!("{name}{ext}"));
                if with_ext.is_file() {
                    return Some(with_ext);
                }
            }
        }
    }
    None
}
