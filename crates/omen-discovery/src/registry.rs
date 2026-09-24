//! Provider registry and the discovery pipeline.
//!
//! The registry owns providers and runs the one coherent pipeline:
//!
//! ```text
//! providers -> DiscoveredCandidate[] -> merge (by SemanticKey)
//!           -> match (MatchQuality + highlight) -> rank (stable order)
//!           -> RankedCandidate[]
//! ```
//!
//! The registry **never** infers candidate authority from provider identity:
//! each [`DiscoveredCandidate`](crate::candidate::DiscoveredCandidate) carries
//! its own concrete authority. A failed provider never poisons siblings.

use std::time::Instant;

use crate::budget::{CostTier, DiscoveryBudget};
use crate::candidate::{MatchedCandidate, RankedCandidate};
use crate::context::ProviderContext;
use crate::matcher::Matcher;
use crate::merge::{MergedCandidate, merge_by_semantic};
use crate::outcome::{DeclineReason, PartialReason, ProviderError, ProviderOutcome};
use crate::provider::{DiscoveryProvider, ProviderId, TriggerDecision};
use crate::rank::LensRanker;
use crate::scheduler::Scheduler;
use crate::telemetry::{Events, Telemetry};

/// Aggregate result of a discovery request.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryResult {
    /// Ranked presentation candidates.
    pub ranked: Vec<RankedCandidate>,
    /// Providers that deliberately declined.
    pub declined: Vec<(ProviderId, DeclineReason)>,
    /// Providers that failed, isolated. Never poisons siblings.
    pub failed: Vec<(ProviderId, ProviderError)>,
    /// Providers that returned a bounded partial result.
    pub partial: Vec<(ProviderId, PartialReason)>,
    /// Providers that produced candidates.
    pub answered: Vec<ProviderId>,
}

impl DiscoveryResult {
    /// Whether any provider failed (partial success is still representable).
    pub fn has_failures(&self) -> bool {
        !self.failed.is_empty()
    }

    /// Clean decline: no candidates anywhere and no provider failures.
    pub fn is_clean_decline(&self) -> bool {
        self.ranked.is_empty() && self.failed.is_empty()
    }
}

/// The registry of discovery providers.
pub struct ProviderRegistry {
    providers: Vec<Box<dyn DiscoveryProvider>>,
    scheduler: Scheduler,
    telemetry: Telemetry,
    operation_id: u64,
}

impl ProviderRegistry {
    pub fn new(scheduler: Scheduler, telemetry: Telemetry) -> Self {
        Self {
            providers: Vec::new(),
            scheduler,
            telemetry,
            operation_id: 0,
        }
    }

    /// Registers a provider.
    pub fn register(&mut self, provider: Box<dyn DiscoveryProvider>) {
        self.providers.push(provider);
    }

    /// Providers currently registered, by id.
    pub fn provider_ids(&self) -> Vec<ProviderId> {
        self.providers.iter().map(|p| p.id()).collect()
    }

