//! Typed Windows observations (facts only). Judgment lives in `judge`.

use crate::observation::Observation;
use std::time::{Duration, Instant};

/// Bounded transcript cap (mirrors POSIX 64 KiB M0 convention).
pub const WIN_TRANSCRIPT_CAP: usize = 64 * 1024;

/// Poll slice for bounded pipe reads.
pub const WIN_POLL_SLICE_MS: u64 = 20;

/// Bounded Windows console transcript capture.
#[derive(Debug, Clone, Default)]
pub struct WinTranscript {
    pub retained: Vec<u8>,
    pub total_bytes: u64,
    pub truncated: bool,
    pub eof_or_hangup: bool,
    pub bounded_out: bool,
}

impl WinTranscript {
    pub fn as_lossy(&self) -> String {
        String::from_utf8_lossy(&self.retained).into_owned()
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.retained
            .windows(needle.len())
            .any(|w| w == needle.as_bytes())
    }

    pub fn has_line(&self, needle: &str) -> bool {
        self.as_lossy().lines().any(|l| l.trim() == needle)
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.total_bytes += chunk.len() as u64;
        if self.retained.len() < WIN_TRANSCRIPT_CAP {
            let room = WIN_TRANSCRIPT_CAP - self.retained.len();
            let take = room.min(chunk.len());
            self.retained.extend_from_slice(&chunk[..take]);
            if take < chunk.len() {
                self.truncated = true;
            }
        } else {
            self.truncated = true;
        }
    }
}

/// File-type codes from `GetFileType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinFileType {
    Unknown,
    Disk,
    Char,
    Pipe,
    Other(u32),
}

impl WinFileType {
    pub fn from_raw(v: u32) -> Self {
        match v {
            0 => WinFileType::Unknown,
            1 => WinFileType::Disk,
            2 => WinFileType::Char,
            3 => WinFileType::Pipe,
            other => WinFileType::Other(other),
        }
    }

    pub fn stable_id(&self) -> &'static str {
        match self {
            WinFileType::Unknown => "unknown",
            WinFileType::Disk => "disk",
            WinFileType::Char => "char",
            WinFileType::Pipe => "pipe",
            WinFileType::Other(_) => "other",
        }
    }

    /// Console-capable file type (ConPTY client sees `CHAR` on std handles).
    pub fn is_console_capable(&self) -> bool {
        matches!(self, WinFileType::Char)
    }
}

/// Availability-separated standard-handle observation for one process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsStdHandleObservation {
    pub stdin_non_null: bool,
    pub stdout_non_null: bool,
    pub stderr_non_null: bool,
    pub stdin_file_type: Option<WinFileType>,
    pub stdout_file_type: Option<WinFileType>,
    pub stderr_file_type: Option<WinFileType>,
    pub stdin_console_mode: Option<u32>,
    pub stdout_console_mode: Option<u32>,
    pub source: String,
}

impl WindowsStdHandleObservation {
    pub fn observation(&self) -> Observation {
        let encode = |t: Option<WinFileType>| -> Option<u32> {
            match t {
                Some(WinFileType::Unknown) => Some(0),
                Some(WinFileType::Disk) => Some(1),
                Some(WinFileType::Char) => Some(2),
                Some(WinFileType::Pipe) => Some(3),
                Some(WinFileType::Other(v)) => Some(v),
                None => None,
            }
        };
        Observation::WindowsStdHandleObservation {
            stdin_non_null: self.stdin_non_null,
            stdout_non_null: self.stdout_non_null,
            stderr_non_null: self.stderr_non_null,
            stdin_file_type: encode(self.stdin_file_type),
            stdout_file_type: encode(self.stdout_file_type),
            stderr_file_type: encode(self.stderr_file_type),
            stdin_console_mode: self.stdin_console_mode,
            stdout_console_mode: self.stdout_console_mode,
            source: self.source.clone(),
        }
    }

