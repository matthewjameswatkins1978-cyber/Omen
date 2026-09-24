//! Shell intrinsic provider — `cd`, `exit`, `quit`.
//!
//! **Authority:** [`Authority::Static`] (the session's compile-time authority:
//! these words are handled by Omen itself, never spawned). TIER 0
//! (memory-only): the list is injected via [`OmenKnowledge`].
//!
//! This restores M0's intrinsic completion under the one provider contract so
//! the live completer needs no second candidate source.

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

/// Discovers shell intrinsics handled by the interactive session.
pub struct IntrinsicsProvider {
    knowledge: Arc<OmenKnowledge>,
}

impl IntrinsicsProvider {
    pub fn new(knowledge: Arc<OmenKnowledge>) -> Self {
        Self { knowledge }
    }
}

impl DiscoveryProvider for IntrinsicsProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("intrinsics")
    }

    fn supported_kinds(&self) -> &'static [Kind] {
        &[Kind::Intrinsic]
    }

    fn triggers(&self, ctx: &ProviderContext) -> TriggerDecision {
        // Command-name position, never for `:action` tokens (those belong to
        // the Omen action provider) and never inside another command's grammar.
        match &ctx.command_position {
            CommandPosition::CommandName if !ctx.query().starts_with(':') => TriggerDecision::Apply,
            _ => TriggerDecision::Skip,
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
        let span = ctx.replacement_span;
        let mut candidates = Vec::new();
        for (i, name) in self.knowledge.shell_intrinsics.iter().enumerate() {
            candidates.push(
                DiscoveredCandidate::new(
                    SemanticKey::new(Kind::Intrinsic, *name, SemanticNamespace::Global),
                    ProvenanceKey::new(
                        "intrinsics",
                        AuthorityClass::Static,
                        EvidenceIdentity::Static,
                    ),
                    *name,
                    Authority::Static,
                    span,
                )
                .with_display(Display::new(*name))
                .with_description(Description::short("shell intrinsic"))
                .with_order_policy(OrderPolicy::Semantic(i as u32))
                .with_safety(SafetyAnnotation::SafeReadOnly),
            );
        }
        if candidates.is_empty() {
            return ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch,
            };
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
            shell_intrinsics: &["cd", "exit", "quit"],
            tool_subcommands: |_| &[],
            is_drive_designator: |_| None,
            typed_handles: &[],
            action_reference_handles: |_| &[],
        })
    }

    fn cmd_ctx(q: &str) -> ProviderContext {
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
    fn offers_intrinsics_in_command_name_position() {
        let mut p = IntrinsicsProvider::new(knowledge());
        let out = p.discover(&cmd_ctx("c"), &DiscoveryBudget::inline_only());
        let names: Vec<String> = out
            .candidates()
            .iter()
            .map(|c| c.value.insert.clone())
            .collect();
        assert_eq!(names, vec!["cd", "exit", "quit"]);
        assert!(
            out.candidates()
                .iter()
                .all(|c| c.authority == Authority::Static)
        );
    }

    #[test]
    fn colon_tokens_and_argument_positions_are_skipped() {
        let p = IntrinsicsProvider::new(knowledge());
        let mut colon = cmd_ctx(":st");
        assert_eq!(p.triggers(&colon), TriggerDecision::Skip);
        colon.command_position = CommandPosition::ExecArg {
            command: "git".into(),
            chain: vec![],
            option_value_of: None,
        };
        assert_eq!(p.triggers(&colon), TriggerDecision::Skip);
    }

    #[test]
    fn tier_is_memory_only() {
        assert_eq!(
            IntrinsicsProvider::new(knowledge()).cost_tier(),
            CostTier::MemoryOnly
        );
    }
}
