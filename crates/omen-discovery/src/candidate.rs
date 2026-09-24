//! Staged candidate types.
//!
//! Three stages, three owners. Illegal state is **unrepresentable**: a
//! [`DiscoveryProvider`](crate::provider::DiscoveryProvider) returns
//! [`DiscoveredCandidate`] and cannot construct ranking-owned fields because
//! those fields live in types the provider never sees.
//!
//! ```text
//! provider  ->  DiscoveredCandidate   (semantic truth only)
//! matcher   ->  MatchedCandidate      (+ match quality / highlight)
//! ranker    ->  RankedCandidate       (+ rank score / signals / slot)
//! ```
//!
//! **Law:** *Providers discover truth. The Lens ranker orders truth. The UI
//! presents truth.*

use serde::{Deserialize, Serialize};

use crate::authority::Authority;
use crate::identity::{ProvenanceKey, SemanticKey, StabilityKey};
use crate::kind::Kind;
use crate::matcher::MatchQuality;

/// A byte span in the request buffer. Deliberately independent of `reedline`
/// so this substrate carries no UI dependency; the projection converts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TextSpan {
    pub start: usize,
    pub end: usize,
}

impl TextSpan {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// A zero-width span at `pos`, i.e. pure insertion.
    pub fn at(pos: usize) -> Self {
        Self {
            start: pos,
            end: pos,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// What replaces the span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateValue {
    /// Canonical text placed at the replacement span.
    pub insert: String,
    /// Whether the editor should append a space after insertion.
    pub append_whitespace: bool,
}

impl CandidateValue {
    pub fn new(insert: impl Into<String>) -> Self {
        Self {
            insert: insert.into(),
            append_whitespace: false,
        }
    }

    pub fn with_trailing_space(mut self) -> Self {
        self.append_whitespace = true;
        self
    }
}

/// Presentation-only display information. Never validity bearing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    /// Label shown in the chooser. May differ from insert text (unquoted view).
    pub label: String,
}

impl Display {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

/// Human-facing description.
///
/// Descriptions are structured short/detail pairs, never prose soup. On
/// semantic merge they are combined only under explicit deterministic rules
/// (see [`crate::merge`]); arbitrary concatenation is forbidden.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Description {
    pub short: String,
    pub detail: Option<String>,
}

impl Description {
    pub fn short(short: impl Into<String>) -> Self {
        Self {
            short: short.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// The provider's claim about its own ordering.
///
/// A provider may assert that its order is *semantic* (required flags before
/// optional, declaration order) — the ranker preserves that relative order. It
/// may also declare [`OrderPolicy::Unspecified`], leaving ordering entirely to
/// the ranker. This is the only ordering input a provider may give.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderPolicy {
    /// Provider asserts its relative order is semantic and must be preserved.
    Semantic(u32),
    /// No order claim.
    Unspecified,
}

/// Advisory safety metadata. Never conflated with validity.
///
/// Lens is advisory only; it is never an execution authority. M1 carries this
/// field so later stages can present it; it does not solve the safety UI.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub enum SafetyAnnotation {
    /// Read-only / no side effects.
    SafeReadOnly,
    /// Mutates local state.
    MutatesLocal,
    /// Mutates external state.
    MutatesExternal,
    /// Performs network IO.
    Networked,
    /// Requires elevated privilege.
    Privileged,
    /// Destructive / irreversible.
    Destructive,
    /// Impact not established. This is the conservative default.
    #[default]
    UnknownImpact,
}

/// Opaque seam for a future explanation (M2). Carries no authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplanationRef(pub String);

// ---------------------------------------------------------------------------
// STAGE 1 — provider output
// ---------------------------------------------------------------------------

/// Provider-produced semantic truth.
///
/// A `DiscoveryProvider` returns only this type. It contains **no**
/// `match_quality`, **no** `match_indices`, **no** `rank_score` and **no**
/// presentation slot — those belong to the matcher and ranker and live in
/// types the provider cannot construct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredCandidate {
    /// WHAT this discoverable thing is. Drives semantic merge.
    pub semantic: SemanticKey,
    /// WHERE the observation came from. Preserved on merge.
    pub provenance: ProvenanceKey,
    /// What replaces the span.
    pub value: CandidateValue,
    /// Optional presentation label.
    pub display: Option<Display>,
    /// Optional structured description.
    pub description: Option<Description>,
    /// Concrete validity evidence for this candidate.
    pub authority: Authority,
    /// Byte range the candidate replaces in the request buffer.
    pub replacement_span: TextSpan,
    /// The provider's ordering claim.
    pub order_policy: OrderPolicy,
    /// Advisory safety metadata.
    pub safety: SafetyAnnotation,
    /// Future explanation seam.
    pub explanation: Option<ExplanationRef>,
}

impl DiscoveredCandidate {
    /// Convenience constructor for a fully-specified discovered candidate.
    pub fn new(
        semantic: SemanticKey,
        provenance: ProvenanceKey,
        value: impl Into<String>,
        authority: Authority,
        replacement_span: TextSpan,
    ) -> Self {
        Self {
            semantic,
            provenance,
            value: CandidateValue::new(value),
            display: None,
            description: None,
            authority,
            replacement_span,
            order_policy: OrderPolicy::Unspecified,
            safety: SafetyAnnotation::UnknownImpact,
            explanation: None,
        }
    }

