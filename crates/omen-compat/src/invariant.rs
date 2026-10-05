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
            _ => None,
        }
    }

    pub const ALL: [InvariantId; 6] = [
        InvariantId::BoundedWaitNoHang,
        InvariantId::ExitCausePreserved,
        InvariantId::DrainBothStreamsNoDeadlock,
        InvariantId::DescriptorClosureEarlyExit,
        InvariantId::ZeroUnscriptedInputWrites,
        InvariantId::EnvRecorded,
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
