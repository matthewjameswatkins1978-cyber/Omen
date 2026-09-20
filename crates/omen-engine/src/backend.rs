use crate::pty::NativePtyHandle;
use crate::supervisor::ExecutionRequest;
use omen_core::{
    Assurance, BackendId, CoreError, EnforcementLevel, ProcessExit, PtySessionId,
    RequiredAssurance, RuntimeStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Backend kind classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BackendKind {
    NativeHost,
    Wsl,
    Container,
    MicroVm,
    Remote,
}

/// Availability state of an execution backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BackendAvailability {
    Available,
    Unavailable { reason: String },
    Unsupported { reason: String },
}

/// Capabilities discovered and supported by the platform execution backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendCapabilities {
    pub filesystem: EnforcementLevel,
    pub network: EnforcementLevel,
    pub descendants: EnforcementLevel,
    pub symlink_escape: EnforcementLevel,
    pub pty: bool,
}

impl BackendCapabilities {
    pub fn filesystem_assurance(&self) -> Assurance {
        self.filesystem.to_assurance()
    }

    pub fn network_assurance(&self) -> Assurance {
        self.network.to_assurance()
    }

    pub fn descendants_assurance(&self) -> Assurance {
        self.descendants.to_assurance()
    }
}

/// Machine-readable descriptor of an execution backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendDescriptor {
    pub id: BackendId,
    pub name: String,
    pub kind: BackendKind,
    pub availability: BackendAvailability,
    pub capabilities: BackendCapabilities,
}

pub type ExecutionWaitResult = Result<(RuntimeStatus, ProcessExit, Vec<u8>, Vec<u8>), CoreError>;
pub type ExecutionWaitFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = ExecutionWaitResult> + Send>>;

/// Handle to a running process spawned by an ExecutionBackend.
pub trait ExecutionHandle: Send {
    fn pid(&self) -> Option<u32>;
    fn terminate_tree(&mut self) -> Result<(), CoreError>;
    fn wait_bounded(self: Box<Self>, timeout: Duration) -> ExecutionWaitFuture;
}

/// Request to spawn a PTY interactive process.
#[derive(Debug, Clone)]
pub struct PtyExecutionRequest {
    pub session_id: PtySessionId,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub rows: u16,
    pub cols: u16,
}

/// Handle to an active PTY session.
pub trait PtyExecutionHandle: Send {
    fn session_id(&self) -> PtySessionId;
    fn pid(&self) -> Option<u32>;
    fn write_input(&mut self, data: &[u8]) -> Result<(), CoreError>;
    fn read_output(&mut self) -> Result<Vec<u8>, CoreError>;
    fn read_output_from(&mut self, offset: usize) -> Result<(Vec<u8>, usize), CoreError>;
    fn resize(&mut self, rows: u16, cols: u16) -> Result<(), CoreError>;
    fn terminate(&mut self) -> Result<(), CoreError>;
    fn try_wait(&mut self) -> Result<Option<ProcessExit>, CoreError>;
}

/// Pluggable physical execution backend trait.
pub trait ExecutionBackend: Send + Sync {
    fn id(&self) -> BackendId;
    fn descriptor(&self) -> BackendDescriptor;
    fn capabilities(&self) -> BackendCapabilities {
        self.descriptor().capabilities
    }

    /// Preflight check: reject before physical execution if required assurance exceeds capabilities.
    fn preflight(&self, required: &RequiredAssurance) -> Result<(), CoreError> {
        let caps = self.capabilities();

        if required.filesystem == Some(Assurance::Enforced)
            && caps.filesystem != EnforcementLevel::Enforced
        {
            return Err(CoreError::AssuranceNotSatisfied {
                required: "Enforced".into(),
                available: format!("{:?}", caps.filesystem),
            });
        }

        if required.network == Some(Assurance::Enforced)
            && caps.network != EnforcementLevel::Enforced
        {
            return Err(CoreError::AssuranceNotSatisfied {
                required: "Enforced".into(),
                available: format!("{:?}", caps.network),
            });
        }

        if required.descendants == Some(Assurance::Enforced)
            && caps.descendants != EnforcementLevel::Enforced
        {
            return Err(CoreError::AssuranceNotSatisfied {
                required: "Enforced".into(),
                available: format!("{:?}", caps.descendants),
            });
        }

        Ok(())
    }