    pub fn with_display(mut self, display: Display) -> Self {
        self.display = Some(display);
        self
    }

    pub fn with_description(mut self, description: Description) -> Self {
        self.description = Some(description);
        self
    }

    pub fn with_order_policy(mut self, policy: OrderPolicy) -> Self {
        self.order_policy = policy;
        self
    }

    pub fn with_safety(mut self, safety: SafetyAnnotation) -> Self {
        self.safety = safety;
        self
    }

    pub fn with_trailing_space(mut self) -> Self {
        self.value.append_whitespace = true;
        self
    }

    pub fn kind(&self) -> Kind {
        self.semantic.kind
    }
}

// ---------------------------------------------------------------------------
// STAGE 2 — matcher output
// ---------------------------------------------------------------------------

/// Matcher-owned state added to a discovered candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchedCandidate {
    pub discovered: DiscoveredCandidate,
    /// How closely the candidate matches the active query.
    pub match_quality: MatchQuality,
    /// Grapheme indices of the matched characters, for highlighting only.
    pub match_indices: Vec<usize>,
}

impl MatchedCandidate {
    pub fn new(
        discovered: DiscoveredCandidate,
        match_quality: MatchQuality,
        match_indices: Vec<usize>,
    ) -> Self {
        Self {
            discovered,
            match_quality,
            match_indices,
        }
    }
}

// ---------------------------------------------------------------------------
// STAGE 3 — ranker output
// ---------------------------------------------------------------------------

/// Why a candidate received its rank. Inspectable, never a mystery float.
///
/// Exists so tests and Bench can answer: *why was candidate A above B?*
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalBreakdown {
    pub match_quality: MatchQuality,
    pub context_relevance: f32,
    pub semantic_order: Option<u32>,
    pub safety_adjustment: f32,
    /// Deterministic tie-break contribution.
    pub stability: u64,
}

/// Composite presentation score. Deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RankScore(pub f32);

/// Stable presentation position assigned after merge and ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PresentationSlot(pub usize);

/// Ranker-owned presentation state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankedCandidate {
    pub matched: MatchedCandidate,
    pub rank_score: RankScore,
    pub signals: SignalBreakdown,
    pub slot: PresentationSlot,
}

impl RankedCandidate {
    pub fn semantic(&self) -> &SemanticKey {
        &self.matched.discovered.semantic
    }

    pub fn authority(&self) -> &Authority {
        &self.matched.discovered.authority
    }

    pub fn value(&self) -> &CandidateValue {
        &self.matched.discovered.value
    }

    pub fn span(&self) -> TextSpan {
        self.matched.discovered.replacement_span
    }

    pub fn stability(&self) -> StabilityKey {
        StabilityKey(self.signals.stability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{EvidenceIdentity, SemanticNamespace};
    use crate::kind::Kind;

    fn discovered() -> DiscoveredCandidate {
        DiscoveredCandidate::new(
            SemanticKey::new(
                Kind::Option,
                "--verbose",
                SemanticNamespace::Tool { tool: "git".into() },
            ),
            ProvenanceKey::new(
                "tool-options",
                crate::authority::AuthorityClass::HelpHarvest,
                EvidenceIdentity::Static,
            ),
            "--verbose",
            Authority::HelpHarvest {
                tool: "git".into(),
                tool_version: None,
                harvested_at_unix: 0,
            },
            TextSpan::new(4, 6),
        )
    }

    #[test]
    fn provider_output_has_no_ranking_fields() {
        // The type simply does not expose them; this compiles as proof.
        let d = discovered();
        let _ = (
            d.semantic,
            d.provenance,
            d.value,
            d.authority,
            d.replacement_span,
        );
    }

    #[test]
    fn stages_nest_without_loss() {
        let d = discovered();
        let span = d.replacement_span;
        let m = MatchedCandidate::new(d, MatchQuality::Prefix, vec![0, 1]);
        let r = RankedCandidate {
            matched: m,
            rank_score: RankScore(1.0),
            signals: SignalBreakdown {
                match_quality: MatchQuality::Prefix,
                context_relevance: 1.0,
                semantic_order: None,
                safety_adjustment: 0.0,
                stability: 0,
            },
            slot: PresentationSlot(0),
        };
        assert_eq!(r.span(), span);
        assert_eq!(r.matched.match_quality, MatchQuality::Prefix);
    }

    #[test]
    fn text_span_at_is_zero_width() {
        assert!(TextSpan::at(7).is_empty());
    }
}
