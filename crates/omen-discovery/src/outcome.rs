//! Provider outcomes.
//!
//! **Never conflate:** "nothing matched", "provider broke" and "provider was
//! cancelled" are three different truths. Partial success must remain
//! representable: eight providers succeeding and one failing is not a failure.

use serde::{Deserialize, Serialize};

use crate::candidate::DiscoveredCandidate;

/// Why a provider deliberately produced nothing. Not a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeclineReason {
    /// Valid sources exist; nothing matched the query.
    NoMatch,
    /// The context does not trigger this provider.
    NotApplicable,
    /// The provider would have needed a higher cost tier than permitted.
    BudgetExhausted,
    /// Cached truth is invalid and regeneration is not permitted.
    StaleUnavailable,
}

/// Why only partial results were returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartialReason {
    /// Per-provider candidate cap reached.
    CandidateCap,
    /// Output byte cap reached.
    ByteCap,
    /// Deadline elapsed mid-discovery.
    Timeout,
    /// Cancellation was requested mid-discovery.
    Cancelled,
}

/// A provider failure. Scoped to one provider; never poisons siblings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderError {
    /// Phase-named error, e.g. `provider.tool_options.harvest.timeout`.
    pub phase: String,
    pub message: String,
}

impl ProviderError {
    pub fn new(phase: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            phase: phase.into(),
            message: message.into(),
        }
    }
}

/// Result of one provider's discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderOutcome {
    /// Authoritative result.
    Answered {
        candidates: Vec<DiscoveredCandidate>,
    },
    /// Provider deliberately found nothing. A first-class valid result.
    Declined { reason: DeclineReason },
    /// Bounded partial result (cap/timeout/cancel).
    Partial {
        candidates: Vec<DiscoveredCandidate>,
        reason: PartialReason,
    },
    /// The provider failed. Isolated to this provider.
    Failed { error: ProviderError },
}

impl ProviderOutcome {
    /// Candidates carried by this outcome, if any.
    pub fn candidates(&self) -> &[DiscoveredCandidate] {
        match self {
            ProviderOutcome::Answered { candidates }
            | ProviderOutcome::Partial { candidates, .. } => candidates,
            ProviderOutcome::Declined { .. } | ProviderOutcome::Failed { .. } => &[],
        }
    }

    /// Whether this outcome represents provider failure (not mere absence).
    pub fn is_failure(&self) -> bool {
        matches!(self, ProviderOutcome::Failed { .. })
    }

    /// Whether the provider positively answered with at least one candidate.
    pub fn is_answered(&self) -> bool {
        match self {
            ProviderOutcome::Answered { candidates } => !candidates.is_empty(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{Authority, AuthorityClass};
    use crate::candidate::TextSpan;
    use crate::identity::{EvidenceIdentity, ProvenanceKey, SemanticKey, SemanticNamespace};
    use crate::kind::Kind;

    fn one() -> DiscoveredCandidate {
        DiscoveredCandidate::new(
            SemanticKey::new(Kind::Command, "git", SemanticNamespace::Global),
            ProvenanceKey::new("path", AuthorityClass::Filesystem, EvidenceIdentity::Static),
            "git",
            Authority::Filesystem {
                root: ".".into(),
                observed_at_unix: 0,
            },
            TextSpan::at(0),
        )
    }

    #[test]
    fn declined_is_not_failure() {
        let o = ProviderOutcome::Declined {
            reason: DeclineReason::NoMatch,
        };
        assert!(!o.is_failure());
        assert!(o.candidates().is_empty());
    }

    #[test]
    fn failed_is_failure() {
        let o = ProviderOutcome::Failed {
            error: ProviderError::new("provider.x.spawn", "boom"),
        };
        assert!(o.is_failure());
    }

    #[test]
    fn partial_carries_candidates_and_reason() {
        let o = ProviderOutcome::Partial {
            candidates: vec![one()],
            reason: PartialReason::CandidateCap,
        };
        assert_eq!(o.candidates().len(), 1);
        assert!(!o.is_failure());
    }

    #[test]
    fn answered_with_empty_vec_is_not_answered() {
        let o = ProviderOutcome::Answered { candidates: vec![] };
        assert!(!o.is_answered());
    }

    #[test]
    fn error_phase_is_named() {
        let e = ProviderError::new("provider.tool_options.harvest.timeout", "t");
        assert!(e.phase.contains('.'));
    }
}
