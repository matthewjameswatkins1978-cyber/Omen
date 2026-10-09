use crate::pty::NativePtyHandle;
use crate::supervisor::ExecutionRequest;
use omen_core::{
    Assurance, BackendId, CoreError, EnforcementLevel, ProcessExit, PtySessionId,
    RequiredAssurance, RuntimeStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
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

pub type PipelineWaitFuture = Pin<Box<dyn Future<Output = PipelineWaitResult> + Send>>;
pub type PipelineWaitResult =
    Result<(RuntimeStatus, Vec<ProcessExit>, Vec<u8>, Vec<Vec<u8>>), CoreError>;

/// Handle to a running process spawned by an ExecutionBackend.
pub trait ExecutionHandle: Send {
    fn pid(&self) -> Option<u32>;
    fn terminate_tree(&mut self) -> Result<(), CoreError>;
    fn wait_bounded(self: Box<Self>, timeout: Duration) -> ExecutionWaitFuture;
    /// Wait with an external cancellation flag.
    ///
    /// E2 stop truth: firing `cancel` records INTENT only. This method still
    /// performs the physical stop (`terminate_tree`) and then observes the
    /// child for a bounded grace period. Death observed → `Cancelled`;
    /// death unconfirmed within grace → `OutcomeUnknown`. The flag firing
    /// never, by itself, manufactures a terminal state.
    fn wait_cancelable(
        self: Box<Self>,
        timeout: Duration,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> ExecutionWaitFuture;
}

/// Handle to a running byte pipeline spawned by an execution backend.
pub trait PipelineExecutionHandle: Send {
    fn terminate_tree(&mut self) -> Result<(), CoreError>;
    fn wait_bounded(self: Box<Self>, timeout: Duration) -> PipelineWaitFuture;
    fn wait_cancelable(
        self: Box<Self>,
        timeout: Duration,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> PipelineWaitFuture;
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
    fn spawn_pipeline(
        &self,
        _stages: &[ExecutionRequest],
    ) -> Result<Box<dyn PipelineExecutionHandle>, CoreError> {
        Err(CoreError::ExecutionFailedCode {
            code: omen_core::ErrorCode::Unsupported,
            message: "the active execution backend does not support supervised byte pipelines"
                .into(),
        })
    }
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

/// A native pipeline is spawned stage-by-stage so each OS pipe connects
/// directly to the next child's stdin. It is intentionally not a shell
/// executor: Omen supplies already-resolved argv and owns all supervision.
pub struct NativePipelineExecutionHandle {
    requests: Vec<ExecutionRequest>,
    stages: Vec<NativeExecutionHandle>,
    stdout_task: Option<tokio::task::JoinHandle<Vec<u8>>>,
    stderr_tasks: Vec<tokio::task::JoinHandle<Vec<u8>>>,
    finished: bool,
}

impl NativePipelineExecutionHandle {
    fn new(requests: &[ExecutionRequest]) -> Self {
        Self {
            requests: requests.to_vec(),
            stages: Vec::with_capacity(requests.len()),
            stdout_task: None,
            stderr_tasks: Vec::with_capacity(requests.len()),
            finished: false,
        }
    }

    fn start(&mut self) -> Result<(), CoreError> {
        let backend = NativeExecutionBackend::new();
        let stage_count = self.requests.len();
        let mut next_stdin = Some(std::process::Stdio::null());

        for (index, request) in self.requests.iter().enumerate() {
            let stdin = next_stdin.take().ok_or_else(|| {
                CoreError::ExecutionFailed("native pipeline stdin connection was lost".into())
            })?;
            let stage = backend.spawn_with_stdio(request, stdin, std::process::Stdio::piped())?;
            self.stages.push(stage);
            let stage = self.stages.last_mut().expect("stage was just inserted");

            if let Some(stderr) = stage.stderr.take() {
                self.stderr_tasks.push(tokio::spawn(async move {
                    let mut stderr = stderr;
                    let mut output = Vec::new();
                    let _ = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut output).await;
                    output
                }));
            }

            if index + 1 < stage_count {
                let stdout = stage.stdout.take().ok_or_else(|| {
                    CoreError::ExecutionFailed(
                        "native pipeline stage did not expose its stdout pipe".into(),
                    )
                })?;
                next_stdin = Some(stdout.try_into().map_err(|error| {
                    CoreError::ExecutionFailed(format!(
                        "failed to connect native pipeline stage {index} to stage {}: {error}",
                        index + 1
                    ))
                })?);
            } else if let Some(mut stdout) = stage.stdout.take() {
                self.stdout_task = Some(tokio::spawn(async move {
                    let mut output = Vec::new();
                    let _ = tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut output).await;
                    output
                }));
            }
        }

        self.requests.clear();
        Ok(())
    }

    async fn wait_inner(
        mut self: Box<Self>,
        timeout: Duration,
        mut cancel: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> PipelineWaitResult {
        if self.requests.is_empty() && self.stages.is_empty() {
            return Err(CoreError::ExecutionFailed(
                "native pipeline cannot contain zero stages".into(),
            ));
        }

        if cancel.as_ref().is_some_and(|flag| *flag.borrow()) {
            self.finished = true;
            let exits = vec![
                ProcessExit {
                    code: None,
                    signal: Some("CANCELLED_BEFORE_DISPATCH".into()),
                };
                self.requests.len()
            ];
            let stderr = vec![Vec::new(); self.requests.len()];
            return Ok((RuntimeStatus::Cancelled, exits, Vec::new(), stderr));
        }

        self.start()?;

        let mut waiters = tokio::task::JoinSet::new();
        let stage_count = self.stages.len();
        for (index, stage) in self.stages.iter_mut().enumerate() {
            if let Some(mut child) = stage.child.take() {
                waiters.spawn(async move { (index, child.wait().await) });
            }
        }

        let mut exits = vec![None; stage_count];
        let mut wait_failed = false;
        let mut runtime_status = RuntimeStatus::Completed;
        {
            let wait_all = async {
                while let Some(result) = waiters.join_next().await {
                    match result {
                        Ok((index, Ok(status))) => {
                            exits[index] = Some(ProcessExit {
                                code: status.code(),
                                signal: None,
                            });
                        }
                        Ok((index, Err(error))) => {
                            exits[index] = Some(ProcessExit {
                                code: None,
                                signal: Some(error.to_string()),
                            });
                            wait_failed = true;
                        }
                        Err(_) => wait_failed = true,
                    }
                }
            };
            tokio::pin!(wait_all);

            let termination_status = if let Some(flag) = cancel.as_mut() {
                tokio::select! {
                    _ = &mut wait_all => None,
                    _ = tokio::time::sleep(timeout) => Some(RuntimeStatus::TimedOut),
                    _ = flag.wait_for(|fired| *fired) => Some(RuntimeStatus::Cancelled),
                }
            } else {
                tokio::select! {
                    _ = &mut wait_all => None,
                    _ = tokio::time::sleep(timeout) => Some(RuntimeStatus::TimedOut),
                }
            };

            if let Some(status) = termination_status {
                self.terminate_tree()?;
                if tokio::time::timeout(CANCEL_TERMINATION_GRACE, &mut wait_all)
                    .await
                    .is_err()
                {
                    runtime_status = RuntimeStatus::OutcomeUnknown;
                } else {
                    runtime_status = status;
                }
            }
        }

        if wait_failed && runtime_status == RuntimeStatus::Completed {
            runtime_status = RuntimeStatus::IoFailed;
        }
        let mut stdout_all = Vec::new();
        let mut stderr_by_stage = vec![Vec::new(); stage_count];
        let mut outputs_complete = true;
        let output_deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        if let Some(task) = self.stdout_task.as_mut() {
            match tokio::time::timeout_at(output_deadline, &mut *task).await {
                Ok(Ok(output)) => stdout_all = output,
                _ => {
                    task.abort();
                    outputs_complete = false;
                }
            }
        }
        for (index, task) in self.stderr_tasks.iter_mut().enumerate() {
            match tokio::time::timeout_at(output_deadline, &mut *task).await {
                Ok(Ok(output)) => stderr_by_stage[index] = output,
                _ => {
                    task.abort();
                    outputs_complete = false;
                }
            }
        }
        if !outputs_complete && runtime_status == RuntimeStatus::Completed {
            runtime_status = RuntimeStatus::IoFailed;
        }

        let stage_exits = exits
            .into_iter()
            .map(|exit| {
                exit.unwrap_or(ProcessExit {
                    code: None,
                    signal: Some("EXIT_UNCONFIRMED".into()),
                })
            })
            .collect();

        self.finished = runtime_status != RuntimeStatus::OutcomeUnknown;
        Ok((runtime_status, stage_exits, stdout_all, stderr_by_stage))
    }
}

impl Drop for NativePipelineExecutionHandle {
    fn drop(&mut self) {
        if !self.finished {
            for stage in &mut self.stages {
                let _ = stage.terminate_tree();
            }
        }
        if let Some(task) = &self.stdout_task {
            task.abort();
        }
        for task in &self.stderr_tasks {
            task.abort();
        }
    }
}

impl PipelineExecutionHandle for NativePipelineExecutionHandle {
    fn terminate_tree(&mut self) -> Result<(), CoreError> {
        for stage in &mut self.stages {
            stage.terminate_tree()?;
        }
        Ok(())
    }

    fn wait_bounded(self: Box<Self>, timeout: Duration) -> PipelineWaitFuture {
        Box::pin(async move { self.wait_inner(timeout, None).await })
    }

    fn wait_cancelable(
        self: Box<Self>,
        timeout: Duration,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> PipelineWaitFuture {
        Box::pin(async move { self.wait_inner(timeout, Some(cancel)).await })
    }
}

impl NativeExecutionHandle {
    /// Physical tree-stop for a child already taken out of the handle
    /// (used by the wait paths, where `terminate_tree` cannot take it).
    /// Same enforcement as `terminate_tree`: job object on Windows,
    /// process-group KILL on Unix, direct kill as the last resort.
    fn terminate_tree_for_wait(handle: &mut Box<Self>, child: &mut tokio::process::Child) {
        #[cfg(windows)]
        if let Some(jg) = &handle.job_guard {
            jg.terminate(1);
        }
        #[cfg(unix)]
        if let Some(pid_val) = handle
            .pgid
            .and_then(|pgid| rustix::process::Pid::from_raw(pgid as i32))
        {
            let _ = rustix::process::kill_process_group(pid_val, rustix::process::Signal::KILL);
        }
        let _ = child.start_kill();
    }
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
                        Self::terminate_tree_for_wait(&mut self, &mut child);
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

    fn wait_cancelable(
        mut self: Box<Self>,
        timeout: Duration,
        mut cancel: tokio::sync::watch::Receiver<bool>,
    ) -> ExecutionWaitFuture {
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

            let (runtime_status, process_exit) = if let Some(mut child) = self.child.take() {
                tokio::select! {
                    res = child.wait() => {
                        match res {
                            Ok(status) => (
                                RuntimeStatus::Completed,
                                ProcessExit {
                                    code: status.code(),
                                    signal: None,
                                },
                            ),
                            Err(e) => (
                                RuntimeStatus::IoFailed,
                                ProcessExit {
                                    code: None,
                                    signal: Some(e.to_string()),
                                },
                            ),
                        }
                    }
                    _ = tokio::time::sleep(timeout) => {
                        // Child deadline: same physical stop as wait_bounded.
                        Self::terminate_tree_for_wait(&mut self, &mut child);
                        (
                            RuntimeStatus::TimedOut,
                            ProcessExit {
                                code: None,
                                signal: Some("SIGKILL_TIMEOUT".into()),
                            },
                        )
                    }
                    _ = async {
                        // Scope the watch guard: `Ref` is not Send, so it
                        // must not be held across the branch's later awaits.
                        let _ = cancel.wait_for(|fired| *fired).await;
                    } => {
                        // Intent observed. Attempt the physical stop, then
                        // CONFIRM death within a bounded grace period.
                        // Unconfirmed death stays OutcomeUnknown: Omen must
                        // not rewrite uncertainty into success or failure.
                        Self::terminate_tree_for_wait(&mut self, &mut child);
                        match tokio::time::timeout(
                            CANCEL_TERMINATION_GRACE,
                            child.wait(),
                        )
                        .await
                        {
                            Ok(Ok(status)) => (
                                RuntimeStatus::Cancelled,
                                ProcessExit {
                                    code: status.code(),
                                    signal: Some("CANCELLED".into()),
                                },
                            ),
                            _ => (
                                RuntimeStatus::OutcomeUnknown,
                                ProcessExit {
                                    code: None,
                                    signal: Some("CANCELLED_UNCONFIRMED".into()),
                                },
                            ),
                        }
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

            Ok((runtime_status, process_exit, stdout_all, stderr_all))
        })
    }
}

/// Bounded grace period to observe physical death after a cancellation kill.
/// Expiry means Omen could not confirm termination: the outcome stays
/// `OutcomeUnknown`, never a manufactured success or failure.
pub const CANCEL_TERMINATION_GRACE: Duration = Duration::from_secs(5);
/// Native host execution backend.
pub struct NativeExecutionBackend;

impl NativeExecutionBackend {
    pub fn new() -> Self {
        Self
    }

    fn spawn_with_stdio(
        &self,
        req: &ExecutionRequest,
        stdin: std::process::Stdio,
        stdout: std::process::Stdio,
    ) -> Result<NativeExecutionHandle, CoreError> {
        if req.argv.is_empty() {
            return Err(CoreError::ExecutionFailed("Argv cannot be empty".into()));
        }

        let mut cmd = tokio::process::Command::new(&req.argv[0]);
        cmd.args(&req.argv[1..]);
        cmd.current_dir(&req.cwd);
        for (key, value) in &req.env {
            cmd.env(key, value);
        }
        for secret in &req.secrets {
            if let omen_core::SecretInjectionContract::EnvironmentVariable { name } =
                &secret.contract
            {
                cmd.env(name, &secret.value);
            }
        }
        cmd.stdin(stdin);
        cmd.stdout(stdout);
        cmd.stderr(std::process::Stdio::piped());

        #[cfg(unix)]
        cmd.process_group(0);
        // Linux orphan backstop: if the Omen session dies abruptly
        // (SIGKILL: no Drop, no cancel path), the child must not
        // outlive it as an orphan. PR_SET_PDEATHSIG delivers SIGKILL
        // on parent death; the getppid recheck closes the fork/prctl
        // race (parent already reaped to init). Windows is covered by
        // KILL_ON_JOB_CLOSE job objects instead. Other Unix keeps
        // process-group semantics without pdeathsig (documented gap).
        #[cfg(target_os = "linux")]
        unsafe {
            cmd.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() == 1 {
                    return Err(std::io::Error::other("parent died before pdeathsig armed"));
                }
                Ok(())
            });
        }

        #[cfg(windows)]
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);

        #[cfg(windows)]
        let job_guard = crate::platform::windows::JobObjectGuard::new().map_err(|error| {
            CoreError::ExecutionFailed(format!("Job object initialization error: {error}"))
        })?;

        let mut child = cmd.spawn().map_err(|error| {
            CoreError::ExecutionFailed(format!(
                "Process spawn failed for '{}': {error}",
                req.argv[0]
            ))
        })?;
        let pid = child.id();

        #[cfg(windows)]
        if let Some(pid) = pid {
            job_guard.assign_pid(pid).map_err(|error| {
                let _ = child.start_kill();
                CoreError::ExecutionFailed(format!(
                    "Failed to assign PID {pid} to Job Object: {error}"
                ))
            })?;
            job_guard.resume_primary_thread(pid).map_err(|error| {
                let _ = child.start_kill();
                CoreError::ExecutionFailed(format!(
                    "Failed to resume contained PID {pid} after Job Object assignment: {error}"
                ))
            })?;
        }

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        Ok(NativeExecutionHandle {
            pid,
            child: Some(child),
            stdout,
            stderr,
            #[cfg(windows)]
            job_guard: Some(job_guard),
            #[cfg(unix)]
            pgid: pid,
        })
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
            symlink_escape: EnforcementLevel::Observed,
            pty: true,
        };

        #[cfg(target_os = "linux")]
        let capabilities = BackendCapabilities {
            filesystem: EnforcementLevel::Observed,
            network: EnforcementLevel::Observed,
            descendants: EnforcementLevel::BestEffort,
            symlink_escape: EnforcementLevel::Observed,
            pty: true,
        };

        #[cfg(not(any(windows, target_os = "linux")))]
        let capabilities = BackendCapabilities {
            filesystem: EnforcementLevel::Observed,
            network: EnforcementLevel::Observed,
            descendants: EnforcementLevel::BestEffort,
            symlink_escape: EnforcementLevel::Observed,
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
        // Stdio setup.
        //
        // Closed means the child is not expected to consume caller input.
        // A created-but-immediately-dropped pipe would still present an
        // *empty readable* stdin, which stdin-sensitive tools (ripgrep with
        // no explicit path, etc.) interpret as "search this empty stream"
        // instead of their normal no-stdin behavior -- a plausible wrong
        // result caused solely by harness topology. Attaching null
        // (/dev/null, NUL) instead yields immediate EOF with no readable
        // pipe, so children observe "no input" rather than "empty input".
        // Inline (or stdin secrets) carries explicit bytes and keeps a pipe.
        let stdin_carries_bytes = req.stdin_payload.as_ref().is_some_and(|p| !p.is_empty())
            || req
                .secrets
                .iter()
                .any(|secret| matches!(secret.contract, omen_core::SecretInjectionContract::Stdin));
        let stdin = match req.stdin_mode {
            omen_core::StdioMode::Closed if !stdin_carries_bytes => std::process::Stdio::null(),
            omen_core::StdioMode::Closed | omen_core::StdioMode::Inline => {
                std::process::Stdio::piped()
            }
            omen_core::StdioMode::Inherit => std::process::Stdio::inherit(),
            _ => std::process::Stdio::null(),
        };
        let mut handle = self.spawn_with_stdio(req, stdin, std::process::Stdio::piped())?;

        // Write stdin payload and stdin secrets
        if let Some(mut stdin) = handle.child.as_mut().and_then(|child| child.stdin.take()) {
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

        Ok(Box::new(handle))
    }

    fn spawn_pipeline(
        &self,
        stages: &[ExecutionRequest],
    ) -> Result<Box<dyn PipelineExecutionHandle>, CoreError> {
        if stages.is_empty() {
            return Err(CoreError::ExecutionFailed(
                "native pipeline cannot contain zero stages".into(),
            ));
        }
        for (index, stage) in stages.iter().enumerate() {
            if stage.argv.is_empty() {
                return Err(CoreError::ExecutionFailed(format!(
                    "native pipeline stage {index} has empty argv"
                )));
            }
            if stage.stdin_payload.is_some() || !stage.secrets.is_empty() {
                return Err(CoreError::ExecutionFailedCode {
                    code: omen_core::ErrorCode::Unsupported,
                    message: format!(
                        "native pipeline stage {index} cannot use inline stdin payloads or secrets"
                    ),
                });
            }
            if !matches!(stage.stdin_mode, omen_core::StdioMode::Closed) {
                return Err(CoreError::ExecutionFailedCode {
                    code: omen_core::ErrorCode::Unsupported,
                    message: format!(
                        "native pipeline stage {index} requires closed stdin semantics"
                    ),
                });
            }
        }
        Ok(Box::new(NativePipelineExecutionHandle::new(stages)))
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

/// Process-wide cache of the WSL availability probe: spawning wsl.exe
/// costs up to 6s cold and BackendRegistry::new() runs per session.
/// WSL install state does not change meaningfully within one process.
static WSL_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

impl WslExecutionBackend {
    pub fn new() -> Self {
        Self
    }

    pub fn is_available() -> bool {
        *WSL_AVAILABLE.get_or_init(Self::probe_available)
    }

    fn probe_available() -> bool {
        #[cfg(windows)]
        {
            // Probe wsl.exe -e true with a hard 6000ms deadline, killing child on timeout to prevent hanging.
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
                symlink_escape: EnforcementLevel::Mediated,
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

    fn spawn_pipeline(
        &self,
        stages: &[ExecutionRequest],
    ) -> Result<Box<dyn PipelineExecutionHandle>, CoreError> {
        if !Self::is_available() {
            return Err(CoreError::ExecutionFailed(
                "WSL backend is unavailable on this system".into(),
            ));
        }

        let translated = stages
            .iter()
            .map(|stage| {
                let mut request = stage.clone();
                let mut argv = vec![
                    "wsl.exe".to_string(),
                    "--cd".to_string(),
                    to_wsl_path(&stage.cwd),
                    "-e".to_string(),
                ];
                argv.extend(stage.argv.iter().cloned());
                request.argv = argv;
                request
            })
            .collect::<Vec<_>>();
        NativeExecutionBackend::new().spawn_pipeline(&translated)
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

    /// Linux orphan backstop: a child spawned through the native backend
    /// (with PR_SET_PDEATHSIG armed pre-exec) must start and exit
    /// normally. This exercises the pre_exec path; the kill-on-parent-
    /// death itself is a kernel guarantee, not something a test can
    /// observe without orphaning a real process.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_spawn_with_pdeathsig_exits_cleanly() {
        use crate::supervisor::ExecutionRequest;
        let backend = NativeExecutionBackend;
        let req = ExecutionRequest::simple(
            vec!["/bin/true".to_string()],
            std::path::PathBuf::from("/tmp"),
        );
        let handle = backend
            .spawn(&req)
            .expect("spawn with pdeathsig must succeed");
        let (status, exit, _, _) = handle
            .wait_bounded(std::time::Duration::from_secs(10))
            .await
            .expect("wait must succeed");
        assert_eq!(exit.code, Some(0), "true exits 0, status={status:?}");
    }
}