    /// True when stdin/stdout are non-null and both look console-capable.
    pub fn console_handles_usable(&self) -> bool {
        self.stdin_non_null
            && self.stdout_non_null
            && self
                .stdin_file_type
                .map(|t| t.is_console_capable())
                .unwrap_or(false)
            && self
                .stdout_file_type
                .map(|t| t.is_console_capable())
                .unwrap_or(false)
    }
}

/// Canonical Windows console-mode snapshot (consequential bits preserved raw).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsConsoleModeSnapshot {
    pub input_mode: Option<u32>,
    pub output_mode: Option<u32>,
    pub available: bool,
    pub source: String,
}

impl WindowsConsoleModeSnapshot {
    pub fn unavailable(source: impl Into<String>) -> Self {
        Self {
            input_mode: None,
            output_mode: None,
            available: false,
            source: source.into(),
        }
    }

    pub fn measured(input: Option<u32>, output: Option<u32>, source: impl Into<String>) -> Self {
        Self {
            input_mode: input,
            output_mode: output,
            available: input.is_some() || output.is_some(),
            source: source.into(),
        }
    }

    pub fn observation(&self) -> Observation {
        Observation::WindowsConsoleModeSnapshot {
            input_mode: self.input_mode,
            output_mode: self.output_mode,
            available: self.available,
            source: self.source.clone(),
        }
    }

    /// Consequential input bits we track for dirty/restore comparisons.
    pub fn input_consequential(&self) -> Option<u32> {
        const MASK: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0008 | 0x0200; // PROCESSED|LINE|ECHO|WINDOW|VT_INPUT
        self.input_mode.map(|m| m & MASK)
    }

    pub fn output_consequential(&self) -> Option<u32> {
        const MASK: u32 = 0x0001 | 0x0002 | 0x0004; // PROCESSED|WRAP_EOL|VT_PROCESSING
        self.output_mode.map(|m| m & MASK)
    }
}

/// Observed console screen-buffer dimensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsConsoleDimensions {
    pub rows: Option<u16>,
    pub cols: Option<u16>,
    pub available: bool,
    pub source: String,
}

impl WindowsConsoleDimensions {
    pub fn unavailable(source: impl Into<String>) -> Self {
        Self {
            rows: None,
            cols: None,
            available: false,
            source: source.into(),
        }
    }

    pub fn observation(&self) -> Observation {
        Observation::WindowsConsoleDimensions {
            rows: self.rows,
            cols: self.cols,
            available: self.available,
            source: self.source.clone(),
        }
    }
}

/// Independent receipt of a Windows console control event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowsCtrlReceipt {
    Observed { kind: &'static str, source: String },
    NotObserved,
    Unavailable { reason: String },
}

impl WindowsCtrlReceipt {
    pub fn observation(&self) -> Option<Observation> {
        match self {
            WindowsCtrlReceipt::Observed { kind, source } => {
                Some(Observation::WindowsCtrlReceiptObserved {
                    event_kind: (*kind).into(),
                    source: source.clone(),
                })
            }
            _ => None,
        }
    }
}

/// Cause context for a Windows exit observation (never inferred from int alone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowsExitCauseContext {
    NormalExit,
    CtrlCObserved,
    CtrlBreakObserved,
    ProductTerminateRequested,
    TimeoutTerminationRequested,
    Unknown,
}

impl WindowsExitCauseContext {
    pub fn stable_id(&self) -> &'static str {
        match self {
            WindowsExitCauseContext::NormalExit => "normal_exit",
            WindowsExitCauseContext::CtrlCObserved => "ctrl_c_observed",
            WindowsExitCauseContext::CtrlBreakObserved => "ctrl_break_observed",
            WindowsExitCauseContext::ProductTerminateRequested => "product_terminate_requested",
            WindowsExitCauseContext::TimeoutTerminationRequested => "timeout_termination_requested",
            WindowsExitCauseContext::Unknown => "unknown",
        }
    }
}

