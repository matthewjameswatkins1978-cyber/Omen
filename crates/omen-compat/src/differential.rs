use crate::core::{CommandSpec, ExitCause, OutputBounds};
use crate::normalization::{NormalizationError, NormalizationProfile};
use crate::runner::{CapturedStream, RunOutcome, run};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Comparison excludes PID, elapsed-time, and scheduling noise by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DifferentialStatus {
    Equivalent,
    Divergent,
    Inconclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DivergenceKind {
    ExitCause,
    Timeout,
    HarnessFailure,
    Stdout,
    Stderr,
}

/// Bounded comparison summary. Raw bytes stay in the transient RunOutcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamComparison {
    pub reference_bytes: usize,
    pub candidate_bytes: usize,
    pub reference_sha256: String,
    pub candidate_sha256: String,
    pub first_difference: Option<usize>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DifferentialResult {
    pub status: DifferentialStatus,
    pub differences: Vec<DivergenceKind>,
    pub reference_exit: Option<ExitCause>,
    pub candidate_exit: Option<ExitCause>,
    pub reference_timed_out: bool,
    pub candidate_timed_out: bool,
    pub reference_harness_failed: bool,
    pub candidate_harness_failed: bool,
    pub stdout: StreamComparison,
    pub stderr: StreamComparison,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DifferentialError {
    #[error(transparent)]
    Normalization(#[from] NormalizationError),
}

pub struct DifferentialBounds {
    pub max_normalized_bytes: usize,
    pub max_replacements: usize,
}

impl Default for DifferentialBounds {
    fn default() -> Self {
        Self {
            max_normalized_bytes: 16 * 1024 * 1024,
            max_replacements: 100_000,
        }
    }
}

/// Compare two independently captured runs. Normalization is explicit,
/// ordered, and bounded; it never edits either source observation.
/// Outcomes from executing and comparing one reference/candidate pair.
#[derive(Debug)]
pub struct DifferentialExecution {
    pub reference: RunOutcome,
    pub candidate: RunOutcome,
    pub comparison: DifferentialResult,
}

/// Run both subjects serially through the same bounded process supervisor,
/// then compare their captured outcomes. Serial order is explicit so a caller
/// does not accidentally introduce resource contention as comparison noise.
pub async fn execute_and_compare(
    reference: &CommandSpec,
    candidate: &CommandSpec,
    output_bounds: &OutputBounds,
    profile: &NormalizationProfile,
    comparison_bounds: DifferentialBounds,
) -> Result<DifferentialExecution, DifferentialError> {
    let reference_run = run(reference, output_bounds).await;
    let candidate_run = run(candidate, output_bounds).await;
    let comparison = compare_runs(&reference_run, &candidate_run, profile, comparison_bounds)?;

    Ok(DifferentialExecution {
        reference: reference_run,
        candidate: candidate_run,
        comparison,
    })
}
pub fn compare_runs(
    reference: &RunOutcome,
    candidate: &RunOutcome,
    profile: &NormalizationProfile,
    bounds: DifferentialBounds,
) -> Result<DifferentialResult, DifferentialError> {
    let stdout = compare_stream(&reference.stdout, &candidate.stdout, profile, &bounds)?;
    let stderr = compare_stream(&reference.stderr, &candidate.stderr, profile, &bounds)?;
    let mut differences = Vec::new();

    if reference.exit_cause != candidate.exit_cause {
        differences.push(DivergenceKind::ExitCause);
    }
    if reference.timed_out != candidate.timed_out {
        differences.push(DivergenceKind::Timeout);
    }
    if reference.harness_failed != candidate.harness_failed {
        differences.push(DivergenceKind::HarnessFailure);
    }
    if stdout.first_difference.is_some() {
        differences.push(DivergenceKind::Stdout);
    }
    if stderr.first_difference.is_some() {
        differences.push(DivergenceKind::Stderr);
    }

    let incomplete = !stdout.complete
        || !stderr.complete
        || reference.timed_out
        || candidate.timed_out
        || reference.harness_failed
        || candidate.harness_failed
        || (reference.exit_cause.is_none() && !reference.timed_out)
        || (candidate.exit_cause.is_none() && !candidate.timed_out);
    let status = if !differences.is_empty() {
        DifferentialStatus::Divergent
    } else if incomplete {
        DifferentialStatus::Inconclusive
    } else {
        DifferentialStatus::Equivalent
    };

    Ok(DifferentialResult {
        status,
        differences,
        reference_exit: reference.exit_cause.clone(),
        candidate_exit: candidate.exit_cause.clone(),
        reference_timed_out: reference.timed_out,
        candidate_timed_out: candidate.timed_out,
        reference_harness_failed: reference.harness_failed,
        candidate_harness_failed: candidate.harness_failed,
        stdout,
        stderr,
    })
}

fn compare_stream(
    reference: &CapturedStream,
    candidate: &CapturedStream,
    profile: &NormalizationProfile,
    bounds: &DifferentialBounds,
) -> Result<StreamComparison, DifferentialError> {
    let left = profile.apply(
        &reference.bytes,
        bounds.max_normalized_bytes,
        bounds.max_replacements,
    )?;
    let right = profile.apply(
        &candidate.bytes,
        bounds.max_normalized_bytes,
        bounds.max_replacements,
    )?;
    let complete = !reference.truncated
        && !candidate.truncated
        && !reference.drain_timed_out
        && !candidate.drain_timed_out
        && reference.drain_complete
        && candidate.drain_complete;

    Ok(StreamComparison {
        reference_bytes: left.bytes.len(),
        candidate_bytes: right.bytes.len(),
        reference_sha256: sha256(&left.bytes),
        candidate_sha256: sha256(&right.bytes),
        first_difference: first_difference(&left.bytes, &right.bytes),
        complete,
    })
}

fn first_difference(left: &[u8], right: &[u8]) -> Option<usize> {
    left.iter()
        .zip(right)
        .position(|(left_byte, right_byte)| left_byte != right_byte)
        .or_else(|| (left.len() != right.len()).then_some(left.len().min(right.len())))
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ExitCause;
    use crate::core::OutputBounds;
    use crate::normalization::NormalizationRule;

    fn outcome(bytes: &[u8], complete: bool) -> RunOutcome {
        RunOutcome {
            observations: Vec::new(),
            exit_cause: Some(ExitCause::Code { code: 0 }),
            stdout: CapturedStream {
                bytes: bytes.to_vec(),
                total_bytes: bytes.len() as u64,
                truncated: !complete,
                eof_observed: complete,
                drain_complete: complete,
                drain_timed_out: false,
            },
            stderr: CapturedStream {
                bytes: Vec::new(),
                total_bytes: 0,
                truncated: false,
                eof_observed: true,
                drain_complete: true,
                drain_timed_out: false,
            },
            timed_out: false,
            cleanup: Default::default(),
            elapsed_ms: 1,
            deadline_ms: 100,
            declared_max_wall_ms: 975,
            bounds: OutputBounds::default(),
            harness_failed: false,
            pid: Some(1),
        }
    }

    #[test]
    fn explicit_normalization_can_prove_byte_equivalence() {
        let profile = NormalizationProfile::new(vec![NormalizationRule::CrLfToLf]);
        let result = compare_runs(
            &outcome(b"alpha\r\nbeta", true),
            &outcome(b"alpha\nbeta", true),
            &profile,
            DifferentialBounds::default(),
        )
        .unwrap();
        assert_eq!(result.status, DifferentialStatus::Equivalent);
        assert!(result.differences.is_empty());
        assert_eq!(
            result.stdout.reference_sha256,
            result.stdout.candidate_sha256
        );
    }

    #[test]
    fn an_observed_prefix_mismatch_is_divergent_even_if_truncated() {
        let result = compare_runs(
            &outcome(b"reference-prefix", false),
            &outcome(b"candidate-prefix", false),
            &NormalizationProfile::default(),
            DifferentialBounds::default(),
        )
        .unwrap();
        assert_eq!(result.status, DifferentialStatus::Divergent);
        assert!(result.differences.contains(&DivergenceKind::Stdout));
    }

    #[test]
    fn equal_truncated_prefix_is_inconclusive_not_equivalent() {
        let result = compare_runs(
            &outcome(b"same-prefix", false),
            &outcome(b"same-prefix", false),
            &NormalizationProfile::default(),
            DifferentialBounds::default(),
        )
        .unwrap();
        assert_eq!(result.status, DifferentialStatus::Inconclusive);
    }

    #[test]
    fn matching_timed_out_runs_are_inconclusive() {
        let mut reference = outcome(b"same-prefix", true);
        reference.exit_cause = None;
        reference.timed_out = true;
        let candidate = reference.clone();
        let result = compare_runs(
            &reference,
            &candidate,
            &NormalizationProfile::default(),
            DifferentialBounds::default(),
        )
        .unwrap();
        assert_eq!(result.status, DifferentialStatus::Inconclusive);
    }
    #[test]
    fn normalization_cannot_hide_exit_semantics() {
        let mut candidate = outcome(b"same", true);
        candidate.exit_cause = Some(ExitCause::Code { code: 7 });
        let result = compare_runs(
            &outcome(b"same", true),
            &candidate,
            &NormalizationProfile::new(vec![NormalizationRule::Replace {
                from: b"same".to_vec(),
                to: Vec::new(),
            }]),
            DifferentialBounds {
                max_normalized_bytes: 64,
                max_replacements: 4,
            },
        )
        .unwrap();
        assert_eq!(result.status, DifferentialStatus::Divergent);
        assert!(result.differences.contains(&DivergenceKind::ExitCause));
    }
}
