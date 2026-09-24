//! Subcommand provider — typed subcommand discovery from structured authority.
//!
//! **Authority:** [`Authority::ToolNative`] when a structured tool spec
//! provides the subcommands, otherwise [`Authority::Static`] for injected
//! syntax metadata. TIER 0 (memory-only).
//!
//! Syntax metadata asserts *which words are canonical arguments*, not that the
//! tool exists; command-position availability comes from the PATH provider.

use std::sync::Arc;

use crate::authority::{Authority, AuthorityClass};
use crate::budget::{CostTier, DiscoveryBudget};
use crate::candidate::{Description, DiscoveredCandidate, OrderPolicy, SafetyAnnotation};
use crate::context::{CommandPosition, ProviderContext};
use crate::identity::{EvidenceIdentity, ProvenanceKey, SemanticKey, SemanticNamespace};
use crate::kind::Kind;
use crate::outcome::{DeclineReason, ProviderOutcome};
use crate::provider::{
    Determinism, DiscoveryProvider, FreshnessPolicy, ProviderId, TriggerDecision,
};
use crate::providers::knowledge::OmenKnowledge;

/// Discovers subcommands of an external tool's grammar.
pub struct SubcommandProvider {
    knowledge: Arc<OmenKnowledge>,
}

impl SubcommandProvider {
    pub fn new(knowledge: Arc<OmenKnowledge>) -> Self {
        Self { knowledge }
    }
}

impl DiscoveryProvider for SubcommandProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("subcommands")
    }

    fn supported_kinds(&self) -> &'static [Kind] {
        &[Kind::Subcommand]
    }

    fn triggers(&self, ctx: &ProviderContext) -> TriggerDecision {
        match &ctx.command_position {
            CommandPosition::ExecArg { chain, .. } if chain.is_empty() => TriggerDecision::Apply,
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
        &[AuthorityClass::Static, AuthorityClass::ToolNative]
    }

    fn freshness(&self) -> FreshnessPolicy {
        FreshnessPolicy::Never
    }

    fn discover(&mut self, ctx: &ProviderContext, _budget: &DiscoveryBudget) -> ProviderOutcome {
        let Some(command) = ctx.tool_in_scope() else {
            return ProviderOutcome::Declined {
                reason: DeclineReason::NotApplicable,
            };
        };
        let subs = (self.knowledge.tool_subcommands)(command);
        if subs.is_empty() {
            return ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch,
            };
        }

        let span = ctx.replacement_span;
        let mut candidates = Vec::new();
        for (i, s) in subs.iter().enumerate() {
            let semantic = SemanticKey::new(
                Kind::Subcommand,
                *s,
                SemanticNamespace::Tool {
                    tool: command.to_string(),
                },
            );
            candidates.push(
                DiscoveredCandidate::new(
                    semantic,
                    ProvenanceKey::new(
                        "subcommands",
                        AuthorityClass::Static,
                        EvidenceIdentity::Static,
                    ),
                    *s,
                    Authority::Static,
                    span,
                )
                .with_description(Description::short(format!("{command} subcommand")))
                .with_order_policy(OrderPolicy::Semantic(i as u32))
                .with_safety(SafetyAnnotation::UnknownImpact),
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
            shell_intrinsics: &["cd"],
            tool_subcommands: |t| match t {
                "cargo" => &["build", "test"],
                _ => &[],
            },
            is_drive_designator: |_| None,
        })
    }

    fn exec_ctx(command: &str, q: &str) -> ProviderContext {
        ProviderContext {
            buffer: format!("{command} {q}"),
            cursor: command.len() + 1 + q.len(),
            active_token: crate::context::ActiveToken {
                span: TextSpan::new(command.len() + 1, command.len() + 1 + q.len()),
                decoded_prefix: q.to_string(),
                decoded_suffix: String::new(),
                literal: q.to_string(),
                raw_prefix: q.to_string(),
                raw_suffix: String::new(),
                split_unsafe: false,
                quote_unclosed: false,
            },
            replacement_span: TextSpan::new(command.len() + 1, command.len() + 1 + q.len()),
            command_position: CommandPosition::ExecArg {
                command: command.to_string(),
                chain: vec![],
                option_value_of: None,
            },
            known_executable: None,
            working_dir: ".".into(),
            project: Default::default(),
            env: Default::default(),
            depth: crate::budget::DiscoveryDepth::Normal,
        }
    }

    #[test]
    fn discovers_tool_subcommands() {
        let mut p = SubcommandProvider::new(knowledge());
        let out = p.discover(&exec_ctx("cargo", ""), &DiscoveryBudget::inline_only());
        let names: Vec<String> = out
            .candidates()
            .iter()
            .map(|c| c.value.insert.clone())
            .collect();
        assert_eq!(names, vec!["build", "test"]);
    }

    #[test]
    fn unknown_tool_declines() {
        let mut p = SubcommandProvider::new(knowledge());
        let out = p.discover(&exec_ctx("unknown", ""), &DiscoveryBudget::inline_only());
        assert!(matches!(
            out,
            ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch
            }
        ));
    }

    #[test]
    fn nested_chain_is_skipped_in_m1() {
        let p = SubcommandProvider::new(knowledge());
        let mut c = exec_ctx("cargo", "");
        c.command_position = CommandPosition::ExecArg {
            command: "cargo".into(),
            chain: vec!["build".into()],
            option_value_of: None,
        };
        assert_eq!(p.triggers(&c), TriggerDecision::Skip);
    }
}
