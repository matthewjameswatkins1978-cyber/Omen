use crate::core::EvidenceGrade;
use crate::observation::Observation;
use serde::{Deserialize, Serialize};

/// Stable machine-readable invariant identity. Prose is presentation only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InvariantId {
    BoundedWaitNoHang,
    ExitCausePreserved,
    DrainBothStreamsNoDeadlock,
    DescriptorClosureEarlyExit,
    ZeroUnscriptedInputWrites,
    EnvRecorded,
    ShellJobSharesShellSession,
    ShellJobHasDistinctProcessGroup,
    TerminalForegroundPgrpIsJob,
    ShellRegainsTtyAfterJobStop,
    ShellRegainsTtyAfterJobExit,
    WaitObservesStoppedState,
    TerminalSigintTargetsForegroundJob,
    ShellSurvivesForegroundJobSigint,
    ExitStatusPreservesSignalNumber,
    ChildSignalMaskUnblockedBeforeExec,
    SigwinchAsyncDeliveredToForegroundPgrpOnResize,
    ShellTermiosSnapshotRestoredAfterAbnormalChildExit,
    NoZombieChildrenOfShell,
}

impl InvariantId {
    pub fn stable_id(&self) -> &'static str {
        match self {
            InvariantId::BoundedWaitNoHang => "BOUNDED_WAIT_NO_HANG",
            InvariantId::ExitCausePreserved => "EXIT_CAUSE_PRESERVED",
            InvariantId::DrainBothStreamsNoDeadlock => "DRAIN_BOTH_STREAMS_NO_DEADLOCK",
            InvariantId::DescriptorClosureEarlyExit => "DESCRIPTOR_CLOSURE_EARLY_EXIT",
            InvariantId::ZeroUnscriptedInputWrites => "ZERO_UNSCRIPTED_INPUT_WRITES",
            InvariantId::EnvRecorded => "ENV_RECORDED",
            InvariantId::ShellJobSharesShellSession => "SHELL_JOB_SHARES_SHELL_SESSION",
            InvariantId::ShellJobHasDistinctProcessGroup => "SHELL_JOB_HAS_DISTINCT_PROCESS_GROUP",
            InvariantId::TerminalForegroundPgrpIsJob => "TERMINAL_FOREGROUND_PGRP_IS_JOB",
            InvariantId::ShellRegainsTtyAfterJobStop => "SHELL_REGAINS_TTY_AFTER_JOB_STOP",
            InvariantId::ShellRegainsTtyAfterJobExit => "SHELL_REGAINS_TTY_AFTER_JOB_EXIT",
            InvariantId::WaitObservesStoppedState => "WAIT_OBSERVES_STOPPED_STATE",
            InvariantId::TerminalSigintTargetsForegroundJob => {
                "TERMINAL_SIGINT_TARGETS_FOREGROUND_JOB"
            }
            InvariantId::ShellSurvivesForegroundJobSigint => "SHELL_SURVIVES_FOREGROUND_JOB_SIGINT",
            InvariantId::ExitStatusPreservesSignalNumber => "EXIT_STATUS_PRESERVES_SIGNAL_NUMBER",
            InvariantId::ChildSignalMaskUnblockedBeforeExec => {
                "CHILD_SIGNAL_MASK_UNBLOCKED_BEFORE_EXEC"
            }
            InvariantId::SigwinchAsyncDeliveredToForegroundPgrpOnResize => {
                "SIGWINCH_ASYNC_DELIVERED_TO_FOREGROUND_PGRP_ON_RESIZE"
            }
            InvariantId::ShellTermiosSnapshotRestoredAfterAbnormalChildExit => {
                "SHELL_TERMIOS_SNAPSHOT_RESTORED_AFTER_ABNORMAL_CHILD_EXIT"
            }
            InvariantId::NoZombieChildrenOfShell => "NO_ZOMBIE_CHILDREN_OF_SHELL",
        }
    }

    pub fn from_stable_id(id: &str) -> Option<Self> {
        match id {
            "BOUNDED_WAIT_NO_HANG" => Some(InvariantId::BoundedWaitNoHang),
            "EXIT_CAUSE_PRESERVED" => Some(InvariantId::ExitCausePreserved),
            "DRAIN_BOTH_STREAMS_NO_DEADLOCK" => Some(InvariantId::DrainBothStreamsNoDeadlock),
            "DESCRIPTOR_CLOSURE_EARLY_EXIT" => Some(InvariantId::DescriptorClosureEarlyExit),
            "ZERO_UNSCRIPTED_INPUT_WRITES" => Some(InvariantId::ZeroUnscriptedInputWrites),
            "ENV_RECORDED" => Some(InvariantId::EnvRecorded),
            "SHELL_JOB_SHARES_SHELL_SESSION" => Some(InvariantId::ShellJobSharesShellSession),
            "SHELL_JOB_HAS_DISTINCT_PROCESS_GROUP" => {
                Some(InvariantId::ShellJobHasDistinctProcessGroup)
            }
            "TERMINAL_FOREGROUND_PGRP_IS_JOB" => Some(InvariantId::TerminalForegroundPgrpIsJob),
            "SHELL_REGAINS_TTY_AFTER_JOB_STOP" => Some(InvariantId::ShellRegainsTtyAfterJobStop),
            "SHELL_REGAINS_TTY_AFTER_JOB_EXIT" => Some(InvariantId::ShellRegainsTtyAfterJobExit),
            "WAIT_OBSERVES_STOPPED_STATE" => Some(InvariantId::WaitObservesStoppedState),
            "TERMINAL_SIGINT_TARGETS_FOREGROUND_JOB" => {
                Some(InvariantId::TerminalSigintTargetsForegroundJob)
            }
            "SHELL_SURVIVES_FOREGROUND_JOB_SIGINT" => {
                Some(InvariantId::ShellSurvivesForegroundJobSigint)
            }
            "EXIT_STATUS_PRESERVES_SIGNAL_NUMBER" => {
                Some(InvariantId::ExitStatusPreservesSignalNumber)
            }
            "CHILD_SIGNAL_MASK_UNBLOCKED_BEFORE_EXEC" => {
                Some(InvariantId::ChildSignalMaskUnblockedBeforeExec)
            }
            "SIGWINCH_ASYNC_DELIVERED_TO_FOREGROUND_PGRP_ON_RESIZE" => {
                Some(InvariantId::SigwinchAsyncDeliveredToForegroundPgrpOnResize)
            }
            "SHELL_TERMIOS_SNAPSHOT_RESTORED_AFTER_ABNORMAL_CHILD_EXIT" => {
                Some(InvariantId::ShellTermiosSnapshotRestoredAfterAbnormalChildExit)
            }
            "NO_ZOMBIE_CHILDREN_OF_SHELL" => Some(InvariantId::NoZombieChildrenOfShell),
            _ => None,
        }
    }

    pub const ALL: [InvariantId; 19] = [
        InvariantId::BoundedWaitNoHang,
        InvariantId::ExitCausePreserved,
        InvariantId::DrainBothStreamsNoDeadlock,
        InvariantId::DescriptorClosureEarlyExit,
        InvariantId::ZeroUnscriptedInputWrites,
        InvariantId::EnvRecorded,
        InvariantId::ShellJobSharesShellSession,
        InvariantId::ShellJobHasDistinctProcessGroup,
        InvariantId::TerminalForegroundPgrpIsJob,
        InvariantId::ShellRegainsTtyAfterJobStop,
        InvariantId::ShellRegainsTtyAfterJobExit,
        InvariantId::WaitObservesStoppedState,
        InvariantId::TerminalSigintTargetsForegroundJob,
        InvariantId::ShellSurvivesForegroundJobSigint,
        InvariantId::ExitStatusPreservesSignalNumber,
        InvariantId::ChildSignalMaskUnblockedBeforeExec,
        InvariantId::SigwinchAsyncDeliveredToForegroundPgrpOnResize,
        InvariantId::ShellTermiosSnapshotRestoredAfterAbnormalChildExit,
        InvariantId::NoZombieChildrenOfShell,
    ];
}

