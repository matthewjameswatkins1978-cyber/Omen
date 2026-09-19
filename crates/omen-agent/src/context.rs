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
    pub probe_error: Option<String>,
    pub blocked_phase: Option<String>,
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
    pub git_diagnostic: Option<String>,
    pub recent_execution: Option<ExecutionSummary>,
    pub recent_failed_execution: Option<ExecutionSummary>,
    pub dirty_facts: Vec<FactInfo>,
    pub current_facts: Vec<FactInfo>,
    pub services: Vec<ManagedServiceInfo>,
    pub available_tools: Vec<String>,
    pub environment: EnvironmentInfo,
}

/// Synchronously blocks on an async future across multithread and single-thread Tokio contexts.
pub fn block_on_async<F>(future: F) -> F::Output
where
    F: std::future::Future + Send,
    F::Output: Send,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        match handle.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| handle.block_on(future))
            }
            _ => std::thread::scope(|s| {
                s.spawn(|| {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("Failed to build local tokio runtime")
                        .block_on(future)
                })
                .join()
                .expect("Tokio runtime thread panicked")
            }),
        }
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build local tokio runtime")
            .block_on(future)
    }
}

fn run_bounded_probe(
    supervisor: &omen_engine::ProcessSupervisor,
    bin: &str,
    args: &[&str],
    cwd: &Path,
    timeout_ms: u64,
) -> Result<Vec<u8>, (String, String)> {
    let mut argv = vec![bin.to_string()];
    argv.extend(args.iter().map(|s| s.to_string()));
    let req = omen_engine::ExecutionRequest {
        argv: argv.clone(),
        cwd: cwd.to_path_buf(),
        env: vec![],
        stdin_mode: omen_core::StdioMode::Closed,
        stdin_payload: None,
        timeout_ms,
        inline_budget: 8192,
        required_assurance: omen_core::RequiredAssurance::default(),
    };

    let out = block_on_async(supervisor.execute(req))
        .map_err(|e| (argv.join(" "), format!("Supervisor execution failed: {e}")))?;

    if out.runtime_status == omen_core::RuntimeStatus::TimedOut {
        return Err((
            argv.join(" "),
            format!("Probe timed out after {timeout_ms}ms"),
        ));
    }

    if !out.process_exit.is_zero() {
        return Err((
            argv.join(" "),
            format!("Process exited with status {:?}", out.process_exit.code),
        ));
    }

    Ok(out.stdout_all)
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
            git_diagnostic: None,
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

    /// Inspects the local Git state in the given directory with a bounded duration (default 1500ms).
    pub fn detect_git_status(path: &Path) -> Option<GitStatusInfo> {
        Self::detect_git_status_with_opts(path, "git", 1500)
    }

    /// Inspects Git state using a specified executable binary and bounded timeout in milliseconds.
    pub fn detect_git_status_with_opts(
        path: &Path,
        git_bin: &str,
        timeout_ms: u64,
    ) -> Option<GitStatusInfo> {
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

        let supervisor = omen_engine::ProcessSupervisor::new();
        let mut status = GitStatusInfo::default();

        // 1. Get branch name
        match run_bounded_probe(
            &supervisor,
            git_bin,
            &["rev-parse", "--abbrev-ref", "HEAD"],
            path,
            timeout_ms,
        ) {
            Ok(bytes) => {
                let name = String::from_utf8_lossy(&bytes).trim().to_string();
                if !name.is_empty() {
                    status.branch = Some(name);
                }
            }
            Err((phase, err)) => {
                status.blocked_phase = Some(phase);
                status.probe_error = Some(err);
                return Some(status);
            }
        }

        // 2. Get remote origin URL
        if let Ok(bytes) = run_bounded_probe(
            &supervisor,
            git_bin,
            &["config", "--get", "remote.origin.url"],
            path,
            timeout_ms,
        ) {
            let remote = String::from_utf8_lossy(&bytes).trim().to_string();
            if !remote.is_empty() {
                status.remote_origin = Some(remote);
            }
        }

        // 3. Get status --porcelain (bounded to first 30 entries)
        match run_bounded_probe(
            &supervisor,
            git_bin,
            &["status", "--porcelain"],
            path,
            timeout_ms,
        ) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                for line in text.lines().take(30) {
                    if line.len() < 3 {
                        continue;
                    }
                    let code = &line[..2];
                    let file = line[3..].trim().to_string();
                    if code.starts_with('?') {
                        status.untracked_files.push(file);
                    } else if code.starts_with('M')
                        || code.starts_with('A')
                        || code.starts_with('D')
                    {
                        status.staged_files.push(file);
                    } else if code.ends_with('M') || code.ends_with('D') {
                        status.modified_files.push(file);
                    }
                }
            }
            Err((phase, err)) => {
                if status.probe_error.is_none() {
                    status.blocked_phase = Some(phase);
                    status.probe_error = Some(err);
                }
            }
        }

        Some(status)
    }
}
