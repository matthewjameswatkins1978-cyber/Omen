use omen_core::InteractiveSessionId;
use omen_ipc::protocol::{FactInfo, ManagedServiceInfo};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Bounded summary of Git state in the current working directory.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitStatusInfo {
    pub branch: Option<String>,
    pub remote_origin: Option<String>,
    pub modified_files: Vec<String>,
    pub staged_files: Vec<String>,
    pub untracked_files: Vec<String>,
    pub commits_ahead: usize,
    pub commits_behind: usize,
}

/// Bounded summary of a recent process execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionSummary {
    pub execution_id: String,
    pub command: String,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub stdout_artifact: Option<String>,
    pub stderr_artifact: Option<String>,
    pub stdout_excerpt: Option<String>,
    pub stderr_excerpt: Option<String>,
    pub timestamp: String,
}

/// Information about the host environment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvironmentInfo {
    pub os: String,
    pub username: String,
    pub product_version: String,
}

impl Default for EnvironmentInfo {
    fn default() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            username: std::env::var("USERNAME")
                .or_else(|_| std::env::var("USER"))
                .unwrap_or_else(|_| "user".to_string()),
            product_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Canonical, bounded structured state representation of the session for Agent reasoning.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentContext {
    pub workspace_id: String,
    pub workspace_root: PathBuf,
    pub session_id: InteractiveSessionId,
    pub cwd: PathBuf,
    pub git: Option<GitStatusInfo>,
    pub recent_execution: Option<ExecutionSummary>,
    pub recent_failed_execution: Option<ExecutionSummary>,
    pub dirty_facts: Vec<FactInfo>,
    pub current_facts: Vec<FactInfo>,
    pub services: Vec<ManagedServiceInfo>,
    pub available_tools: Vec<String>,
    pub environment: EnvironmentInfo,
}

impl AgentContext {
    pub fn new(
        workspace_id: impl Into<String>,
        workspace_root: PathBuf,
        session_id: InteractiveSessionId,
        cwd: PathBuf,
    ) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            workspace_root,
            session_id,
            cwd,
            git: None,
            recent_execution: None,
            recent_failed_execution: None,
            dirty_facts: Vec::new(),
            current_facts: Vec::new(),
            services: Vec::new(),
            available_tools: Vec::new(),
            environment: EnvironmentInfo::default(),
        }
    }

    /// Truncates string to max bytes on a clean UTF-8 boundary.
    pub fn bounded_excerpt(raw: &str, max_bytes: usize) -> String {
        if raw.len() <= max_bytes {
            return raw.to_string();
        }
        let mut idx = max_bytes;
        while !raw.is_char_boundary(idx) && idx > 0 {
            idx -= 1;
        }
        format!("{}... [truncated]", &raw[..idx])
    }

    /// Inspects the local Git state in the given directory with a bounded duration.
    pub fn detect_git_status(path: &Path) -> Option<GitStatusInfo> {
        let git_dir = path.join(".git");
        if !git_dir.exists() {
            // Check parent directories up to 4 levels
            let mut cur = path.parent();
            let mut found = false;
            for _ in 0..4 {
                if let Some(p) = cur {
                    if p.join(".git").exists() {
                        found = true;
                        break;
                    }
                    cur = p.parent();
                } else {
                    break;
                }
            }
            if !found {
                return None;
            }
        }

        let mut status = GitStatusInfo::default();

        // 1. Get branch name
        if let Ok(out) = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(path)
            .output()
            && out.status.success()
        {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !name.is_empty() {
                status.branch = Some(name);
            }
        }

        // 2. Get remote origin URL
        if let Ok(out) = std::process::Command::new("git")
            .args(["config", "--get", "remote.origin.url"])
            .current_dir(path)
            .output()
            && out.status.success()
        {
            let remote = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !remote.is_empty() {
                status.remote_origin = Some(remote);
            }
        }

        // 3. Get status --porcelain (bounded to first 30 entries)
        if let Ok(out) = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(path)
            .output()
            && out.status.success()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines().take(30) {
                if line.len() < 3 {
                    continue;
                }
                let code = &line[..2];
                let file = line[3..].trim().to_string();
                if code.starts_with('?') {
                    status.untracked_files.push(file);
                } else if code.starts_with('M') || code.starts_with('A') || code.starts_with('D') {
                    status.staged_files.push(file);
                } else if code.ends_with('M') || code.ends_with('D') {
                    status.modified_files.push(file);
                }
            }
        }

        Some(status)
    }
}
