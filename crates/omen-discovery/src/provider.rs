//! The one discovery provider contract.
//!
//! Every source of candidates — commands, options, files, Omen actions,
//! subcommands, workspaces, values — is a `DiscoveryProvider` producing
//! [`DiscoveredCandidate`]. There is no per-source completion system.
//!
//! A provider declares its **capabilities** (kinds, cost tier, determinism,
//! authority classes it may produce, freshness). The concrete authority lives
//! on each candidate it emits; the registry never infers candidate authority
//! from provider identity.

use serde::{Deserialize, Serialize};

use crate::authority::AuthorityClass;
use crate::budget::{CostTier, DiscoveryBudget};
use crate::context::ProviderContext;
use crate::kind::Kind;
use crate::outcome::ProviderOutcome;

/// Stable provider identity. Used for cache keys, telemetry, provenance.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProviderId(pub String);

impl ProviderId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether a provider applies to a given context. Must be a cheap predicate
/// with no I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerDecision {
    /// The provider applies and should be dispatched.
    Apply,
    /// The provider does not apply to this context.
    Skip,
}

/// Whether a provider's output is deterministic or heuristic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Determinism {
    /// Same context + same underlying facts => same candidates.
    Deterministic,
    /// May vary (heuristic). Affects ranking trust and staleness handling.
    Heuristic,
}

/// How a provider's results may be cached and when they go stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FreshnessPolicy {
    /// Never cached; recomputed each request.
    Never,
    /// Cached for a fixed duration.
    Ttl { seconds: u64 },
    /// Cached but invalidated when an identity component changes
    /// (executable digest, directory mtime, env fingerprint).
    IdentityBound,
}

/// The one provider contract.
pub trait DiscoveryProvider: Send {
    /// Stable identity.
    fn id(&self) -> ProviderId;

    /// Which kinds this provider can produce. Used for dispatch and collision policy.
    fn supported_kinds(&self) -> &'static [Kind];

    /// Cheap applicability predicate. Must not perform I/O.
    fn triggers(&self, ctx: &ProviderContext) -> TriggerDecision;

    /// Cost tier. Enforced by the scheduler: tiers above
    /// [`CostTier::CheapLocal`] are dispatched asynchronously and never block
    /// the editor loop.
    fn cost_tier(&self) -> CostTier;

    /// Deterministic or heuristic.
    fn determinism(&self) -> Determinism;

    /// Authority classes this provider **may** produce. Not what it did
    /// produce — that is per candidate.
    fn authority_capabilities(&self) -> &'static [AuthorityClass];

    /// Freshness / cache policy.
    fn freshness(&self) -> FreshnessPolicy;

    /// Perform discovery under a bounded budget.
    ///
    /// Must observe `budget.deadline` and `budget.cancel`; exceeding either
    /// returns [`crate::outcome::ProviderOutcome::Partial`] or
    /// [`crate::outcome::ProviderOutcome::Failed`] with a phase-named error.
    fn discover(&mut self, ctx: &ProviderContext, budget: &DiscoveryBudget) -> ProviderOutcome;
}