    fn spawn(&self, req: &ExecutionRequest) -> Result<Box<dyn ExecutionHandle>, CoreError>;
    fn spawn_pty(
        &self,
        req: &PtyExecutionRequest,
    ) -> Result<Box<dyn PtyExecutionHandle>, CoreError>;
}

/// Physical process handle for native host OS processes.
pub struct NativeExecutionHandle {
    pub pid: Option<u32>,
    pub child: Option<tokio::process::Child>,
    pub stdout: Option<tokio::process::ChildStdout>,
    pub stderr: Option<tokio::process::ChildStderr>,
    #[cfg(windows)]
    pub job_guard: Option<crate::platform::windows::JobObjectGuard>,
    #[cfg(unix)]
    pub pgid: Option<u32>,
}

impl ExecutionHandle for NativeExecutionHandle {
    fn pid(&self) -> Option<u32> {
        self.pid
    }

    fn terminate_tree(&mut self) -> Result<(), CoreError> {
        #[cfg(windows)]
        if let Some(jg) = &self.job_guard {
            jg.terminate(1);
        }
        #[cfg(unix)]
        if let Some(pid_val) = self
            .pgid
            .and_then(|pgid| rustix::process::Pid::from_raw(pgid as i32))
        {
            let _ = rustix::process::kill_process_group(pid_val, rustix::process::Signal::KILL);
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
        Ok(())
    }

    fn wait_bounded(mut self: Box<Self>, timeout: Duration) -> ExecutionWaitFuture {
        Box::pin(async move {
            let stdout_pipe = self.stdout.take();
            let stderr_pipe = self.stderr.take();

            let stdout_task = tokio::spawn(async move {
                if let Some(mut pipe) = stdout_pipe {
                    let mut buf = Vec::new();
                    let _ = tokio::io::AsyncReadExt::read_to_end(&mut pipe, &mut buf).await;
                    buf
                } else {
                    Vec::new()
                }
            });

            let stderr_task = tokio::spawn(async move {
                if let Some(mut pipe) = stderr_pipe {
                    let mut buf = Vec::new();
                    let _ = tokio::io::AsyncReadExt::read_to_end(&mut pipe, &mut buf).await;
                    buf
                } else {
                    Vec::new()
                }
            });

            let wait_res = if let Some(mut child) = self.child.take() {
                let res = tokio::time::timeout(timeout, child.wait()).await;
                match res {
                    Ok(Ok(status)) => (
                        RuntimeStatus::Completed,
                        ProcessExit {
                            code: status.code(),
                            signal: None,
                        },
                    ),
                    Ok(Err(e)) => (
                        RuntimeStatus::IoFailed,
                        ProcessExit {
                            code: None,
                            signal: Some(e.to_string()),
                        },
                    ),
                    Err(_) => {
                        #[cfg(windows)]
                        if let Some(jg) = &self.job_guard {
                            jg.terminate(1);
                        }
                        #[cfg(unix)]
                        if let Some(pid_val) = self
                            .pgid
                            .and_then(|pgid| rustix::process::Pid::from_raw(pgid as i32))
                        {
                            let _ = rustix::process::kill_process_group(
                                pid_val,
                                rustix::process::Signal::KILL,
                            );
                        }
                        let _ = child.start_kill();
                        (
                            RuntimeStatus::TimedOut,
                            ProcessExit {
                                code: None,
                                signal: Some("SIGKILL_TIMEOUT".into()),
                            },
                        )
                    }
                }
            } else {
                (
                    RuntimeStatus::Completed,
                    ProcessExit {
                        code: Some(0),
                        signal: None,
                    },
                )
            };

            let stdout_all = tokio::time::timeout(Duration::from_millis(500), stdout_task)
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();
            let stderr_all = tokio::time::timeout(Duration::from_millis(500), stderr_task)
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();

            Ok((wait_res.0, wait_res.1, stdout_all, stderr_all))
        })
    }
}

/// Native host execution backend.
pub struct NativeExecutionBackend;

impl NativeExecutionBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NativeExecutionBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutionBackend for NativeExecutionBackend {
    fn id(&self) -> BackendId {
        BackendId::native()
    }

