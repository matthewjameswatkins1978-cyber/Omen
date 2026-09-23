use crate::core::{CommandSpec, Deadline, EvidenceGrade, ExitCause, OutputBounds, Platform};
use crate::invariant::{InvariantId, InvariantResult};
use crate::observation::Observation;
use serde::{Deserialize, Serialize};
use std::fmt;

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

/// Summary of a captured stream for durable evidence.
///
/// Never includes raw textual output by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StreamSummary {
    pub kept_bytes: u64,
    pub total_bytes: u64,
    pub truncated: bool,
    pub eof_observed: bool,
    pub drain_bounded_out: bool,
}

impl StreamSummary {
    pub fn from_captured(
        kept_bytes: u64,
        total_bytes: u64,
        truncated: bool,
        eof_observed: bool,
        drain_bounded_out: bool,
    ) -> Self {
        Self {
            kept_bytes,
            total_bytes,
            truncated,
            eof_observed,
            drain_bounded_out,
        }
    }
}

/// Secret-safe projection of a process invocation for durable evidence.
///
/// Does not retain environment values, stdin bytes, or arbitrary argv.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEvidence {
    pub program: String,
    pub argv_count: usize,
    /// Explicitly classified-safe argument literals (opt-in only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv_safe: Option<Vec<String>>,
    pub cwd_policy: String,
    pub env_policy_kind: String,
    pub env_keys: Vec<String>,
    pub stdin_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_byte_length: Option<usize>,
    pub deadline: Deadline,
}

impl CommandEvidence {
    /// Redacted by default: structure and counts only.
    pub fn from_execution(spec: &CommandSpec) -> Self {
        Self {
            program: spec.program.display().to_string(),
            argv_count: spec.argv.len(),
            argv_safe: None,
            cwd_policy: match &spec.cwd {
                Some(_) => "set".to_string(),
                None => "inherit".to_string(),
            },
            env_policy_kind: spec.env.kind_name().to_string(),
            env_keys: spec.env.key_names(),
            stdin_mode: spec.stdin.mode_name().to_string(),
            stdin_byte_length: spec.stdin.byte_length(),
            deadline: spec.deadline,
        }
    }

    /// Caller explicitly marks argv literals as safe to persist.
    pub fn with_safe_argv(mut self, argv: Vec<String>) -> Self {
        self.argv_safe = Some(argv);
        self
    }
}

/// How much of the original execution state a replay record retains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayFidelity {
    /// Required values retained; safe for controlled fixtures without secrets.
    Exact,
    /// Some required values retained; others intentionally omitted.
    Partial,
    /// Values intentionally omitted; not an exact replay.
    Redacted,
}

/// Durable replay description. Never a blind clone of execution state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayDescriptor {
    pub program: String,
    pub argv: Vec<String>,
    pub argv_count: usize,
    pub cwd_policy: String,
    pub env_policy_kind: String,
    pub env_keys: Vec<String>,
    pub stdin_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_byte_length: Option<usize>,
    pub deadline: Deadline,
    pub fixture_mode: String,
    pub expected_invariants: Vec<InvariantId>,
    pub fidelity: ReplayFidelity,
}

impl ReplayDescriptor {
    /// Explicit redacted projection: counts, kinds, and key names only.
    pub fn redacted(
        spec: &CommandSpec,
        fixture_mode: impl Into<String>,
        expected_invariants: Vec<InvariantId>,
    ) -> Self {
        let evidence = CommandEvidence::from_execution(spec);
        Self {
            program: evidence.program,
            argv: Vec::new(),
            argv_count: evidence.argv_count,
            cwd_policy: evidence.cwd_policy,
            env_policy_kind: evidence.env_policy_kind,
            env_keys: evidence.env_keys,
            stdin_mode: evidence.stdin_mode,
            stdin_byte_length: evidence.stdin_byte_length,
            deadline: evidence.deadline,
            fixture_mode: fixture_mode.into(),
            expected_invariants,
            fidelity: ReplayFidelity::Redacted,
        }
    }

    /// Controlled fixture replay: caller explicitly marks argv as safe.
    pub fn exact_fixture(
        spec: &CommandSpec,
        fixture_mode: impl Into<String>,
        expected_invariants: Vec<InvariantId>,
        safe_argv: Vec<String>,
    ) -> Self {
        let evidence = CommandEvidence::from_execution(spec).with_safe_argv(safe_argv.clone());
        Self {
            program: evidence.program,
            argv: safe_argv,
            argv_count: evidence.argv_count,
            cwd_policy: evidence.cwd_policy,
            env_policy_kind: evidence.env_policy_kind,
            env_keys: evidence.env_keys,
            stdin_mode: evidence.stdin_mode,
            stdin_byte_length: evidence.stdin_byte_length,
            deadline: evidence.deadline,
            fixture_mode: fixture_mode.into(),
            expected_invariants,
            fidelity: ReplayFidelity::Exact,
        }
    }

    pub fn from_evidence(
        evidence: CommandEvidence,
        fixture_mode: impl Into<String>,
        expected_invariants: Vec<InvariantId>,
        fidelity: ReplayFidelity,
    ) -> Self {
        let argv = evidence.argv_safe.clone().unwrap_or_default();
        Self {
            program: evidence.program,
            argv,
            argv_count: evidence.argv_count,
            cwd_policy: evidence.cwd_policy,
            env_policy_kind: evidence.env_policy_kind,
            env_keys: evidence.env_keys,
            stdin_mode: evidence.stdin_mode,
            stdin_byte_length: evidence.stdin_byte_length,
            deadline: evidence.deadline,
            fixture_mode: fixture_mode.into(),
            expected_invariants,
            fidelity,
        }
    }
}

/// Replayable failure evidence. Secret-bearing execution state is projected
/// through [`CommandEvidence`], never embedded raw.
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
    pub command: Option<CommandEvidence>,
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

    /// Persist secret-safe command evidence only (never raw CommandSpec).
    pub fn command_evidence(mut self, spec: &CommandSpec) -> Self {
        let evidence = CommandEvidence::from_execution(spec);
        self.failure.deadline = Some(evidence.deadline);
        self.failure.command = Some(evidence);
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

impl fmt::Display for ReplayFidelity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayFidelity::Exact => f.write_str("exact"),
            ReplayFidelity::Partial => f.write_str("partial"),
            ReplayFidelity::Redacted => f.write_str("redacted"),
        }
    }
}
