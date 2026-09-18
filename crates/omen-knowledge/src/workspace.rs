use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Computes a deterministic workspace ID from the canonical root path.
pub fn deterministic_workspace_id(root: &Path) -> String {
    let canonical = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/");
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let hex_str = hex::encode(hasher.finalize());
    hex_str[..16].to_string()
}

/// Resolves the Omen state directory for a given workspace.
pub fn resolve_workspace_dir(root: &Path) -> PathBuf {
    let base_dir = if let Ok(custom) = env::var("OMEN_STATE_HOME") {
        PathBuf::from(custom)
    } else if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        PathBuf::from(local_app_data).join("Omen")
    } else if let Ok(home) = env::var("HOME") {
        PathBuf::from(home).join(".omen")
    } else {
        PathBuf::from(".omen-state")
    };

    let ws_id = deterministic_workspace_id(root);
    let ws_dir = base_dir.join("workspaces").join(ws_id);
    fs::create_dir_all(&ws_dir).expect("Failed to create workspace state directory");
    ws_dir
}
