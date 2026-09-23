use crate::core::ExitCause;
use serde::{Deserialize, Serialize};

/// Which standard stream an observation refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Stdout,
    Stderr,
    Stdin,
}

impl StreamKind {
    pub fn stable_id(&self) -> &'static str {
        match self {
            StreamKind::Stdout => "stdout",
            StreamKind::Stderr => "stderr",
            StreamKind::Stdin => "stdin",
        }
    }
}

/// Evidence of what happened. Observations are not conclusions.
///
/// Durable observations must not retain secret-bearing execution values
/// (environment values, stdin payloads, arbitrary stdout text).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Observation {
    ProcessSpawned {
        pid: u32,
    },
    StdoutChunk {
        bytes: u64,
    },
    StderrChunk {
        bytes: u64,
    },
    ExitObserved {
        cause: ExitCause,
    },
    TimeoutObserved {
        elapsed_ms: u64,
        deadline_ms: u64,
    },
    ProcessStillAlive {
        pid: u32,
    },
    ProcessTreeObserved {
        root_pid: u32,
        child_pids: Vec<u32>,
        source: String,
    },
    StdinClosed,
    StdinWrote {
        bytes: u64,
    },
    StdinDeliveryBounded {
        bytes_attempted: u64,
        completed: bool,
    },
    DescriptorClosed {
        stream: StreamKind,
    },
    OutputTruncated {
        stream: StreamKind,
        kept_bytes: u64,
        total_bytes: u64,
    },
    OutputComplete {
        stream: StreamKind,
        total_bytes: u64,
    },
    /// Stream drain stopped at the declared grace bound without proving EOF.
    StreamDrainBoundedOut {
        stream: StreamKind,
        total_bytes_observed: u64,
    },
    CleanupAttempted {
        method: String,
        kill_succeeded: bool,
        reaped: bool,
    },
    /// Environment key applied to the child. Values are never retained.
    EnvApplied {
        key: String,
    },
    /// Controlled Compat fixture report (not arbitrary child JSON).
    FixtureReport {
        fixture: String,
        payload: serde_json::Value,
    },
    HarnessFailed {
        phase: String,
        detail: String,
    },
}

impl Observation {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Observation::ProcessSpawned { .. } => "process_spawned",
            Observation::StdoutChunk { .. } => "stdout_chunk",
            Observation::StderrChunk { .. } => "stderr_chunk",
            Observation::ExitObserved { .. } => "exit_observed",
            Observation::TimeoutObserved { .. } => "timeout_observed",
            Observation::ProcessStillAlive { .. } => "process_still_alive",
            Observation::ProcessTreeObserved { .. } => "process_tree_observed",
            Observation::StdinClosed => "stdin_closed",
            Observation::StdinWrote { .. } => "stdin_wrote",
            Observation::StdinDeliveryBounded { .. } => "stdin_delivery_bounded",
            Observation::DescriptorClosed { .. } => "descriptor_closed",
            Observation::OutputTruncated { .. } => "output_truncated",
            Observation::OutputComplete { .. } => "output_complete",
            Observation::StreamDrainBoundedOut { .. } => "stream_drain_bounded_out",
            Observation::CleanupAttempted { .. } => "cleanup_attempted",
            Observation::EnvApplied { .. } => "env_applied",
            Observation::FixtureReport { .. } => "fixture_report",
            Observation::HarnessFailed { .. } => "harness_failed",
        }
    }
}