    fn descriptor(&self) -> BackendDescriptor {
        #[cfg(windows)]
        let capabilities = BackendCapabilities {
            filesystem: EnforcementLevel::Observed,
            network: EnforcementLevel::Observed,
            descendants: EnforcementLevel::Enforced, // Windows Job Objects guarantee descendant termination
            symlink_escape: EnforcementLevel::Enforced,
            pty: true,
        };

        #[cfg(target_os = "linux")]
        let capabilities = BackendCapabilities {
            filesystem: EnforcementLevel::Observed,
            network: EnforcementLevel::Observed,
            descendants: EnforcementLevel::BestEffort,
            symlink_escape: EnforcementLevel::Enforced,
            pty: true,
        };

        #[cfg(not(any(windows, target_os = "linux")))]
        let capabilities = BackendCapabilities {
            filesystem: EnforcementLevel::Observed,
            network: EnforcementLevel::Observed,
            descendants: EnforcementLevel::BestEffort,
            symlink_escape: EnforcementLevel::Enforced,
            pty: true,
        };

        BackendDescriptor {
            id: self.id(),
            name: "Native Host".to_string(),
            kind: BackendKind::NativeHost,
            availability: BackendAvailability::Available,
            capabilities,
        }
    }

    fn spawn(&self, req: &ExecutionRequest) -> Result<Box<dyn ExecutionHandle>, CoreError> {
        if req.argv.is_empty() {
            return Err(CoreError::ExecutionFailed("Argv cannot be empty".into()));
        }

        let mut cmd = tokio::process::Command::new(&req.argv[0]);
        if req.argv.len() > 1 {
            cmd.args(&req.argv[1..]);
        }
        cmd.current_dir(&req.cwd);

        for (k, v) in &req.env {
            cmd.env(k, v);
        }

        // Secret injection into environment variables
        for secret in &req.secrets {
            if let omen_core::SecretInjectionContract::EnvironmentVariable { name } =
                &secret.contract
            {
                cmd.env(name, &secret.value);
            }
        }

        // Stdio setup
        match req.stdin_mode {
            omen_core::StdioMode::Closed | omen_core::StdioMode::Inline => {
                cmd.stdin(std::process::Stdio::piped());
            }
            omen_core::StdioMode::Inherit => {
                cmd.stdin(std::process::Stdio::inherit());
            }
            _ => {
                cmd.stdin(std::process::Stdio::null());
            }
        }

        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        #[cfg(unix)]
        cmd.process_group(0);

        #[cfg(windows)]
        let job_guard = crate::platform::windows::JobObjectGuard::new().map_err(|e| {
            CoreError::ExecutionFailed(format!("Job object initialization error: {e}"))
        })?;

        let mut child = cmd.spawn().map_err(|e| {
            CoreError::ExecutionFailed(format!("Process spawn failed for '{}': {e}", req.argv[0]))
        })?;

        let pid = child.id();

        #[cfg(windows)]
        if let Some(p) = pid {
            job_guard.assign_pid(p).map_err(|e| {
                let _ = child.start_kill();
                CoreError::ExecutionFailed(format!("Failed to assign PID {p} to Job Object: {e}"))
            })?;
        }

        // Write stdin payload and stdin secrets
        if let Some(mut stdin) = child.stdin.take() {
            let mut stdin_bytes = req.stdin_payload.clone().unwrap_or_default();
            for secret in &req.secrets {
                if let omen_core::SecretInjectionContract::Stdin = &secret.contract {
                    stdin_bytes.extend_from_slice(secret.value.as_bytes());
                    stdin_bytes.push(b'\n');
                }
            }
            if !stdin_bytes.is_empty() {
                tokio::spawn(async move {
                    use tokio::io::AsyncWriteExt;
                    let _ = stdin.write_all(&stdin_bytes).await;
                    drop(stdin);
                });
            } else {
                drop(stdin);
            }
        }

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        Ok(Box::new(NativeExecutionHandle {
            pid,
            child: Some(child),
            stdout,
            stderr,
            #[cfg(windows)]
            job_guard: Some(job_guard),
            #[cfg(unix)]
            pgid: pid,
        }))
    }

