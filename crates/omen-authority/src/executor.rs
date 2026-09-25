//! Physical execution: Omen's substrate remains authoritative.
//!
//! The executor accepts ONLY [`VerifiedExecutionBinding`]: a value whose
//! physical command was produced by the Omen-owned trusted resolver from
//! the authorised semantic action and verified against the Tethers
//! dispatch. There is no argv/cwd parameter left to supply, so
//! post-COMMIT physical substitution is impossible by type — the old
//! `execute(&verified_dispatch, arbitrary_argv, arbitrary_cwd, ...)`
//! shape no longer exists.
//!
//! Execution itself still runs through the existing `omen_engine`
//! supervision unchanged (argv-only, environment isolation, timeouts,
//! cancellation, process-tree cleanup, capture, evidence). H2 adds no
//! executor, no replacement semantics — only the admission seam before
//! it, now closed through the trusted binding.
//!
//! The [`PhysicalExecutor`] trait lets the matrix prove spawn counts
//! deterministically (counting fakes with marker files); production uses
//! [`SupervisorExecutor`], which re-verifies executable identity
//! immediately before spawn (bind -> spawn window).

use crate::AuthorityError;
use crate::binding::VerifiedExecutionBinding;
use omen_core::{ProcessExit, RuntimeStatus};
use omen_engine::supervisor::{ExecutionOutput, ExecutionRequest, ProcessSupervisor};

/// One physical attempt. `attempted == false` means NO process was
/// spawned (cancel-before-dispatch); the Gate must never receive
/// `attempted: false` for a committed dispatch, so the outcome layer
/// defers instead of sending.
#[derive(Debug, Clone)]
pub struct ExecAttempt {
    pub attempted: bool,
    pub runtime: RuntimeStatus,
    pub exit: ProcessExit,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration_ms: u64,
    /// Omen-side execution identity (external_execution_identity).
    pub omen_exec_id: String,
}

impl ExecAttempt {
    /// Truthful classification for the OUTCOME frame.
    pub fn classification(&self) -> crate::protocol::OutcomeClassification {
        use crate::protocol::OutcomeClassification as C;
        use omen_core::RuntimeStatus as R;
        if !self.attempted {
            return C::Uncertain;
        }
        match self.runtime {
            R::Completed if self.exit.is_zero() => C::Succeeded,
            R::Completed => C::Failed,
            R::TimedOut | R::Cancelled | R::SpawnFailed | R::ContainmentFailed | R::IoFailed => {
                C::Failed
            }
            R::OutcomeUnknown => C::Uncertain,
        }
    }

    pub fn error_text(&self) -> Option<String> {
        use omen_core::RuntimeStatus as R;
        if !self.attempted {
            return Some("omen.exec.not_attempted: cancelled before dispatch".to_string());
        }
        match self.runtime {
            R::Completed if self.exit.is_zero() => None,
            R::Completed => Some(format!(
                "omen.exec.exit_code: {} stderr: {}",
                self.exit.code.unwrap_or(-1),
                String::from_utf8_lossy(&self.stderr)
                    .chars()
                    .take(512)
                    .collect::<String>()
            )),
            R::TimedOut => Some("omen.exec.timeout: deadline fired, tree terminated".to_string()),
            R::Cancelled => Some("omen.exec.cancelled: stop observed, death confirmed".to_string()),
            R::SpawnFailed => Some("omen.exec.spawn_failed".to_string()),
            R::ContainmentFailed => Some("omen.exec.containment_failed".to_string()),
            R::IoFailed => Some("omen.exec.io_failed".to_string()),
            R::OutcomeUnknown => Some("omen.exec.outcome_unknown".to_string()),
        }
    }
}

pub trait PhysicalExecutor: Send {
    /// Execute EXACTLY the verified binding. No argv, no cwd, no
    /// timeout parameter: every physical input is inside `binding`,
    /// already verified against the Tethers dispatch.
    fn execute(
        &mut self,
        binding: &VerifiedExecutionBinding,
    ) -> impl std::future::Future<Output = Result<ExecAttempt, AuthorityError>> + Send;
}

