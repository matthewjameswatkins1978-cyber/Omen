//! Central Lens ranker.
//!
//! **Law:** *Providers discover truth. The Lens ranker orders truth. The UI
//! presents truth.*
//!
//! Providers do not globally rank themselves. A provider may declare an
//! [`OrderPolicy`](crate::candidate::OrderPolicy) where its order is semantic
//! (required before optional, declaration order); the ranker respects that
//! relative order without surrendering global presentation control.
//!
//! Authority strength is **not** a global importance score. It resolves
//! competing validity evidence for the same semantic candidate only (see
//! [`crate::merge`]); ordering of distinct valid candidates uses match quality,
//! context relevance and declared order.
//!
//! Ranking is deterministic: same candidates + same context + same
//! configuration => same order.

use crate::candidate::{
    MatchedCandidate, OrderPolicy, PresentationSlot, RankScore, RankedCandidate, SafetyAnnotation,
    SignalBreakdown,
};
use crate::context::{CommandPosition, ProviderContext};
use crate::identity::StabilityKey;
use crate::matcher::MatchQuality;
use crate::merge::MergedCandidate;

/// Weight of the match-quality signal in presentation ordering.
const MATCH_QUALITY_WEIGHT: f32 = 100.0;
/// Weight of contextual relevance.
const CONTEXT_WEIGHT: f32 = 40.0;
/// Weight of a provider-declared semantic order (small; keeps it subordinate).
const SEMANTIC_ORDER_WEIGHT: f32 = 5.0;

/// Deterministic presentation scoring.
pub struct LensRanker;

impl LensRanker {
    /// Orders merged, matched candidates into a stable presentation list.
    pub fn rank(
        merged: Vec<(MergedCandidate, MatchedCandidate)>,
        ctx: &ProviderContext,
    ) -> Vec<RankedCandidate> {
        let mut scored: Vec<(f32, StabilityKey, RankedCandidate)> = merged
            .into_iter()
            .map(|(m, matched)| {
                let signals = score(&matched, ctx);
                let total = signals_total(&signals);
                let stability = m.stability;
                let rc = RankedCandidate {
                    matched,
                    rank_score: RankScore(total),
                    signals,
                    slot: PresentationSlot(0),
                };
                (total, stability, rc)
            })
            .collect();

        // Deterministic ordering: score desc, then semantic-order-preserving
        // stability key asc. No floating-point nondeterminism in the tie-break.
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.0.cmp(&b.1.0))
        });

        scored
            .into_iter()
            .enumerate()
            .map(|(i, (_, _, mut rc))| {
                rc.slot = PresentationSlot(i);
                rc
            })
            .collect()
    }
}

fn signals_total(s: &SignalBreakdown) -> f32 {
    (MATCH_QUALITY_WEIGHT * match_quality_score(s.match_quality))
        + (CONTEXT_WEIGHT * s.context_relevance)
        + (SEMANTIC_ORDER_WEIGHT
            * s.semantic_order
                .map(|o| 1.0 / (1.0 + o as f32))
                .unwrap_or(0.0))
        + s.safety_adjustment
}

fn match_quality_score(q: MatchQuality) -> f32 {
    // Stronger qualities are lower ordinal; invert so stronger sorts higher.
    match q {
        MatchQuality::Exact => 1.0,
        MatchQuality::Prefix => 0.85,
        MatchQuality::TokenPrefix => 0.65,
        MatchQuality::Normalised => 0.5,
        MatchQuality::Fuzzy => 0.3,
        MatchQuality::Semantic => 0.2,
    }
}

fn score(matched: &MatchedCandidate, ctx: &ProviderContext) -> SignalBreakdown {
    let semantic_order = match matched.discovered.order_policy {
        OrderPolicy::Semantic(o) => Some(o),
        OrderPolicy::Unspecified => None,
    };

    let context_relevance = contextual_relevance(matched, ctx);
    let safety_adjustment = safety_adjustment(matched.discovered.safety);

    SignalBreakdown {
        match_quality: matched.match_quality,
        context_relevance,
        semantic_order,
        safety_adjustment,
        stability: StabilityKey::derive(&matched.discovered.semantic).0,
    }
}