    pub fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }

    pub fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn next_operation(&mut self) -> u64 {
        self.operation_id = self.operation_id.wrapping_add(1);
        self.operation_id
    }

    /// Runs the full inline pipeline for tiers that may run inline.
    ///
    /// Providers whose cost tier exceeds [`CostTier::CheapLocal`] are **not**
    /// executed here; they are collected into `deferred` for the caller to
    /// dispatch asynchronously. This is the responsiveness guarantee: the
    /// editor thread never executes blocking local I/O or a subprocess.
    pub fn discover_inline(
        &mut self,
        ctx: &ProviderContext,
        budget: &DiscoveryBudget,
    ) -> (DiscoveryResult, Vec<ProviderId>) {
        let operation_id = self.next_operation();
        self.telemetry
            .record(Events::completion_requested(operation_id));

        let mut answered = Vec::new();
        let mut declined = Vec::new();
        let mut failed = Vec::new();
        let mut partial = Vec::new();
        let mut deferred = Vec::new();
        let mut batch = Vec::new();

        for provider in self.providers.iter_mut() {
            let pid = provider.id();
            if provider.triggers(ctx) == TriggerDecision::Skip {
                declined.push((pid, DeclineReason::NotApplicable));
                continue;
            }
            let tier = provider.cost_tier();
            if tier == CostTier::Network {
                // Tier 3 never runs on ordinary Tab in M1.
                declined.push((pid, DeclineReason::BudgetExhausted));
                continue;
            }
            if tier.must_be_async() {
                // Absolute responsiveness rule: never block the editor loop.
                // These are dispatched by the caller under a separate async
                // budget, not declined.
                deferred.push(pid);
                continue;
            }
            if !budget.permits(tier) {
                declined.push((pid, DeclineReason::BudgetExhausted));
                continue;
            }

            self.telemetry
                .record(Events::provider_started(operation_id, &pid, tier));
            let started = Instant::now();
            let outcome = provider.discover(ctx, budget);
            let latency = started.elapsed();

            match &outcome {
                ProviderOutcome::Answered { candidates } => {
                    self.telemetry.record(Events::provider_finished(
                        operation_id,
                        &pid,
                        tier,
                        candidates.len(),
                        latency,
                        "answered",
                    ));
                    answered.push(pid.clone());
                    batch.extend(candidates.iter().cloned());
                }
                ProviderOutcome::Declined { reason } => {
                    self.telemetry.record(Events::provider_finished(
                        operation_id,
                        &pid,
                        tier,
                        0,
                        latency,
                        "declined",
                    ));
                    declined.push((pid.clone(), *reason));
                }
                ProviderOutcome::Partial { candidates, reason } => {
                    self.telemetry.record(Events::provider_finished(
                        operation_id,
                        &pid,
                        tier,
                        candidates.len(),
                        latency,
                        "partial",
                    ));
                    partial.push((pid.clone(), *reason));
                    batch.extend(candidates.iter().cloned());
                }
                ProviderOutcome::Failed { error } => {
                    self.telemetry
                        .record(Events::provider_failure(operation_id, &pid));
                    failed.push((pid.clone(), error.clone()));
                }
            }
        }

        let ranked = run_pipeline(batch, ctx, &self.telemetry, operation_id);

        (
            DiscoveryResult {
                ranked,
                declined,
                failed,
                partial,
                answered,
            },
            deferred,
        )
    }

    /// Merges a background result (from the scheduler) into a ranked list.
    ///
    /// Used when a TIER 1B/2 provider finishes after the editor loop already
    /// returned an inline answer.
    pub fn rank_background(
        &self,
        batch: Vec<crate::candidate::DiscoveredCandidate>,
        ctx: &ProviderContext,
    ) -> Vec<RankedCandidate> {
        let operation_id = 0;
        run_pipeline(batch, ctx, &self.telemetry, operation_id)
    }
}