/// Observed Windows exit status with explicit cause context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsExitObservation {
    pub raw_status: Option<u32>,
    pub product_code: Option<i32>,
    pub cause_context: WindowsExitCauseContext,
    pub source: String,
}

impl WindowsExitObservation {
    pub fn observation(&self) -> Observation {
        Observation::WindowsExitObservation {
            raw_status: self.raw_status,
            product_code: self.product_code,
            cause_context: self.cause_context.stable_id().into(),
            source: self.source.clone(),
        }
    }

    /// True when the observed raw DWORD bits equal `expected` (bit preservation).
    pub fn preserves_bits(&self, expected: u32) -> bool {
        self.raw_status == Some(expected) || self.product_code.map(|c| c as u32) == Some(expected)
    }
}

/// Process liveness fact from Toolhelp / wait APIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsProcessLiveness {
    pub pid: u32,
    pub alive: bool,
    pub parent_pid: Option<u32>,
    pub source: String,
}

impl WindowsProcessLiveness {
    pub fn observation(&self) -> Observation {
        Observation::WindowsProcessLiveness {
            pid: self.pid,
            alive: self.alive,
            parent_pid: self.parent_pid,
            source: self.source.clone(),
        }
    }
}

/// ConPTY lifecycle observation (API phase + success).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsConPtyLifecycle {
    pub phase: String,
    pub ok: bool,
    pub detail: String,
    pub source: String,
}

impl WindowsConPtyLifecycle {
    pub fn observation(&self) -> Observation {
        Observation::WindowsConPtyLifecycleObservation {
            phase: self.phase.clone(),
            ok: self.ok,
            detail: self.detail.clone(),
            source: self.source.clone(),
        }
    }
}

/// Parse fixture `windows-report` JSON from a transcript line.
///
/// Returns `None` when no usable report is present — never invents facts.
pub fn parse_windows_report(text: &str) -> Option<WindowsReport> {
    for line in text.lines().rev() {
        let trimmed = line.trim();
        let start = trimmed.find('{')?;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&trimmed[start..]) else {
            continue;
        };
        if v.get("fixture").and_then(|f| f.as_str()) != Some("windows-report") {
            continue;
        }
        let pid = v.get("pid").and_then(|p| p.as_u64())? as u32;
        let ppid = v.get("ppid").and_then(|p| p.as_u64()).map(|p| p as u32);
        let file_type = |key: &str| -> Option<WinFileType> {
            v.get(key)
                .and_then(|t| t.as_u64())
                .map(|t| WinFileType::from_raw(t as u32))
        };
        return Some(WindowsReport {
            pid,
            ppid,
            stdin_non_null: v
                .get("stdin_non_null")
                .and_then(|b| b.as_bool())
                .unwrap_or(false),
            stdout_non_null: v
                .get("stdout_non_null")
                .and_then(|b| b.as_bool())
                .unwrap_or(false),
            stderr_non_null: v
                .get("stderr_non_null")
                .and_then(|b| b.as_bool())
                .unwrap_or(false),
            stdin_file_type: file_type("stdin_file_type"),
            stdout_file_type: file_type("stdout_file_type"),
            stderr_file_type: file_type("stderr_file_type"),
            stdin_console_mode: v
                .get("stdin_console_mode")
                .and_then(|m| m.as_u64())
                .map(|m| m as u32),
            stdout_console_mode: v
                .get("stdout_console_mode")
                .and_then(|m| m.as_u64())
                .map(|m| m as u32),
            winsize_rows: v
                .get("winsize_rows")
                .and_then(|r| r.as_u64())
                .map(|r| r as u16),
            winsize_cols: v
                .get("winsize_cols")
                .and_then(|c| c.as_u64())
                .map(|c| c as u16),
            input_mode: v
                .get("input_mode")
                .and_then(|m| m.as_u64())
                .map(|m| m as u32),
            output_mode: v
                .get("output_mode")
                .and_then(|m| m.as_u64())
                .map(|m| m as u32),
        });
    }
    None
}