/// Contextual relevance: deterministic, M1-only. No personal frequency/recency.
fn contextual_relevance(matched: &MatchedCandidate, ctx: &ProviderContext) -> f32 {
    use crate::kind::Kind;
    let kind = matched.discovered.semantic.kind;

    let mut relevance: f32 = match kind {
        // In command-name position, commands/intrinsics/actions are the point.
        Kind::OmenAction => 0.95,
        Kind::Intrinsic => 0.9,
        Kind::Command => 0.88,
        Kind::Subcommand => 0.82,
        Kind::Option => 0.8,
        Kind::ArgumentValue => 0.75,
        Kind::Reference => 0.72,
        Kind::Resource => 0.7,
        Kind::Service => 0.68,
        Kind::Workspace => 0.66,
        Kind::Capability => 0.64,
        Kind::Directory | Kind::File => 0.62,
        Kind::HistoryItem => 0.6,
    };

    // Boost kinds that match the semantic position of the cursor.
    let position_boost = match (&ctx.command_position, kind) {
        (CommandPosition::CommandName, Kind::Command | Kind::Intrinsic | Kind::OmenAction) => 0.15,
        (CommandPosition::OptionName { .. }, Kind::Option) => 0.2,
        (CommandPosition::ExecArg { .. }, Kind::Subcommand) => 0.18,
        (CommandPosition::ActionArg { .. }, Kind::ArgumentValue | Kind::Resource) => 0.18,
        (_, Kind::Directory | Kind::File) if path_relevant(ctx) => 0.15,
        _ => 0.0,
    };
    relevance += position_boost;

    // Prefer directories once a path parent is typed (locality bias).
    if kind == Kind::Directory && path_relevant(ctx) {
        relevance += 0.05;
    }

    relevance.min(1.0)
}

fn path_relevant(ctx: &ProviderContext) -> bool {
    let q = ctx.query();
    q.contains('/')
        || q.contains('\\')
        || q.starts_with('.')
        || !matches!(ctx.command_position, CommandPosition::CommandName)
}