/// The one pipeline: merge -> match -> rank. Deterministic.
fn run_pipeline(
    batch: Vec<crate::candidate::DiscoveredCandidate>,
    ctx: &ProviderContext,
    telemetry: &Telemetry,
    operation_id: u64,
) -> Vec<RankedCandidate> {
    if batch.is_empty() {
        return Vec::new();
    }
    telemetry.record(Events::provider_finished(
        operation_id,
        &ProviderId::new("pipeline"),
        CostTier::MemoryOnly,
        batch.len(),
        std::time::Duration::ZERO,
        "answered",
    ));

    // 1. Semantic merge: group by SemanticKey, preserve all provenance.
    let merged: Vec<MergedCandidate> = merge_by_semantic(batch);

    // 2. Match: staged deterministic matching against the active query.
    let query = ctx.query();
    let mut pairs: Vec<(MergedCandidate, MatchedCandidate)> = Vec::new();
    for m in merged {
        let discovered = m.primary.clone();
        if let Some(matched) = Matcher::match_one(&discovered, query) {
            pairs.push((m, matched));
        }
    }

    // 3. Rank: central, deterministic, inspectable.
    LensRanker::rank(pairs, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{Authority, AuthorityClass};
    use crate::candidate::{DiscoveredCandidate, TextSpan};
    use crate::context::{ActiveToken, EnvFacts, ProjectContext};
    use crate::identity::{EvidenceIdentity, ProvenanceKey, SemanticKey, SemanticNamespace};
    use crate::kind::Kind;
    use crate::outcome::ProviderOutcome;
    use crate::provider::{Determinism, FreshnessPolicy};

    struct Fixed {
        id: &'static str,
        tier: CostTier,
        outcomes: Vec<ProviderOutcome>,
        classes: &'static [AuthorityClass],
    }

    impl DiscoveryProvider for Fixed {
        fn id(&self) -> ProviderId {
            ProviderId::new(self.id)
        }
        fn supported_kinds(&self) -> &'static [Kind] {
            &[Kind::Option]
        }
        fn triggers(&self, _ctx: &ProviderContext) -> TriggerDecision {
            TriggerDecision::Apply
        }
        fn cost_tier(&self) -> CostTier {
            self.tier
        }
        fn determinism(&self) -> Determinism {
            Determinism::Deterministic
        }
        fn authority_capabilities(&self) -> &'static [AuthorityClass] {
            self.classes
        }
        fn freshness(&self) -> FreshnessPolicy {
            FreshnessPolicy::Never
        }
        fn discover(&mut self, _ctx: &ProviderContext, _b: &DiscoveryBudget) -> ProviderOutcome {
            self.outcomes.pop().unwrap_or(ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch,
            })
        }
    }

    fn opt(v: &str, class: AuthorityClass, authority: Authority) -> DiscoveredCandidate {
        DiscoveredCandidate::new(
            SemanticKey::new(
                Kind::Option,
                v,
                SemanticNamespace::Tool { tool: "git".into() },
            ),
            ProvenanceKey::new("tool-options", class, EvidenceIdentity::Static),
            v,
            authority,
            TextSpan::new(4, 9),
        )
    }

    fn ctx() -> ProviderContext {
        ProviderContext {
            buffer: "git --ver".into(),
            cursor: 9,
            active_token: ActiveToken {
                span: TextSpan::new(4, 9),
                decoded_prefix: "--ver".into(),
                decoded_suffix: String::new(),
                literal: "--ver".into(),
                raw_prefix: "--ver".into(),
                raw_suffix: String::new(),
                split_unsafe: false,
                quote_unclosed: false,
            },
            replacement_span: TextSpan::new(4, 9),
            command_position: crate::context::CommandPosition::OptionName {
                command: "git".into(),
                chain: vec![],
            },
            known_executable: None,
            working_dir: ".".into(),
            project: ProjectContext::default(),
            env: EnvFacts::default(),
            depth: crate::budget::DiscoveryDepth::Normal,
        }
    }

    #[test]
    fn failed_provider_does_not_poison_siblings() {
        let mut reg = ProviderRegistry::new(Scheduler::spawn(), Telemetry::new(64));
        reg.register(Box::new(Fixed {
            id: "ok",
            tier: CostTier::MemoryOnly,
            outcomes: vec![ProviderOutcome::Answered {
                candidates: vec![opt(
                    "--verify",
                    AuthorityClass::ToolNative,
                    Authority::ToolNative {
                        tool: "git".into(),
                        spec_version: None,
                    },
                )],
            }],
            classes: &[AuthorityClass::ToolNative],
        }));
        reg.register(Box::new(Fixed {
            id: "broken",
            tier: CostTier::MemoryOnly,
            outcomes: vec![ProviderOutcome::Failed {
                error: ProviderError::new("discovery.provider.broken.spawn", "boom"),
            }],
            classes: &[AuthorityClass::ToolNative],
        }));

        let (result, deferred) = reg.discover_inline(&ctx(), &DiscoveryBudget::inline_only());
        assert!(deferred.is_empty());
        assert_eq!(result.ranked.len(), 1, "sibling still answered");
        assert!(result.has_failures(), "failure is recorded");
        assert!(!result.is_clean_decline());
    }

    #[test]
    fn async_tier_is_deferred_not_executed() {
        let mut reg = ProviderRegistry::new(Scheduler::spawn(), Telemetry::new(64));
        reg.register(Box::new(Fixed {
            id: "fs",
            tier: CostTier::BlockingLocal,
            outcomes: vec![],
            classes: &[AuthorityClass::Filesystem],
        }));
        let (result, deferred) = reg.discover_inline(&ctx(), &DiscoveryBudget::inline_only());
        assert_eq!(deferred, vec![ProviderId::new("fs")]);
        assert!(
            result.ranked.is_empty(),
            "no inline execution of blocking tier"
        );
    }

    #[test]
    fn same_semantic_merges_across_providers() {
        let mut reg = ProviderRegistry::new(Scheduler::spawn(), Telemetry::new(64));
        reg.register(Box::new(Fixed {
            id: "native",
            tier: CostTier::MemoryOnly,
            outcomes: vec![ProviderOutcome::Answered {
                candidates: vec![opt(
                    "--verify",
                    AuthorityClass::ToolNative,
                    Authority::ToolNative {
                        tool: "git".into(),
                        spec_version: None,
                    },
                )],
            }],
            classes: &[AuthorityClass::ToolNative],
        }));
        reg.register(Box::new(Fixed {
            id: "harvest",
            tier: CostTier::MemoryOnly,
            outcomes: vec![ProviderOutcome::Answered {
                candidates: vec![opt(
                    "--verify",
                    AuthorityClass::HelpHarvest,
                    Authority::HelpHarvest {
                        tool: "git".into(),
                        tool_version: None,
                        harvested_at_unix: 0,
                    },
                )],
            }],
            classes: &[AuthorityClass::HelpHarvest],
        }));

        let (result, _) = reg.discover_inline(&ctx(), &DiscoveryBudget::inline_only());
        assert_eq!(result.ranked.len(), 1, "merged into one semantic candidate");
    }

    #[test]
    fn clean_decline_is_first_class() {
        let mut reg = ProviderRegistry::new(Scheduler::spawn(), Telemetry::new(64));
        reg.register(Box::new(Fixed {
            id: "none",
            tier: CostTier::MemoryOnly,
            outcomes: vec![ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch,
            }],
            classes: &[AuthorityClass::ToolNative],
        }));
        let (result, _) = reg.discover_inline(&ctx(), &DiscoveryBudget::inline_only());
        assert!(result.is_clean_decline());
    }
}
