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
//!
//! ## Provider-faithful deferred dispatch
//!
//! TIER 1B/2 work is not re-labelled and re-created downstream. The registry
//! hands the caller a [`DeferredWork`] that *is* the real registered provider:
//! its identity, authority capability, cost tier and output stay bound
//! together. If `help-harvest` is deferred, the work that runs is
//! `help-harvest` — never a substitute provider.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use crate::budget::{CostTier, DiscoveryBudget};
use crate::candidate::{DiscoveredCandidate, MatchedCandidate, RankedCandidate};
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
    /// The **raw** discovered batch (pre-merge). Preserved so late async
    /// results re-merge against the same observations instead of against a
    /// merged-away projection — otherwise supporting provenance would be lost
    /// the moment a deferred provider finishes.
    pub batch: Vec<DiscoveredCandidate>,
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

/// One provider slot: identity and tier cached at registration, the provider
/// itself behind shared ownership so deferred work can carry the *real*
/// implementation to the worker thread.
struct Registered {
    id: ProviderId,
    tier: CostTier,
    provider: Arc<Mutex<Box<dyn DiscoveryProvider>>>,
}

fn lock_provider(
    p: &Arc<Mutex<Box<dyn DiscoveryProvider>>>,
) -> MutexGuard<'_, Box<dyn DiscoveryProvider>> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

/// TIER 1B/2 work handed to the caller for asynchronous dispatch.
///
/// The closure captures the registered provider itself (shared ownership) and
/// the request context; executing it is executing the real provider.
pub struct DeferredWork {
    /// The provider this work belongs to. Not a label — the same identity the
    /// executed provider reports.
    pub provider: ProviderId,
    /// The provider's declared cost tier, carried unchanged.
    pub tier: CostTier,
    /// The discovery request that produced this work.
    pub operation_id: u64,
    run: Box<dyn FnOnce(&DiscoveryBudget) -> ProviderOutcome + Send>,
}

/// The registry of discovery providers.
pub struct ProviderRegistry {
    providers: Vec<Registered>,
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

    /// Registers a provider. Its identity and cost tier are cached.
    pub fn register(&mut self, provider: Box<dyn DiscoveryProvider>) {
        let id = provider.id();
        let tier = provider.cost_tier();
        self.providers.push(Registered {
            id,
            tier,
            provider: Arc::new(Mutex::new(provider)),
        });
    }