/// Structured fixture windows-report payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsReport {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub stdin_non_null: bool,
    pub stdout_non_null: bool,
    pub stderr_non_null: bool,
    pub stdin_file_type: Option<WinFileType>,
    pub stdout_file_type: Option<WinFileType>,
    pub stderr_file_type: Option<WinFileType>,
    pub stdin_console_mode: Option<u32>,
    pub stdout_console_mode: Option<u32>,
    pub winsize_rows: Option<u16>,
    pub winsize_cols: Option<u16>,
    pub input_mode: Option<u32>,
    pub output_mode: Option<u32>,
}

impl WindowsReport {
    pub fn std_handle_observation(&self) -> WindowsStdHandleObservation {
        WindowsStdHandleObservation {
            stdin_non_null: self.stdin_non_null,
            stdout_non_null: self.stdout_non_null,
            stderr_non_null: self.stderr_non_null,
            stdin_file_type: self.stdin_file_type,
            stdout_file_type: self.stdout_file_type,
            stderr_file_type: self.stderr_file_type,
            stdin_console_mode: self.stdin_console_mode,
            stdout_console_mode: self.stdout_console_mode,
            source: "fixture_windows_report".into(),
        }
    }

    pub fn console_mode_snapshot(&self) -> WindowsConsoleModeSnapshot {
        WindowsConsoleModeSnapshot::measured(
            self.input_mode.or(self.stdin_console_mode),
            self.output_mode.or(self.stdout_console_mode),
            "fixture_windows_report",
        )
    }

    pub fn dimensions(&self) -> WindowsConsoleDimensions {
        match (self.winsize_rows, self.winsize_cols) {
            (Some(rows), Some(cols)) => WindowsConsoleDimensions {
                rows: Some(rows),
                cols: Some(cols),
                available: true,
                source: "fixture_windows_report".into(),
            },
            _ => WindowsConsoleDimensions::unavailable("fixture dimensions missing"),
        }
    }
}

/// Wait helper: poll until predicate or deadline (no unbounded waits).
pub fn poll_until(budget: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if pred() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(WIN_POLL_SLICE_MS));
    }
}

/// Terminal outcome of one caller-visible write attempt (observation, not judgment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsWriteOutcome {
    /// Worker finished the requested bytes before the caller deadline.
    Completed,
    /// Worker returned after `CancelSynchronousIo` (or equivalent request).
    Cancelled,
    /// Synchronous `WriteFile` failed (including broken pipe / zero write).
    Failed,
    /// Caller deadline expired and worker completion was not observed within
    /// the cancellation bound (writer is poisoned; session must not reuse it).
    TimedOut,
}

impl WindowsWriteOutcome {
    pub fn stable_id(&self) -> &'static str {
        match self {
            WindowsWriteOutcome::Completed => "completed",
            WindowsWriteOutcome::Cancelled => "cancelled",
            WindowsWriteOutcome::Failed => "failed",
            WindowsWriteOutcome::TimedOut => "timed_out",
        }
    }
}

/// Bounded evidence for one caller-visible write (D2-022).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsWriteObservation {
    pub request_id: u64,
    pub requested_bytes: usize,
    pub written_bytes: usize,
    pub outcome: WindowsWriteOutcome,
    pub elapsed: Duration,
    pub cancel_requested: bool,
    pub worker_completed: bool,
    pub source: String,
}

impl WindowsWriteObservation {
    pub fn observation(&self) -> Observation {
        Observation::HarnessFailed {
            phase: format!("write.{}", self.outcome.stable_id()),
            detail: format!(
                "request_id={} requested={} written={} cancel_requested={} \
                 worker_completed={} elapsed_ms={} source={}",
                self.request_id,
                self.requested_bytes,
                self.written_bytes,
                self.cancel_requested,
                self.worker_completed,
                self.elapsed.as_millis(),
                self.source
            ),
        }
    }
}