    fn spawn_pty(
        &self,
        req: &PtyExecutionRequest,
    ) -> Result<Box<dyn PtyExecutionHandle>, CoreError> {
        let handle = NativePtyHandle::spawn(req)?;
        Ok(Box::new(handle))
    }
}

/// Converts a Windows or host path into a deterministic WSL path (e.g. `D:\Omen Shell` -> `/mnt/d/Omen Shell`).
pub fn to_wsl_path(path: &Path) -> String {
    let path_str = path.to_string_lossy().replace('\\', "/");
    let bytes = path_str.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = &path_str[2..];
        let rest_trimmed = rest.trim_start_matches('/');
        format!("/mnt/{drive}/{rest_trimmed}")
    } else {
        path_str
    }
}

/// Windows Subsystem for Linux (WSL) execution backend.
pub struct WslExecutionBackend;

impl WslExecutionBackend {
    pub fn new() -> Self {
        Self
    }

    pub fn is_available() -> bool {
        #[cfg(windows)]
        {
            // Probe wsl.exe -e true with a hard 1500ms deadline, killing child on timeout to prevent hanging.
            let mut child = match std::process::Command::new("wsl.exe")
                .arg("-e")
                .arg("true")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(c) => c,
                Err(_) => return false,
            };

            let start = std::time::Instant::now();
            let deadline = std::time::Duration::from_millis(6000);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => return status.success(),
                    Ok(None) => {
                        if start.elapsed() >= deadline {
                            let _ = child.kill();
                            let _ = child.wait();
                            return false;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(_) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return false;
                    }
                }
            }
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

impl Default for WslExecutionBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutionBackend for WslExecutionBackend {
    fn id(&self) -> BackendId {
        BackendId::wsl()
    }

    fn descriptor(&self) -> BackendDescriptor {
        let available = Self::is_available();
        let availability = if available {
            BackendAvailability::Available
        } else {
            BackendAvailability::Unavailable {
                reason: "WSL2 environment not detected on this system".into(),
            }
        };

        BackendDescriptor {
            id: self.id(),
            name: "Windows Subsystem for Linux".to_string(),
            kind: BackendKind::Wsl,
            availability,
            capabilities: BackendCapabilities {
                filesystem: EnforcementLevel::Mediated,
                network: EnforcementLevel::Mediated,
                descendants: EnforcementLevel::Mediated,
                symlink_escape: EnforcementLevel::Enforced,
                pty: true,
            },
        }
    }

    fn spawn(&self, req: &ExecutionRequest) -> Result<Box<dyn ExecutionHandle>, CoreError> {
        if !Self::is_available() {
            return Err(CoreError::ExecutionFailed(
                "WSL backend is unavailable on this system".into(),
            ));
        }

        let wsl_cwd = to_wsl_path(&req.cwd);
        let mut wsl_argv = vec![
            "wsl.exe".to_string(),
            "--cd".to_string(),
            wsl_cwd,
            "-e".to_string(),
        ];
        wsl_argv.extend(req.argv.clone());

        let mut translated_req = req.clone();
        translated_req.argv = wsl_argv;

        NativeExecutionBackend::new().spawn(&translated_req)
    }

    fn spawn_pty(
        &self,
        req: &PtyExecutionRequest,
    ) -> Result<Box<dyn PtyExecutionHandle>, CoreError> {
        if !Self::is_available() {
            return Err(CoreError::ExecutionFailed(
                "WSL backend is unavailable on this system".into(),
            ));
        }

        let wsl_cwd = to_wsl_path(&req.cwd);
        let mut wsl_argv = vec![
            "wsl.exe".to_string(),
            "--cd".to_string(),
            wsl_cwd,
            "-e".to_string(),
        ];
        wsl_argv.extend(req.argv.clone());

        let mut translated_req = req.clone();
        translated_req.argv = wsl_argv;

        NativeExecutionBackend::new().spawn_pty(&translated_req)
    }
}

/// Fallback portable execution backend.
pub struct PortableExecutionBackend;

impl ExecutionBackend for PortableExecutionBackend {
    fn id(&self) -> BackendId {
        BackendId::new("portable").unwrap_or_else(|_| BackendId::native())
    }