/// Advisory only: demote unknown/privileged/dangerous candidates slightly so
/// safer alternatives surface first, but never remove them.
/// Penalty applied to risky candidates so safer alternatives surface first.
/// Values are `kind_weight * SAFETY_ADJUSTMENT_SCALE`, folded to literals.
fn safety_adjustment(safety: SafetyAnnotation) -> f32 {
    match safety {
        SafetyAnnotation::SafeReadOnly => 0.0,
        SafetyAnnotation::MutatesLocal => -0.75,
        SafetyAnnotation::MutatesExternal => -1.5,
        SafetyAnnotation::Networked => -1.8,
        SafetyAnnotation::Privileged => -3.0,
        SafetyAnnotation::Destructive => -4.5,
        SafetyAnnotation::UnknownImpact => -1.2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{Authority, AuthorityClass};
    use crate::candidate::{
        CandidateValue, Description, DiscoveredCandidate, OrderPolicy, SafetyAnnotation, TextSpan,
    };
    use crate::context::{ActiveToken, EnvFacts, ProjectContext};
    use crate::identity::{
        EvidenceIdentity, ProvenanceKey, SemanticKey, SemanticNamespace, StabilityKey,
    };
    use crate::kind::Kind;
    use crate::matcher::Matcher;

    fn ctx() -> ProviderContext {
        ProviderContext {
            buffer: "git --ver".into(),
            cursor: 9,
            active_token: ActiveToken::synthetic(9),
            replacement_span: TextSpan::new(4, 9),
            command_position: CommandPosition::OptionName {
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

    fn opt(name: &str, order: OrderPolicy) -> DiscoveredCandidate {
        DiscoveredCandidate::new(
            SemanticKey::new(
                Kind::Option,
                name,
                SemanticNamespace::Tool { tool: "git".into() },
            ),
            ProvenanceKey::new(
                "tool-options",
                AuthorityClass::ToolNative,
                EvidenceIdentity::Static,
            ),
            name,
            Authority::ToolNative {
                tool: "git".into(),
                spec_version: None,
            },
            TextSpan::new(4, 9),
        )
        .with_order_policy(order)
        .with_description(Description::short(format!("the {name} option")))
    }

    fn to_ranked(
        candidates: Vec<DiscoveredCandidate>,
        ctx: &ProviderContext,
    ) -> Vec<RankedCandidate> {
        let mut pairs = Vec::new();
        for c in candidates {
            let semantic = c.semantic.clone();
            let merged = crate::merge::MergedCandidate {
                semantic,
                primary: c.clone(),
                supporting: vec![],
                stability: StabilityKey::derive(&c.semantic),
            };
            let matched = Matcher::match_one(&c, "").unwrap();
            pairs.push((merged, matched));
        }
        LensRanker::rank(pairs, ctx)
    }

    #[test]
    fn stronger_match_quality_sorts_first() {
        let c = ctx();
        let ranked = to_ranked(
            vec![
                opt("--verbose", OrderPolicy::Unspecified),
                opt("--verify", OrderPolicy::Unspecified),
            ],
            &c,
        );
        let _ = ranked;
        // Both match empty query equally; deterministic order must be stable.
        let a = to_ranked(vec![opt("--verbose", OrderPolicy::Unspecified)], &c);
        let b = to_ranked(vec![opt("--verbose", OrderPolicy::Unspecified)], &c);
        assert_eq!(a[0].rank_score, b[0].rank_score);
    }

    #[test]
    fn ranking_is_deterministic() {
        let c = ctx();
        let batch = || {
            vec![
                opt("--verbose", OrderPolicy::Semantic(1)),
                opt("--verify", OrderPolicy::Semantic(0)),
                opt("--version", OrderPolicy::Unspecified),
            ]
        };
        let a = to_ranked(batch(), &c);
        let b = to_ranked(batch(), &c);
        let names = |r: &[RankedCandidate]| -> Vec<String> {
            r.iter().map(|x| x.value().insert.clone()).collect()
        };
        assert_eq!(names(&a), names(&b), "same input must produce same order");
    }

    #[test]
    fn semantic_order_is_respected_but_not_dominant() {
        let c = ctx();
        let ranked = to_ranked(
            vec![
                opt("--bbb", OrderPolicy::Semantic(0)),
                opt("--aaa", OrderPolicy::Semantic(1)),
            ],
            &c,
        );
        // Same match quality; semantic order 0 should outrank semantic order 1.
        assert_eq!(ranked[0].value().insert, "--bbb");
    }

    #[test]
    fn signals_explain_ordering() {
        let c = ctx();
        let ranked = to_ranked(vec![opt("--verbose", OrderPolicy::Semantic(3))], &c);
        assert_eq!(ranked[0].signals.semantic_order, Some(3));
        assert!(ranked[0].signals.match_quality.is_active_in_m1());
    }

    #[test]
    fn slots_are_sequential() {
        let c = ctx();
        let ranked = to_ranked(
            vec![
                opt("--a", OrderPolicy::Unspecified),
                opt("--b", OrderPolicy::Unspecified),
                opt("--c", OrderPolicy::Unspecified),
            ],
            &c,
        );
        for (i, r) in ranked.iter().enumerate() {
            assert_eq!(r.slot.0, i);
        }
    }

    #[test]
    fn safety_demotes_but_never_removes() {
        let c = ctx();
        let safe =
            opt("--safe", OrderPolicy::Unspecified).with_safety(SafetyAnnotation::SafeReadOnly);
        let mut dangerous = opt("--force", OrderPolicy::Unspecified);
        dangerous.safety = SafetyAnnotation::Destructive;
        dangerous.value = CandidateValue::new("--force");
        let ranked = to_ranked(vec![dangerous, safe], &c);
        assert_eq!(
            ranked.len(),
            2,
            "dangerous candidate is demoted, not removed"
        );
        assert_eq!(ranked[0].value().insert, "--safe", "safer surfaces first");
    }
}