    /// Providers currently registered, by id.
    pub fn provider_ids(&self) -> Vec<ProviderId> {
        self.providers.iter().map(|p| p.id.clone()).collect()
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

    /// Cheap applicability question: would **any** runnable async provider
    /// (TIER 1B/2; never Network) apply to this context?
    ///
    /// Used by the interactive layer to tell "discovery is not final yet"
    /// apart from "discovery proved zero" without dispatching anything.
    pub fn any_async_triggered(&self, ctx: &ProviderContext) -> bool {
        self.providers.iter().any(|r| {
            r.tier != CostTier::Network
                && r.tier.must_be_async()
                && lock_provider(&r.provider).triggers(ctx) == TriggerDecision::Apply
        })
    }

    /// Runs the full inline pipeline for tiers that may run inline.
    ///
    /// Providers whose cost tier must run async are **not** executed here;
    /// they are returned as [`DeferredWork`] carrying the real provider for
    /// the caller to dispatch through [`ProviderRegistry::dispatch`]. This is
    /// the responsiveness guarantee: the editor thread never executes blocking
    /// local I/O or a subprocess.
    pub fn discover_inline(
        &mut self,
        ctx: &ProviderContext,
        budget: &DiscoveryBudget,
    ) -> (DiscoveryResult, Vec<DeferredWork>) {
        let operation_id = self.next_operation();
        self.telemetry
            .record(Events::completion_requested(operation_id));

        let mut answered = Vec::new();
        let mut declined = Vec::new();
        let mut failed = Vec::new();
        let mut partial = Vec::new();
        let mut deferred = Vec::new();
        let mut batch: Vec<DiscoveredCandidate> = Vec::new();

        for reg in &self.providers {
            let pid = reg.id.clone();
            let tier = reg.tier;

            let applies = lock_provider(&reg.provider).triggers(ctx) == TriggerDecision::Apply;
            if !applies {
                declined.push((pid, DeclineReason::NotApplicable));
                continue;
            }
            if tier == CostTier::Network {
                // Tier 3 never runs on ordinary Tab in M1.
                declined.push((pid, DeclineReason::BudgetExhausted));
                continue;
            }
            if tier.must_be_async() {
                // Absolute responsiveness rule: hand back the real provider
                // for asynchronous execution. Never substitute, never run.
                let arc = reg.provider.clone();
                let ctx2 = ctx.clone();
                deferred.push(DeferredWork {
                    provider: pid,
                    tier,
                    operation_id,
                    run: Box::new(move |budget: &DiscoveryBudget| {
                        lock_provider(&arc).discover(&ctx2, budget)
                    }),
                });
                continue;
            }
            if !budget.permits(tier) {
                declined.push((pid, DeclineReason::BudgetExhausted));
                continue;
            }

            self.telemetry
                .record(Events::provider_started(operation_id, &pid, tier));
            let started = Instant::now();
            let outcome = lock_provider(&reg.provider).discover(ctx, budget);
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

        let ranked = run_pipeline(batch.clone(), ctx, &self.telemetry, operation_id);

        (
            DiscoveryResult {
                ranked,
                declined,
                failed,
                partial,
                answered,
                batch,
            },
            deferred,
        )
    }

    /// Dispatches deferred work to the async worker under the provider's own
    /// identity and cost tier.
    ///
    /// Returns `false` when the scheduler refuses the tier (never for TIER
    /// 1B/2 work produced by [`discover_inline`](Self::discover_inline)).
    pub fn dispatch(&self, work: DeferredWork, budget: DiscoveryBudget) -> bool {
        self.telemetry.record(Events::provider_started(
            work.operation_id,
            &work.provider,
            work.tier,
        ));
        self.scheduler.dispatch(
            work.provider,
            work.tier,
            work.operation_id,
            budget,
            work.run,
        )
    }

    /// The one pipeline (merge -> match -> rank) over an arbitrary batch —
    /// used to fold late async results into an already-ranked request.
    pub fn rank_batch(
        &self,
        batch: Vec<crate::candidate::DiscoveredCandidate>,
        ctx: &ProviderContext,
    ) -> Vec<RankedCandidate> {
        run_pipeline(batch, ctx, &self.telemetry, 0)
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
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Fixed {
        id: &'static str,
        tier: CostTier,
        outcomes: Vec<ProviderOutcome>,
        classes: &'static [AuthorityClass],
        ran: Option<Arc<AtomicBool>>,
    }

    impl Fixed {
        fn new(id: &'static str, tier: CostTier, outcomes: Vec<ProviderOutcome>) -> Self {
            Self {
                id,
                tier,
                outcomes,
                classes: &[AuthorityClass::ToolNative],
                ran: None,
            }
        }
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
            if let Some(flag) = &self.ran {
                flag.store(true, Ordering::SeqCst);
            }
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

    fn registry() -> ProviderRegistry {
        ProviderRegistry::new(Scheduler::spawn(), Telemetry::new(64))
    }

    fn await_work(reg: &ProviderRegistry) -> crate::scheduler::WorkResult {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(r) = reg.scheduler().take_result() {
                return r;
            }
            assert!(
                Instant::now() < deadline,
                "dispatched provider work must complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn failed_provider_does_not_poison_siblings() {
        let mut reg = registry();
        reg.register(Box::new(Fixed::new(
            "ok",
            CostTier::MemoryOnly,
            vec![ProviderOutcome::Answered {
                candidates: vec![opt(
                    "--verify",
                    AuthorityClass::ToolNative,
                    Authority::ToolNative {
                        tool: "git".into(),
                        spec_version: None,
                    },
                )],
            }],
        )));
        let mut broken = Fixed::new(
            "broken",
            CostTier::MemoryOnly,
            vec![ProviderOutcome::Failed {
                error: ProviderError::new("discovery.provider.broken.spawn", "boom"),
            }],
        );
        broken.classes = &[AuthorityClass::ToolNative];
        reg.register(Box::new(broken));

        let (result, deferred) = reg.discover_inline(&ctx(), &DiscoveryBudget::inline_only());
        assert!(deferred.is_empty());
        assert_eq!(result.ranked.len(), 1, "sibling still answered");
        assert!(result.has_failures(), "failure is recorded");
        assert!(!result.is_clean_decline());
    }

    #[test]
    fn async_tier_is_deferred_not_executed() {
        let mut reg = registry();
        let ran = Arc::new(AtomicBool::new(false));
        let mut fs = Fixed::new("fs", CostTier::BlockingLocal, vec![]);
        fs.classes = &[AuthorityClass::Filesystem];
        fs.ran = Some(ran.clone());
        reg.register(Box::new(fs));

        let (result, deferred) = reg.discover_inline(&ctx(), &DiscoveryBudget::inline_only());
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].provider, ProviderId::new("fs"));
        assert_eq!(deferred[0].tier, CostTier::BlockingLocal);
        assert!(
            result.ranked.is_empty(),
            "no inline execution of blocking tier"
        );
        assert!(
            !ran.load(Ordering::SeqCst),
            "the deferred provider has not run yet"
        );
    }

    #[test]
    fn deferred_work_executes_the_real_registered_provider() {
        // Defect-2 invariant: IF help-harvest is deferred, THE WORK THAT RUNS
        // MUST BE help-harvest — same identity, same tier, same implementation.
        let mut reg = registry();
        let ran = Arc::new(AtomicBool::new(false));
        let mut slow = Fixed::new(
            "help-harvest",
            CostTier::Subprocess,
            vec![ProviderOutcome::Answered {
                candidates: vec![opt(
                    "--harvested",
                    AuthorityClass::HelpHarvest,
                    Authority::HelpHarvest {
                        tool: "git".into(),
                        tool_version: None,
                        harvested_at_unix: 0,
                    },
                )],
            }],
        );
        slow.classes = &[AuthorityClass::HelpHarvest];
        slow.ran = Some(ran.clone());
        reg.register(Box::new(slow));

        let (result, mut deferred) =
            reg.discover_inline(&ctx(), &DiscoveryBudget::allowing_subprocess());
        assert!(result.ranked.is_empty());
        assert_eq!(deferred.len(), 1);
        let work = deferred.pop().unwrap();
        assert_eq!(work.provider, ProviderId::new("help-harvest"));
        assert_eq!(work.tier, CostTier::Subprocess, "real tier survives");

        assert!(reg.dispatch(work, DiscoveryBudget::allowing_subprocess()));
        let wr = await_work(&reg);
        assert_eq!(wr.provider, ProviderId::new("help-harvest"));
        assert_eq!(wr.tier, CostTier::Subprocess, "tier not re-labelled");
        assert!(
            ran.load(Ordering::SeqCst),
            "the REAL registered provider executed"
        );
        assert_eq!(wr.outcome.candidates().len(), 1);
        assert!(matches!(
            wr.outcome.candidates()[0].authority,
            Authority::HelpHarvest { .. }
        ));
        // Provider identity, authority capability and output stayed bound.
        assert!(wr.outcome.is_answered());
    }

    #[test]
    fn same_semantic_merges_across_providers() {
        let mut reg = registry();
        reg.register(Box::new(Fixed::new(
            "native",
            CostTier::MemoryOnly,
            vec![ProviderOutcome::Answered {
                candidates: vec![opt(
                    "--verify",
                    AuthorityClass::ToolNative,
                    Authority::ToolNative {
                        tool: "git".into(),
                        spec_version: None,
                    },
                )],
            }],
        )));
        reg.register(Box::new(Fixed::new(
            "harvest",
            CostTier::MemoryOnly,
            vec![ProviderOutcome::Answered {
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
        )));

        let (result, _) = reg.discover_inline(&ctx(), &DiscoveryBudget::inline_only());
        assert_eq!(result.ranked.len(), 1, "merged into one semantic candidate");
        assert!(
            result.ranked[0].has_supporting_evidence(),
            "both observations remain inspectable after ranking"
        );
        assert_eq!(
            result.batch.len(),
            2,
            "raw batch preserves merged-away observations for async re-merge"
        );
    }

    #[test]
    fn any_async_triggered_distinguishes_pending_from_final_zero() {
        let mut reg = registry();
        let mut fs = Fixed::new("fs", CostTier::BlockingLocal, vec![]);
        fs.classes = &[AuthorityClass::Filesystem];
        reg.register(Box::new(fs));

        assert!(
            reg.any_async_triggered(&ctx()),
            "async provider applies => discovery not final"
        );

        // A network-only registration must never mark Tab pending: nothing
        // would ever arrive.
        let mut reg2 = registry();
        reg2.register(Box::new(Fixed::new("net", CostTier::Network, vec![])));
        assert!(!reg2.any_async_triggered(&ctx()));
    }

    #[test]
    fn clean_decline_is_first_class() {
        let mut reg = registry();
        reg.register(Box::new(Fixed::new(
            "none",
            CostTier::MemoryOnly,
            vec![ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch,
            }],
        )));
        let (result, _) = reg.discover_inline(&ctx(), &DiscoveryBudget::inline_only());
        assert!(result.is_clean_decline());
        assert!(result.batch.is_empty());
    }
}