    fn descriptor(&self) -> BackendDescriptor {
        BackendDescriptor {
            id: self.id(),
            name: "Portable Host".into(),
            kind: BackendKind::NativeHost,
            availability: BackendAvailability::Available,
            capabilities: BackendCapabilities {
                filesystem: EnforcementLevel::Observed,
                network: EnforcementLevel::Observed,
                descendants: EnforcementLevel::Observed,
                symlink_escape: EnforcementLevel::Observed,
                pty: true,
            },
        }
    }

    fn spawn(&self, req: &ExecutionRequest) -> Result<Box<dyn ExecutionHandle>, CoreError> {
        NativeExecutionBackend::new().spawn(req)
    }

    fn spawn_pty(
        &self,
        req: &PtyExecutionRequest,
    ) -> Result<Box<dyn PtyExecutionHandle>, CoreError> {
        NativeExecutionBackend::new().spawn_pty(req)
    }
}

/// Registry of available execution backends.
pub struct BackendRegistry {
    backends: RwLock<HashMap<BackendId, Arc<dyn ExecutionBackend>>>,
    active_backend: RwLock<BackendId>,
}

impl BackendRegistry {
    pub fn new() -> Self {
        let mut map: HashMap<BackendId, Arc<dyn ExecutionBackend>> = HashMap::new();

        let native = Arc::new(NativeExecutionBackend::new());
        map.insert(native.id(), native);

        if WslExecutionBackend::is_available() {
            let wsl = Arc::new(WslExecutionBackend::new());
            map.insert(wsl.id(), wsl);
        }

        Self {
            backends: RwLock::new(map),
            active_backend: RwLock::new(BackendId::native()),
        }
    }

    pub fn register(&self, backend: Arc<dyn ExecutionBackend>) {
        self.backends.write().unwrap().insert(backend.id(), backend);
    }

    pub fn list(&self) -> Vec<BackendDescriptor> {
        self.backends
            .read()
            .unwrap()
            .values()
            .map(|b| b.descriptor())
            .collect()
    }

    pub fn get(&self, id: &BackendId) -> Option<Arc<dyn ExecutionBackend>> {
        self.backends.read().unwrap().get(id).cloned()
    }

    pub fn active(&self) -> Arc<dyn ExecutionBackend> {
        let id = self.active_backend.read().unwrap().clone();
        self.get(&id)
            .unwrap_or_else(|| Arc::new(NativeExecutionBackend::new()))
    }

    pub fn set_active(&self, id: &BackendId) -> Result<(), CoreError> {
        if self.backends.read().unwrap().contains_key(id) {
            *self.active_backend.write().unwrap() = id.clone();
            Ok(())
        } else {
            Err(CoreError::ExecutionFailed(format!(
                "Unknown backend ID: {}",
                id.as_str()
            )))
        }
    }
}

impl Default for BackendRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub fn create_platform_backend() -> Box<dyn ExecutionBackend> {
    Box::new(NativeExecutionBackend::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_wsl_path() {
        let path = Path::new(r"C:\Users\Matmus\project");
        let wsl = to_wsl_path(path);
        assert_eq!(wsl, "/mnt/c/Users/Matmus/project");

        let path_d = Path::new(r"D:\Omen Shell\crates");
        let wsl_d = to_wsl_path(path_d);
        assert_eq!(wsl_d, "/mnt/d/Omen Shell/crates");
    }

    #[test]
    fn test_backend_registry() {
        let registry = BackendRegistry::new();
        let list = registry.list();
        assert!(!list.is_empty());
        assert!(list.iter().any(|b| b.id == BackendId::native()));

        let active = registry.active();
        assert_eq!(active.id(), BackendId::native());
    }
}