/// Production executor: the existing Omen supervision path.
pub struct SupervisorExecutor {
    supervisor: ProcessSupervisor,
}

impl SupervisorExecutor {
    pub fn new() -> Self {
        Self {
            supervisor: ProcessSupervisor::new(),
        }
    }
}

impl Default for SupervisorExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl PhysicalExecutor for SupervisorExecutor {
    async fn execute(
        &mut self,
        binding: &VerifiedExecutionBinding,
    ) -> Result<ExecAttempt, AuthorityError> {
        // Bind -> spawn window: re-verify TARGET executable identity
        // immediately before spawn. A binary swapped after binding
        // refuses here with zero spawn.
        let current = match std::fs::read(binding.exe()) {
            Ok(b) => format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(b)),
            Err(e) => {
                return Err(AuthorityError::Execute(format!(
                    "exec.exe_unreadable: {}: {e}",
                    binding.exe().display()
                )));
            }
        };
        if current != binding.exe_sha256() {
            return Err(AuthorityError::Execute(format!(
                "exec.exe_identity_changed: {} (zero spawn)",
                binding.exe().display()
            )));
        }
        let mut req =
            ExecutionRequest::simple(binding.argv().to_vec(), binding.cwd().to_path_buf());
        // Bounded empty environment projection: the engine's argv-only
        // isolation policy governs (no second environment model).
        req.env = binding.env().to_vec();
        req.timeout_ms = binding.timeout_ms();
        let out: ExecutionOutput =
            self.supervisor.execute(req).await.map_err(|e| {
                AuthorityError::Execute(format!("supervisor.execute.failed: {e:?}"))
            })?;
        Ok(ExecAttempt {
            attempted: true,
            runtime: out.runtime_status,
            exit: out.process_exit,
            stdout: out.stdout_bounded,
            stderr: out.stderr_bounded,
            duration_ms: out.duration_ms,
            omen_exec_id: format!("omen-exec-{}", binding.execution_id()),
        })
    }
}

/// Deterministic test executor: records calls AND the exact physical
/// values it was asked to execute, writes the spawn sentinel marker
/// (proving a physical execution WOULD have happened), and returns a
/// scripted attempt. Zero calls + absent marker == zero spawn. The
/// recorded argv/cwd prove the executed command IS the verified binding.
pub struct CountingExecutor {
    pub calls: u64,
    pub marker: Option<std::path::PathBuf>,
    pub scripted: ExecAttempt,
    pub last_argv: Vec<String>,
    pub last_cwd: Option<std::path::PathBuf>,
}

impl CountingExecutor {
    pub fn new(marker: Option<std::path::PathBuf>, scripted: ExecAttempt) -> Self {
        Self {
            calls: 0,
            marker,
            scripted,
            last_argv: Vec::new(),
            last_cwd: None,
        }
    }

    pub fn succeeding(marker: Option<std::path::PathBuf>) -> Self {
        Self::new(
            marker,
            ExecAttempt {
                attempted: true,
                runtime: RuntimeStatus::Completed,
                exit: ProcessExit::success(0),
                stdout: b"marker-ok".to_vec(),
                stderr: Vec::new(),
                duration_ms: 3,
                omen_exec_id: "omen-exec-test".to_string(),
            },
        )
    }
}

impl PhysicalExecutor for CountingExecutor {
    async fn execute(
        &mut self,
        binding: &VerifiedExecutionBinding,
    ) -> Result<ExecAttempt, AuthorityError> {
        self.calls += 1;
        self.last_argv = binding.argv().to_vec();
        self.last_cwd = Some(binding.cwd().to_path_buf());
        // Physical truth: the sentinel is written ONLY when a process
        // was actually spawned. A cancelled-before-dispatch attempt
        // writes nothing — marker absence == zero spawn.
        if self.scripted.attempted
            && let Some(marker) = &self.marker
        {
            std::fs::write(marker, format!("spawn-{}\n", self.calls))
                .map_err(|e| AuthorityError::Execute(format!("sentinel.write.failed: {e}")))?;
        }
        Ok(self.scripted.clone())
    }
}
