//! Omen Lens M1 discovery substrate.
//!
//! **Foundational law:**
//!
//! ```text
//! PROVIDERS DISCOVER TRUTH.
//! THE LENS RANKER ORDERS TRUTH.
//! THE UI PRESENTS TRUTH.
//! ```
//!
//! This crate is *semantic infrastructure*. It has **no** dependency on
//! Reedline, terminal UI, ConPTY, human rendering, network/model providers or
//! Tethers authority execution. Dependency direction is:
//!
//! ```text
//! omen-interactive  ->  omen-discovery  ->  existing Omen semantic truth
//! ```
//!
//! Machine Lens will eventually consume this crate directly; the human UI and
//! the machine view are projections of the same semantic system.
//!
//! ## Staged candidate model
//!
//! Illegal state is unrepresentable. A provider returns
//! [`candidate::DiscoveredCandidate`] and cannot construct ranking-owned
//! fields because those live in types the provider never sees:
//!
//! ```text
//! provider  ->  DiscoveredCandidate   (semantic truth only)
//! matcher   ->  MatchedCandidate      (+ match quality / highlight)
//! ranker    ->  RankedCandidate       (+ rank score / signals / slot)
//! ```

pub mod authority;
pub mod budget;
pub mod cache;
pub mod candidate;
pub mod config;
pub mod context;
pub mod error;
pub mod identity;
pub mod kind;
pub mod matcher;
pub mod merge;
pub mod outcome;
pub mod provider;
pub mod providers;
pub mod rank;
pub mod registry;
pub mod scheduler;
pub mod telemetry;

pub use authority::{Authority, AuthorityClass, validity_evidence_strength};
pub use budget::{CancellationToken, CostTier, DiscoveryBudget, DiscoveryDepth};
pub use cache::{CacheIdentity, CacheKey, DiscoveryCache, Freshness};
pub use candidate::{
    CandidateValue, Description, DiscoveredCandidate, Display, ExplanationRef, MatchedCandidate,
    OrderPolicy, PresentationSlot, RankScore, RankedCandidate, SafetyAnnotation, SignalBreakdown,
    TextSpan,
};
pub use config::LensConfig;
pub use context::{
    ActiveToken, CommandPosition, EnvFacts, ExecutableIdentity, ProjectContext, ProviderContext,
};
pub use error::DiscoveryError;
pub use identity::{
    CanonicalValue, EvidenceIdentity, ProvenanceKey, SemanticKey, SemanticNamespace, StabilityKey,
};
pub use kind::Kind;
pub use matcher::{MatchQuality, Matcher};
pub use merge::{MergedCandidate, SupportingEvidence, merge_by_semantic};
pub use outcome::{DeclineReason, PartialReason, ProviderError, ProviderOutcome};
pub use provider::{Determinism, DiscoveryProvider, FreshnessPolicy, ProviderId, TriggerDecision};
pub use rank::LensRanker;
pub use registry::{DiscoveryResult, ProviderRegistry};
pub use scheduler::{Scheduler, WorkResult, WorkStatus};
pub use telemetry::{Phase, Telemetry, TelemetryEvent};

/// Crate version identity.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
