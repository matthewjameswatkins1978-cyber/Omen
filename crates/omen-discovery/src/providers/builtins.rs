//! Shell builtin provider — the read-only builtin registry
//! (`ls`, `grep`, `sort`, …).
//!
//! **Authority:** [`Authority::Static`] (the registry's own words, handled
//! in-process, never spawned). TIER 0 (memory-only): names and one-line
//! usage flow through [`OmenKnowledge`] — the single source of truth stays
//! the builtin registry, not a second handwritten list.

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

/// Discovers read-only shell builtins at command-name position.
pub struct BuiltinsProvider {
    knowledge: Arc<OmenKnowledge>,
}

impl BuiltinsProvider {
    pub fn new(knowledge: Arc<OmenKnowledge>) -> Self {
        Self { knowledge }
    }
}

impl DiscoveryProvider for BuiltinsProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("builtins")
    }

    fn supported_kinds(&self) -> &'static [Kind] {
        &[Kind::Intrinsic]
    }

    fn triggers(&self, ctx: &ProviderContext) -> TriggerDecision {
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
        for (i, name) in self.knowledge.shell_builtins.iter().enumerate() {
            let description = (self.knowledge.builtin_help)(name)
                .map(|(usage, _)| format!("builtin: {usage}"))
                .unwrap_or_else(|| "shell builtin".to_string());
            candidates.push(
                DiscoveredCandidate::new(
                    SemanticKey::new(Kind::Intrinsic, *name, SemanticNamespace::Global),
                    ProvenanceKey::new(
                        "builtins",
                        AuthorityClass::Static,
                        EvidenceIdentity::Static,
                    ),
                    *name,
                    Authority::Static,
                    span,
                )
                .with_display(Display::new(*name))
                .with_description(Description::short(description))
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
            shell_intrinsics: &[],
            shell_builtins: &["ls", "grep", "sort"],
            builtin_help: |name| match name {
                "ls" => Some(("ls [-a] [-l] [path ...]", "listing")),
                _ => None,
            },
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
    fn offers_builtins_with_usage_descriptions() {
        let mut p = BuiltinsProvider::new(knowledge());
        let out = p.discover(&cmd_ctx("s"), &DiscoveryBudget::inline_only());
        let names: Vec<String> = out
            .candidates()
            .iter()
            .map(|c| c.value.insert.clone())
            .collect();
        assert_eq!(names, vec!["ls", "grep", "sort"]);
        let ls = &out.candidates()[0];
        assert!(
            ls.description
                .as_ref()
                .expect("described")
                .short
                .contains("ls [-a]")
        );
        assert!(
            out.candidates()
                .iter()
                .all(|c| c.authority == Authority::Static)
        );
    }

    #[test]
    fn colon_tokens_and_argument_positions_are_skipped() {
        let p = BuiltinsProvider::new(knowledge());
        let mut colon = cmd_ctx(":st");
        assert_eq!(p.triggers(&colon), TriggerDecision::Skip);
        colon.command_position = CommandPosition::ExecArg {
            command: "ls".into(),
            chain: vec![],
            option_value_of: None,
        };
        assert_eq!(p.triggers(&colon), TriggerDecision::Skip);
    }
}