/// Explicit Compat teardown stages (not implicit Drop side effects).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsConPtyTeardownStage {
    Running,
    StopAcceptingInput,
    CancelActiveWrite,
    TerminateWaitClient,
    BeginConPtyClose,
    OutputDrainContinues,
    CloseReturned,
    PipeBreakObserved,
    WorkersExit,
    HandlesClosedOnce,
    Closed,
}

impl WindowsConPtyTeardownStage {
    pub fn stable_id(&self) -> &'static str {
        match self {
            WindowsConPtyTeardownStage::Running => "running",
            WindowsConPtyTeardownStage::StopAcceptingInput => "stop_accepting_input",
            WindowsConPtyTeardownStage::CancelActiveWrite => "cancel_active_write",
            WindowsConPtyTeardownStage::TerminateWaitClient => "terminate_wait_client",
            WindowsConPtyTeardownStage::BeginConPtyClose => "begin_conpty_close",
            WindowsConPtyTeardownStage::OutputDrainContinues => "output_drain_continues",
            WindowsConPtyTeardownStage::CloseReturned => "close_returned",
            WindowsConPtyTeardownStage::PipeBreakObserved => "pipe_break_observed",
            WindowsConPtyTeardownStage::WorkersExit => "workers_exit",
            WindowsConPtyTeardownStage::HandlesClosedOnce => "handles_closed_once",
            WindowsConPtyTeardownStage::Closed => "closed",
        }
    }
}

/// Bounded evidence for one Compat ConPTY `shutdown_bounded` call (D2-022).
///
/// PASS law (control tier): `close_returned` AND input/output workers stopped
/// AND handles closed once AND elapsed within limit AND not `bounded_out`.
/// Escape via watchdog alone is HARNESS FAILURE, never PASS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsConPtyShutdownObservation {
    pub input_worker_stopped: bool,
    pub active_write_cancelled: bool,
    pub client_exit_observed: bool,
    pub close_started: bool,
    pub close_returned: bool,
    pub close_timed_out: bool,
    pub output_pipe_broken: bool,
    pub output_worker_stopped: bool,
    pub handles_closed_once: bool,
    pub elapsed: Duration,
    pub bounded_out: bool,
    pub source: String,
}

impl WindowsConPtyShutdownObservation {
    pub fn already_closed(source: &str) -> Self {
        Self {
            input_worker_stopped: true,
            active_write_cancelled: false,
            client_exit_observed: true,
            close_started: true,
            close_returned: true,
            close_timed_out: false,
            output_pipe_broken: false,
            output_worker_stopped: true,
            handles_closed_once: true,
            elapsed: Duration::ZERO,
            bounded_out: false,
            source: format!("{source}:already_closed"),
        }
    }

    /// Control-tier harness shutdown law (not a product invariant).
    pub fn harness_shutdown_pass(&self, limit: Duration) -> bool {
        !self.bounded_out
            && !self.close_timed_out
            && self.elapsed <= limit
            && self.close_started
            && self.close_returned
            && self.input_worker_stopped
            && self.output_worker_stopped
            && self.handles_closed_once
    }

    pub fn observation(&self) -> Observation {
        Observation::WindowsConPtyLifecycleObservation {
            phase: "shutdown_bounded".into(),
            ok: self.harness_shutdown_pass(Duration::from_secs(u64::MAX)),
            detail: format!(
                "input_stopped={} write_cancelled={} client_exit={} close_started={} \
                 close_returned={} close_timed_out={} pipe_broken={} output_stopped={} \
                 handles_closed={} elapsed_ms={} bounded_out={}",
                self.input_worker_stopped,
                self.active_write_cancelled,
                self.client_exit_observed,
                self.close_started,
                self.close_returned,
                self.close_timed_out,
                self.output_pipe_broken,
                self.output_worker_stopped,
                self.handles_closed_once,
                self.elapsed.as_millis(),
                self.bounded_out,
            ),
            source: self.source.clone(),
        }
    }
}