/// Richer than bool: PASS/FAIL is not the whole truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InvariantOutcome {
    Pass,
    Fail,
    Unsupported,
    Unavailable,
    OpenDefect,
    Inconclusive,
}

impl InvariantOutcome {
    pub fn stable_id(&self) -> &'static str {
        match self {
            InvariantOutcome::Pass => "PASS",
            InvariantOutcome::Fail => "FAIL",
            InvariantOutcome::Unsupported => "UNSUPPORTED",
            InvariantOutcome::Unavailable => "UNAVAILABLE",
            InvariantOutcome::OpenDefect => "OPEN_DEFECT",
            InvariantOutcome::Inconclusive => "INCONCLUSIVE",
        }
    }

    pub fn is_failure(&self) -> bool {
        matches!(self, InvariantOutcome::Fail | InvariantOutcome::OpenDefect)
    }
}

/// Judge result for one invariant against a set of observations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvariantResult {
    pub invariant: InvariantId,
    pub outcome: InvariantOutcome,
    pub evidence_grade: EvidenceGrade,
    pub reason: String,
    #[serde(default)]
    pub observations: Vec<Observation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

impl InvariantResult {
    pub fn new(
        invariant: InvariantId,
        outcome: InvariantOutcome,
        evidence_grade: EvidenceGrade,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            invariant,
            outcome,
            evidence_grade,
            reason: reason.into(),
            observations: Vec::new(),
            reference: None,
        }
    }

    pub fn pass(
        invariant: InvariantId,
        evidence_grade: EvidenceGrade,
        reason: impl Into<String>,
    ) -> Self {
        Self::new(invariant, InvariantOutcome::Pass, evidence_grade, reason)
    }

    pub fn fail(
        invariant: InvariantId,
        evidence_grade: EvidenceGrade,
        reason: impl Into<String>,
    ) -> Self {
        Self::new(invariant, InvariantOutcome::Fail, evidence_grade, reason)
    }

    pub fn unavailable(invariant: InvariantId, reason: impl Into<String>) -> Self {
        Self::new(
            invariant,
            InvariantOutcome::Unavailable,
            EvidenceGrade::Unavailable,
            reason,
        )
    }

    pub fn inconclusive(
        invariant: InvariantId,
        evidence_grade: EvidenceGrade,
        reason: impl Into<String>,
    ) -> Self {
        Self::new(
            invariant,
            InvariantOutcome::Inconclusive,
            evidence_grade,
            reason,
        )
    }

    pub fn with_observations(mut self, observations: Vec<Observation>) -> Self {
        self.observations = observations;
        self
    }

    pub fn with_reference(mut self, reference: impl Into<String>) -> Self {
        self.reference = Some(reference.into());
        self
    }
}
