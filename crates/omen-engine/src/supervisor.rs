use crate::backend::{ExecutionBackend, create_platform_backend};
use omen_core::{
    AdapterClassification, CoreError, EnforcementReport, ProcessExit, RequiredAssurance,
    RuntimeStatus, StdioMode,
};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

pub const DEFAULT_INLINE_BUDGET: usize = 8192;

#[derive(Debug, Clone)]
pub struct ExecutionRequest {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub stdin_mode: StdioMode,
    pub stdin_payload: Option<Vec<u8>>,
    pub timeout_ms: u64,
    pub inline_budget: usize,
    pub required_assurance: RequiredAssurance,
}

#[derive(Debug, Clone)]
pub struct ExecutionOutput {
    pub runtime_status: RuntimeStatus,
    pub process_exit: ProcessExit,
    pub adapter_classification: AdapterClassification,
    pub enforcement: EnforcementReport,
    pub stdout_bounded: Vec<u8>,
    pub stderr_bounded: Vec<u8>,
    pub stdout_all: Vec<u8>,
    pub stderr_all: Vec<u8>,
    pub duration_ms: u64,
}

pub struct ProcessSupervisor {
    backend: Box<dyn ExecutionBackend>,
}

impl ProcessSupervisor {
    pub fn new() -> Self {
        Self {
            backend: create_platform_backend(),
        }
    }

    pub fn with_backend(backend: Box<dyn ExecutionBackend>) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &dyn ExecutionBackend {
        self.backend.as_ref()
    }

    pub async fn execute(&self, req: ExecutionRequest) -> Result<ExecutionOutput, CoreError> {
        // 1. Preflight check: reject if required assurance exceeds platform capabilities
        self.backend.preflight(&req.required_assurance)?;

        if req.argv.is_empty() {
            return Err(CoreError::ExecutionFailed("Argv cannot be empty".into()));
        }

        let start_time = std::time::Instant::now();

        let mut cmd = Command::new(&req.argv[0]);
        if req.argv.len() > 1 {
            cmd.args(&req.argv[1..]);
        }
        cmd.current_dir(&req.cwd);

        for (k, v) in &req.env {
            cmd.env(k, v);
        }

        // Stdio setup: closed stdin receives EOF immediately
        match req.stdin_mode {
            StdioMode::Closed | StdioMode::Inline => {
                cmd.stdin(Stdio::piped());
            }
            StdioMode::Inherit => {
                cmd.stdin(Stdio::inherit());
            }
            _ => {
                cmd.stdin(Stdio::null());
            }
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        #[cfg(windows)]
        let job_guard = crate::platform::windows::JobObjectGuard::new().map_err(|e| {
            CoreError::ExecutionFailed(format!("Job object initialization error: {e}"))
        })?;

        let mut child = cmd.spawn().map_err(|e| {
            CoreError::ExecutionFailed(format!("Process spawn failed for '{}': {e}", req.argv[0]))
        })?;

        let child_pid = child.id();

        #[cfg(windows)]
        if let Some(pid) = child_pid {
            let _ = job_guard.assign_pid(pid);
        }

        // Handle stdin delivery or close
        if let Some(mut stdin) = child.stdin.take() {
            if let (StdioMode::Inline, Some(payload)) = (req.stdin_mode, &req.stdin_payload) {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(payload).await;
            }
            // Dropping stdin immediately sends EOF to the child
            drop(stdin);
        }

        let mut stdout_pipe = child.stdout.take().expect("stdout pipe missing");
        let mut stderr_pipe = child.stderr.take().expect("stderr pipe missing");

        let stdout_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stdout_pipe.read_to_end(&mut buf).await;
            buf
        });

        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stderr_pipe.read_to_end(&mut buf).await;
            buf
        });

        let timeout_duration = Duration::from_millis(req.timeout_ms);
        let wait_result = tokio::time::timeout(timeout_duration, child.wait()).await;

        let (runtime_status, process_exit) = match wait_result {
            Ok(Ok(exit_status)) => {
                let code = exit_status.code();
                (RuntimeStatus::Completed, ProcessExit { code, signal: None })
            }
            Ok(Err(e)) => (
                RuntimeStatus::IoFailed,
                ProcessExit {
                    code: None,
                    signal: Some(e.to_string()),
                },
            ),
            Err(_) => {
                // Timeout fired: terminate process tree
                #[cfg(windows)]
                job_guard.terminate(1);

                let _ = child.kill().await;

                (
                    RuntimeStatus::TimedOut,
                    ProcessExit {
                        code: None,
                        signal: Some("SIGKILL_TIMEOUT".into()),
                    },
                )
            }
        };

        let stdout_all = stdout_task.await.unwrap_or_default();
        let stderr_all = stderr_task.await.unwrap_or_default();

        let stdout_bounded = if stdout_all.len() > req.inline_budget {
            stdout_all[..req.inline_budget].to_vec()
        } else {
            stdout_all.clone()
        };

        let stderr_bounded = if stderr_all.len() > req.inline_budget {
            stderr_all[..req.inline_budget].to_vec()
        } else {
            stderr_all.clone()
        };

        let adapter_classification = if runtime_status == RuntimeStatus::Completed {
            if process_exit.is_zero() {
                AdapterClassification::Success
            } else {
                AdapterClassification::Failure
            }
        } else {
            AdapterClassification::Failure
        };

        let caps = self.backend.capabilities();
        let enforcement = EnforcementReport {
            filesystem: caps.filesystem,
            network: caps.network,
            descendant_processes: caps.descendants,
            symlink_escape: caps.symlink_escape,
        };

        let duration_ms = start_time.elapsed().as_millis() as u64;

        Ok(ExecutionOutput {
            runtime_status,
            process_exit,
            adapter_classification,
            enforcement,
            stdout_bounded,
            stderr_bounded,
            stdout_all,
            stderr_all,
            duration_ms,
        })
    }
}

impl Default for ProcessSupervisor {
    fn default() -> Self {
        Self::new()
    }
}
