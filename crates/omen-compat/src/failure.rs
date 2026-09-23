use crate::core::{CommandSpec, Deadline, EvidenceGrade, ExitCause, OutputBounds, Platform};
use crate::invariant::{InvariantId, InvariantResult};
use crate::observation::Observation;
use serde::{Deserialize, Serialize};

/// Deterministic classification of a structured failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// Observations contradict the invariant.
    Contradiction,
    /// Required evidence could not be obtained in this environment.
    MissingEvidence,
    /// Harness or fixture failed before the invariant could be judged.
    HarnessError,
    /// Known open defect intentionally preserved as evidence.
    KnownOpenDefect,
    /// Judge produced an inconclusive outcome without a hard contradiction.
    Inconclusive,
}

/// Summary of a captured stream suitable for failure records (not a full dump).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StreamSummary {
    pub kept_bytes: u64,
    pub total_bytes: u64,
    pub truncated: bool,
    pub preview: String,
}

impl StreamSummary {
    pub fn from_capture(kept: &[u8], total: u64, truncated: bool) -> Self {
        let preview_limit = 200usize;
        let preview = String::from_utf8_lossy(kept)
            .chars()
            .take(preview_limit)
            .collect::<String>();
        Self {
            kept_bytes: kept.len() as u64,
            total_bytes: total,
            truncated,
            preview,
        }
    }
}

/// Enough information to deterministically re-run a portable probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayDescriptor {
    pub program: String,
    pub argv: Vec<String>,
    pub cwd_policy: String,
    pub env_policy: String,
    pub stdin: crate::core::StdinSpec,
    pub deadline: Deadline,
    pub fixture_mode: String,
    pub expected_invariants: Vec<InvariantId>,
}

impl ReplayDescriptor {
    pub fn from_command(
        spec: &CommandSpec,
        fixture_mode: impl Into<String>,
        expected_invariants: Vec<InvariantId>,
    ) -> Self {
        let env_policy = match &spec.env {
            crate::core::EnvPolicy::Clear => "clear".to_string(),
            crate::core::EnvPolicy::AllowList(pairs) => format!("allow_list:{pairs:?}"),
            crate::core::EnvPolicy::InheritWith(pairs) => format!("inherit_with:{pairs:?}"),
        };
        let cwd_policy = match &spec.cwd {
            Some(path) => format!("set:{}", path.display()),
            None => "inherit".to_string(),
        };
        Self {
            program: spec.program.display().to_string(),
            argv: spec.argv.clone(),
            cwd_policy,
            env_policy,
            stdin: spec.stdin.clone(),
            deadline: spec.deadline,
            fixture_mode: fixture_mode.into(),
            expected_invariants,
        }
    }
}

/// Replayable failure evidence. Values are omitted when unknown rather than
/// invented.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredFailure {
    pub invariant: InvariantId,
    pub platform: Platform,
    pub scenario: String,
    pub failure_kind: FailureKind,
    pub evidence_grade: EvidenceGrade,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<Deadline>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_cause: Option<ExitCause>,
    #[serde(default)]
    pub stdout: StreamSummary,
    #[serde(default)]
    pub stderr: StreamSummary,
    #[serde(default)]
    pub observations: Vec<Observation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<InvariantResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay: Option<ReplayDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimal_reproducer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub likely_subsystem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_bounds: Option<OutputBounds>,
}

impl StructuredFailure {
    pub fn builder(
        invariant: InvariantId,
        platform: Platform,
        scenario: impl Into<String>,
        failure_kind: FailureKind,
        evidence_grade: EvidenceGrade,
        reason: impl Into<String>,
    ) -> StructuredFailureBuilder {
        StructuredFailureBuilder {
            failure: Self {
                invariant,
                platform,
                scenario: scenario.into(),
                failure_kind,
                evidence_grade,
                reason: reason.into(),
                seed: None,
                command: None,
                deadline: None,
                exit_cause: None,
                stdout: StreamSummary::default(),
                stderr: StreamSummary::default(),
                observations: Vec::new(),
                result: None,
                replay: None,
                minimal_reproducer: None,
                likely_subsystem: None,
                output_bounds: None,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct StructuredFailureBuilder {
    failure: StructuredFailure,
}

impl StructuredFailureBuilder {
    pub fn seed(mut self, seed: u64) -> Self {
        self.failure.seed = Some(seed);
        self
    }

    pub fn command(mut self, command: CommandSpec) -> Self {
        self.failure.deadline = Some(command.deadline);
        self.failure.command = Some(command);
        self
    }

    pub fn exit_cause(mut self, cause: ExitCause) -> Self {
        self.failure.exit_cause = Some(cause);
        self
    }

    pub fn stdout(mut self, summary: StreamSummary) -> Self {
        self.failure.stdout = summary;
        self
    }

    pub fn stderr(mut self, summary: StreamSummary) -> Self {
        self.failure.stderr = summary;
        self
    }

    pub fn observations(mut self, observations: Vec<Observation>) -> Self {
        self.failure.observations = observations;
        self
    }

    pub fn result(mut self, result: InvariantResult) -> Self {
        self.failure.result = Some(result);
        self
    }

    pub fn replay(mut self, replay: ReplayDescriptor) -> Self {
        self.failure.replay = Some(replay);
        self
    }

    pub fn minimal_reproducer(mut self, reproducer: impl Into<String>) -> Self {
        self.failure.minimal_reproducer = Some(reproducer.into());
        self
    }

    pub fn likely_subsystem(mut self, subsystem: impl Into<String>) -> Self {
        self.failure.likely_subsystem = Some(subsystem.into());
        self
    }

    pub fn output_bounds(mut self, bounds: OutputBounds) -> Self {
        self.failure.output_bounds = Some(bounds);
        self
    }

    pub fn build(self) -> StructuredFailure {
        self.failure
    }
}
