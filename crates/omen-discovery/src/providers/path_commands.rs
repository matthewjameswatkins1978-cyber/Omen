//! PATH command provider.
//!
//! **Authority:** [`Authority::Filesystem`]. TIER 1A: the provider reads a
//! shared bounded in-memory cache that is populated out-of-band. The keystroke
//! hot path never scans PATH; the scan itself is TIER 1B work owned by the
//! caller and refreshed on a TTL.
//!
//! This deliberately does **not** duplicate the existing command cache: the
//! interactive layer injects the same list it already maintains.

use std::sync::{Arc, Mutex};

use crate::authority::{Authority, AuthorityClass};
use crate::budget::{CostTier, DiscoveryBudget};
use crate::candidate::{
    CandidateValue, Description, DiscoveredCandidate, Display, SafetyAnnotation,
};
use crate::context::{CommandPosition, ProviderContext};
use crate::identity::{EvidenceIdentity, ProvenanceKey, SemanticKey, SemanticNamespace};
use crate::kind::Kind;
use crate::outcome::{DeclineReason, ProviderOutcome};
use crate::provider::{
    Determinism, DiscoveryProvider, FreshnessPolicy, ProviderId, TriggerDecision,
};

/// Shared bounded PATH command cache.
///
/// Owned by the interactive layer (which refreshes it out-of-band) and read by
/// this provider. Read is a cheap lock + clone of names matching the query.
#[derive(Debug, Clone, Default)]
pub struct PathCommandCache {
    inner: Arc<Mutex<Vec<String>>>,
    truncated: Arc<Mutex<bool>>,
}

impl PathCommandCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the cached names (out-of-band refresh).
    pub fn set(&self, names: Vec<String>, truncated: bool) {
        if let Ok(mut v) = self.inner.lock() {
            *v = names;
        }
        if let Ok(mut t) = self.truncated.lock() {
            *t = truncated;
        }
    }

    pub fn names(&self) -> Vec<String> {
        self.inner.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn truncated(&self) -> bool {
        self.truncated.lock().map(|t| *t).unwrap_or(false)
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|v| v.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Discovers real executable/command names from the shared bounded cache.
pub struct PathCommandsProvider {
    cache: PathCommandCache,
}

impl PathCommandsProvider {
    pub fn new(cache: PathCommandCache) -> Self {
        Self { cache }
    }
}

impl DiscoveryProvider for PathCommandsProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("path-commands")
    }

    fn supported_kinds(&self) -> &'static [Kind] {
        &[Kind::Command]
    }

    fn triggers(&self, ctx: &ProviderContext) -> TriggerDecision {
        // Command-name position only, and only for tokens that are not an
        // explicit path or a bare drive designator (those are filesystem).
        if !matches!(ctx.command_position, CommandPosition::CommandName) {
            return TriggerDecision::Skip;
        }
        let q = ctx.query();
        if q.contains('/') || q.contains('\\') || q.starts_with('.') {
            return TriggerDecision::Skip;
        }
        TriggerDecision::Apply
    }

    fn cost_tier(&self) -> CostTier {
        CostTier::CheapLocal
    }

    fn determinism(&self) -> Determinism {
        Determinism::Deterministic
    }

    fn authority_capabilities(&self) -> &'static [AuthorityClass] {
        &[AuthorityClass::Filesystem]
    }

    fn freshness(&self) -> FreshnessPolicy {
        FreshnessPolicy::Ttl { seconds: 60 }
    }

    fn discover(&mut self, ctx: &ProviderContext, budget: &DiscoveryBudget) -> ProviderOutcome {
        if self.cache.is_empty() {
            // Cache not yet populated out-of-band. This is not a failure and not
            // a fabricated answer; it is a clean decline.
            return ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch,
            };
        }
        let query = ctx.query().to_lowercase();
        let span = ctx.replacement_span;
        let mut candidates = Vec::new();
        for name in self.cache.names() {
            if candidates.len() >= budget.max_candidates {
                return ProviderOutcome::Partial {
                    candidates,
                    reason: crate::outcome::PartialReason::CandidateCap,
                };
            }
            if !query.is_empty() && !name.to_lowercase().starts_with(&query) {
                continue;
            }
            candidates.push(
                DiscoveredCandidate::new(
                    SemanticKey::new(Kind::Command, &name, SemanticNamespace::Global),
                    ProvenanceKey::new(
                        "path-commands",
                        AuthorityClass::Filesystem,
                        EvidenceIdentity::Filesystem {
                            root: "PATH".into(),
                            mtime_unix: None,
                        },
                    ),
                    name.clone(),
                    Authority::Filesystem {
                        root: "PATH".into(),
                        observed_at_unix: 0,
                    },
                    span,
                )
                .with_display(Display::new(&name))
                .with_description(Description::short("command on PATH"))
                .with_safety(SafetyAnnotation::UnknownImpact),
            );
        }
        if candidates.is_empty() {
            return ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch,
            };
        }
        let _ = CandidateValue::new("");
        ProviderOutcome::Answered { candidates }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::TextSpan;

    fn ctx(q: &str) -> ProviderContext {
        ProviderContext {
            buffer: q.to_string(),
            cursor: q.len(),
            active_token: crate::context::ActiveToken {
                span: TextSpan::new(0, q.len()),
                decoded_prefix: q.to_string(),
                decoded_suffix: String::new(),
                literal: q.to_string(),
                raw_prefix: q.to_string(),
                raw_suffix: String::new(),
                split_unsafe: false,
                quote_unclosed: false,
            },
            replacement_span: TextSpan::new(0, q.len()),
            command_position: CommandPosition::CommandName,
            known_executable: None,
            working_dir: ".".into(),
            project: Default::default(),
            env: Default::default(),
            depth: crate::budget::DiscoveryDepth::Normal,
        }
    }

    #[test]
    fn discovers_cached_commands() {
        let cache = PathCommandCache::new();
        cache.set(vec!["git".into(), "cargo".into()], false);
        let mut p = PathCommandsProvider::new(cache);
        let out = p.discover(&ctx("gi"), &DiscoveryBudget::inline_only());
        assert_eq!(out.candidates().len(), 1);
        assert_eq!(out.candidates()[0].value.insert, "git");
    }

    #[test]
    fn empty_cache_declines_without_fabrication() {
        let mut p = PathCommandsProvider::new(PathCommandCache::new());
        let out = p.discover(&ctx(""), &DiscoveryBudget::inline_only());
        assert!(matches!(
            out,
            ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch
            }
        ));
    }

    #[test]
    fn explicit_path_token_is_skipped() {
        let cache = PathCommandCache::new();
        cache.set(vec!["git".into()], false);
        let p = PathCommandsProvider::new(cache);
        assert_eq!(p.triggers(&ctx("./x")), TriggerDecision::Skip);
    }

    #[test]
    fn tier_is_cheap_local() {
        assert_eq!(
            PathCommandsProvider::new(PathCommandCache::new()).cost_tier(),
            CostTier::CheapLocal
        );
    }
}
