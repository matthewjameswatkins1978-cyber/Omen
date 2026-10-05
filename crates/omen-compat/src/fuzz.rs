use serde::Serialize;
use thiserror::Error;

pub const MAX_FUZZ_CASES: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzPlan {
    pub seed: u64,
    pub max_cases: usize,
    pub predicate_id: String,
}

impl FuzzPlan {
    pub fn new(
        seed: u64,
        max_cases: usize,
        predicate_id: impl Into<String>,
    ) -> Result<Self, FuzzPlanError> {
        let predicate_id = predicate_id.into();
        if max_cases == 0 || max_cases > MAX_FUZZ_CASES {
            return Err(FuzzPlanError::InvalidCaseBound);
        }
        if predicate_id.trim().is_empty() || predicate_id.len() > 128 {
            return Err(FuzzPlanError::InvalidPredicateId);
        }
        Ok(Self {
            seed,
            max_cases,
            predicate_id,
        })
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FuzzPlanError {
    #[error("fuzz case bound must be between 1 and {MAX_FUZZ_CASES}")]
    InvalidCaseBound,
    #[error("failure predicate id must be 1..=128 non-whitespace bytes")]
    InvalidPredicateId,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FuzzFinding<C> {
    pub schema: &'static str,
    pub seed: u64,
    pub case_index: usize,
    pub case_seed: u64,
    pub failure_predicate: String,
    pub input: C,
}

impl<C: Serialize> FuzzFinding<C> {
    /// Render a deterministic, self-contained regression record.
    /// Callers should supply fixture-safe structured inputs, never secrets.
    pub fn render_regression_fixture(&self) -> Result<Vec<u8>, serde_json::Error> {
        let canonical = serde_json::to_value(self)?;
        serde_json::to_vec_pretty(&canonical)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuzzReport<C> {
    pub seed: u64,
    pub predicate_id: String,
    pub cases_tested: usize,
    pub exhausted: bool,
    pub finding: Option<FuzzFinding<C>>,
}

/// Enumerate deterministic seeds and stop at the first predicate match.
/// FuzzPlan bounds the number of generated cases. Callers must keep each
/// generator and predicate operation bounded; process-backed predicates must
/// use the Compat runner.
pub fn run_structured_fuzz<C, Generate, Fails>(
    plan: &FuzzPlan,
    mut generate: Generate,
    mut fails: Fails,
) -> FuzzReport<C>
where
    C: Serialize,
    Generate: FnMut(u64) -> C,
    Fails: FnMut(&C) -> bool,
{
    let mut finding = None;
    let mut cases_tested = 0;

    for case_index in 0..plan.max_cases {
        let case_seed = derive_case_seed(plan.seed, case_index as u64);
        let input = generate(case_seed);
        cases_tested += 1;
        if fails(&input) {
            finding = Some(FuzzFinding {
                schema: "omen.compat.regression.v1",
                seed: plan.seed,
                case_index,
                case_seed,
                failure_predicate: plan.predicate_id.clone(),
                input,
            });
            break;
        }
    }

    FuzzReport {
        seed: plan.seed,
        predicate_id: plan.predicate_id.clone(),
        cases_tested,
        exhausted: finding.is_none(),
        finding,
    }
}

/// Stable SplitMix64 derivation; output is independent of platform RNG state.
pub fn derive_case_seed(seed: u64, index: u64) -> u64 {
    let mut value = seed.wrapping_add(index.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzz_seed_and_input_sequence_are_reproducible() {
        let plan = FuzzPlan::new(42, 12, "test.predicate").unwrap();
        let first = run_structured_fuzz(&plan, |seed| seed % 17, |_| false);
        let second = run_structured_fuzz(&plan, |seed| seed % 17, |_| false);
        assert_eq!(first, second);
        assert_eq!(first.cases_tested, 12);
        assert!(first.exhausted);
    }

    #[test]
    fn first_failure_is_preserved_as_a_regression_fixture() {
        let plan = FuzzPlan::new(7, 50, "fixture.contains_bug").unwrap();
        let report =
            run_structured_fuzz(&plan, |seed| seed, |seed| *seed == derive_case_seed(7, 4));
        let finding = report.finding.unwrap();
        assert_eq!(finding.case_index, 4);
        let json = String::from_utf8(finding.render_regression_fixture().unwrap()).unwrap();
        assert!(json.contains("omen.compat.regression.v1"));
        assert!(json.contains("fixture.contains_bug"));
    }

    #[test]
    fn fuzz_plan_rejects_unbounded_or_unnamed_campaigns() {
        assert_eq!(
            FuzzPlan::new(0, 0, "predicate"),
            Err(FuzzPlanError::InvalidCaseBound)
        );
        assert_eq!(
            FuzzPlan::new(0, MAX_FUZZ_CASES + 1, "predicate"),
            Err(FuzzPlanError::InvalidCaseBound)
        );
        assert_eq!(
            FuzzPlan::new(0, 1, " "),
            Err(FuzzPlanError::InvalidPredicateId)
        );
    }
}
