use serde::Serialize;
use std::future::Future;
use thiserror::Error;

pub const MAX_SHRINK_ATTEMPTS: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShrinkPlan {
    pub max_attempts: usize,
    pub predicate_id: String,
}

impl ShrinkPlan {
    pub fn new(max_attempts: usize, predicate_id: impl Into<String>) -> Result<Self, ShrinkError> {
        let predicate_id = predicate_id.into();
        if max_attempts == 0 || max_attempts > MAX_SHRINK_ATTEMPTS {
            return Err(ShrinkError::InvalidAttemptBound);
        }
        if predicate_id.trim().is_empty() || predicate_id.len() > 128 {
            return Err(ShrinkError::InvalidPredicateId);
        }
        Ok(Self {
            max_attempts,
            predicate_id,
        })
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ShrinkError {
    #[error("shrink attempt bound must be between 1 and {MAX_SHRINK_ATTEMPTS}")]
    InvalidAttemptBound,
    #[error("failure predicate id must be 1..=128 non-whitespace bytes")]
    InvalidPredicateId,
    #[error("initial input does not satisfy the declared failure predicate")]
    InitialInputDoesNotFail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShrinkReport<T> {
    pub predicate_id: String,
    pub original_items: usize,
    pub minimized: Vec<T>,
    pub attempts: usize,
    pub minimal: bool,
}

/// Deterministic delta-debugging over an ordered structured input. A removal is
/// accepted only if the caller's named failure predicate still holds. Predicate
/// calls must be bounded; process-backed predicates should await the Compat Runner.
pub async fn shrink_sequence<T, F, Fut>(
    input: &[T],
    plan: &ShrinkPlan,
    mut still_fails: F,
) -> Result<ShrinkReport<T>, ShrinkError>
where
    T: Clone,
    F: FnMut(Vec<T>) -> Fut,
    Fut: Future<Output = bool>,
{
    if !still_fails(input.to_vec()).await {
        return Err(ShrinkError::InitialInputDoesNotFail);
    }

    let original_items = input.len();
    let mut current = input.to_vec();
    let mut attempts = 1usize;
    let mut granularity = 2usize;
    let mut minimal = current.is_empty();

    while !current.is_empty() && attempts < plan.max_attempts && !minimal {
        granularity = granularity.min(current.len()).max(1);
        let chunk_size = current.len().div_ceil(granularity);
        let mut reduced = false;

        for start in (0..current.len()).step_by(chunk_size) {
            if attempts >= plan.max_attempts {
                break;
            }
            let end = (start + chunk_size).min(current.len());
            let mut candidate = Vec::with_capacity(current.len() - (end - start));
            candidate.extend_from_slice(&current[..start]);
            candidate.extend_from_slice(&current[end..]);
            attempts += 1;

            if still_fails(candidate.clone()).await {
                current = candidate;
                granularity = granularity.saturating_sub(1).max(2);
                reduced = true;
                break;
            }
        }

        if !reduced {
            if granularity >= current.len() {
                minimal = true;
            } else {
                granularity = (granularity * 2).min(current.len());
            }
        }
    }

    if current.is_empty() {
        minimal = true;
    }

    Ok(ShrinkReport {
        predicate_id: plan.predicate_id.clone(),
        original_items,
        minimized: current,
        attempts,
        minimal,
    })
}

#[derive(Serialize)]
struct MinimalRegression<'a, T> {
    schema: &'static str,
    failure_predicate: &'a str,
    original_items: usize,
    shrink_attempts: usize,
    minimal: bool,
    input: &'a [T],
}

impl<T: Serialize> ShrinkReport<T> {
    /// Render the minimized input with its predicate and minimization evidence.
    /// This records whether the attempt budget proved minimality; it never
    /// upgrades a budget-limited reduction into a minimality claim.
    pub fn render_regression_fixture(&self) -> Result<Vec<u8>, serde_json::Error> {
        let record = MinimalRegression {
            schema: "omen.compat.regression.v1",
            failure_predicate: &self.predicate_id,
            original_items: self.original_items,
            shrink_attempts: self.attempts,
            minimal: self.minimal,
            input: &self.minimized,
        };
        let canonical = serde_json::to_value(record)?;
        serde_json::to_vec_pretty(&canonical)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shrinker_preserves_the_failure_and_reduces_noise_deterministically() {
        let plan = ShrinkPlan::new(100, "case.contains_bug_token").unwrap();
        let input = ["noise-a", "noise-b", "BUG", "noise-c", "noise-d"];
        let report = shrink_sequence(&input, &plan, |items| async move { items.contains(&"BUG") })
            .await
            .unwrap();
        assert_eq!(report.minimized, vec!["BUG"]);
        assert!(report.minimal);
        let fixture: serde_json::Value =
            serde_json::from_slice(&report.render_regression_fixture().unwrap()).unwrap();
        assert_eq!(fixture["schema"], "omen.compat.regression.v1");
        assert_eq!(fixture["failure_predicate"], "case.contains_bug_token");
        assert_eq!(fixture["input"][0], "BUG");
        assert!(report.attempts <= plan.max_attempts);
    }

    #[tokio::test]
    async fn shrinker_refuses_a_non_failing_initial_case() {
        let plan = ShrinkPlan::new(10, "always-fails").unwrap();
        assert_eq!(
            shrink_sequence::<u8, _, _>(&[1, 2], &plan, |_| async { false }).await,
            Err(ShrinkError::InitialInputDoesNotFail)
        );
    }

    #[tokio::test]
    async fn an_empty_counterexample_is_reported_minimal() {
        let plan = ShrinkPlan::new(20, "case.is_empty").unwrap();
        let report = shrink_sequence(&[1], &plan, |items| async move {
            items.is_empty() || items == [1]
        })
        .await
        .unwrap();
        assert!(report.minimized.is_empty());
        assert!(report.minimal);
    }
    #[tokio::test]
    async fn shrinker_respects_its_attempt_budget() {
        let plan = ShrinkPlan::new(2, "non-empty").unwrap();
        let input = [1, 2, 3, 4, 5, 6, 7, 8];
        let report = shrink_sequence(&input, &plan, |items| async move { !items.is_empty() })
            .await
            .unwrap();
        assert_eq!(report.attempts, 2);
        assert!(!report.minimal);
        assert!(!report.minimized.is_empty());
    }
}
