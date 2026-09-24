//! Typed-reference provider — the authoritative `@handle` vocabulary.
//!
//! **Authority:** [`Authority::Static`] (Omen's grammar authority; these
//! handles parse under `grammar::TypedReference`). TIER 0.
//!
//! Invented URI-style handles (`@symbol://...`) are **never** emitted: only
//! the injected static handles, plus the per-action handles declared by the
//! owning layer.

use std::sync::Arc;

use crate::authority::{Authority, AuthorityClass};
use crate::budget::{CostTier, DiscoveryBudget};
use crate::candidate::{Description, DiscoveredCandidate, Display, OrderPolicy, SafetyAnnotation};
use crate::context::{CommandPosition, ProviderContext};
use crate::identity::{EvidenceIdentity, ProvenanceKey, SemanticKey, SemanticNamespace};
use crate::kind::Kind;
use crate::outcome::{DeclineReason, ProviderOutcome};
use crate::provider::{
    Determinism, DiscoveryProvider, FreshnessPolicy, ProviderId, TriggerDecision,
};
use crate::providers::knowledge::OmenKnowledge;

/// Discovers authoritative typed-reference handles.
pub struct ReferenceProvider {
    knowledge: Arc<OmenKnowledge>,
}

impl ReferenceProvider {
    pub fn new(knowledge: Arc<OmenKnowledge>) -> Self {
        Self { knowledge }
    }

    fn handles_for(&self, ctx: &ProviderContext) -> &'static [&'static str] {
        match &ctx.command_position {
            CommandPosition::CommandName => {
                if ctx.query().starts_with('@') {
                    self.knowledge.typed_handles
                } else {
                    &[]
                }
            }
            CommandPosition::ActionArg { action } => {
                (self.knowledge.action_reference_handles)(action)
            }
            _ => &[],
        }
    }
}

impl DiscoveryProvider for ReferenceProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("references")
    }

    fn supported_kinds(&self) -> &'static [Kind] {
        &[Kind::Reference]
    }

    fn triggers(&self, ctx: &ProviderContext) -> TriggerDecision {
        if !self.handles_for(ctx).is_empty() {
            TriggerDecision::Apply
        } else {
            TriggerDecision::Skip
        }
    }

    fn cost_tier(&self) -> CostTier {
        CostTier::MemoryOnly
    }

    fn determinism(&self) -> Determinism {
        Determinism::Deterministic
    }

    fn authority_capabilities(&self) -> &'static [AuthorityClass] {
        &[AuthorityClass::Static]
    }

    fn freshness(&self) -> FreshnessPolicy {
        FreshnessPolicy::Never
    }

    fn discover(&mut self, ctx: &ProviderContext, _budget: &DiscoveryBudget) -> ProviderOutcome {
        let handles = self.handles_for(ctx);
        if handles.is_empty() {
            return ProviderOutcome::Declined {
                reason: DeclineReason::NotApplicable,
            };
        }
        let span = ctx.replacement_span;
        let mut candidates = Vec::new();
        for (i, h) in handles.iter().enumerate() {
            candidates.push(
                DiscoveredCandidate::new(
                    SemanticKey::new(Kind::Reference, *h, SemanticNamespace::Global),
                    ProvenanceKey::new(
                        "references",
                        AuthorityClass::Static,
                        EvidenceIdentity::Static,
                    ),
                    *h,
                    Authority::Static,
                    span,
                )
                .with_display(Display::new(*h))
                .with_description(Description::short("typed reference"))
                .with_order_policy(OrderPolicy::Semantic(i as u32))
                .with_safety(SafetyAnnotation::SafeReadOnly),
            );
        }
        ProviderOutcome::Answered { candidates }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::TextSpan;

    fn knowledge() -> Arc<OmenKnowledge> {
        Arc::new(OmenKnowledge {
            actions: &[],
            action_subcommands: |_| &[],
            shell_intrinsics: &[],
            tool_subcommands: |_| &[],
            is_drive_designator: |_| None,
            typed_handles: &["@last", "@failed"],
            action_reference_handles: |a| match a {
                "rerun" | "show" => &["@last"],
                _ => &[],
            },
        })
    }

    fn ctx(q: &str, position: CommandPosition) -> ProviderContext {
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
            command_position: position,
            known_executable: None,
            working_dir: ".".into(),
            project: Default::default(),
            env: Default::default(),
            depth: crate::budget::DiscoveryDepth::Normal,
        }
    }

    #[test]
    fn offers_static_handles_for_at_prefix() {
        let mut p = ReferenceProvider::new(knowledge());
        let out = p.discover(
            &ctx("@la", CommandPosition::CommandName),
            &DiscoveryBudget::inline_only(),
        );
        let names: Vec<String> = out
            .candidates()
            .iter()
            .map(|c| c.value.insert.clone())
            .collect();
        assert!(names.contains(&"@last".to_string()));
        assert!(names.contains(&"@failed".to_string()));
        // Only authoritative handles — nothing invented.
        assert!(
            names
                .iter()
                .all(|n| n.starts_with('@') && !n.contains("://"))
        );
    }

    #[test]
    fn plain_tokens_do_not_trigger() {
        let p = ReferenceProvider::new(knowledge());
        assert_eq!(
            p.triggers(&ctx("gi", CommandPosition::CommandName)),
            TriggerDecision::Skip
        );
        assert_eq!(
            p.triggers(&ctx("", CommandPosition::CommandName)),
            TriggerDecision::Skip,
            "empty command-name query is not a reference query"
        );
    }

    #[test]
    fn action_scoped_handles_only_for_declared_actions() {
        let mut p = ReferenceProvider::new(knowledge());
        let out = p.discover(
            &ctx(
                "",
                CommandPosition::ActionArg {
                    action: "rerun".into(),
                },
            ),
            &DiscoveryBudget::inline_only(),
        );
        assert_eq!(out.candidates().len(), 1);
        assert_eq!(
            p.triggers(&ctx(
                "",
                CommandPosition::ActionArg {
                    action: "status".into()
                }
            )),
            TriggerDecision::Skip,
            "free-form actions offer no static handles"
        );
    }
}
