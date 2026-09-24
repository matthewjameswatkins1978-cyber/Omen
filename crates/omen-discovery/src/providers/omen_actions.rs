//! Omen action provider — canonical `:action` names and their subcommands.
//!
//! **Authority:** [`Authority::OmenFact`]. TIER 0 (memory-only): the action
//! list is compile-time knowledge injected via [`OmenKnowledge`].
//!
//! The provider emits the *same* action list the dispatcher accepts; there is
//! no second handwritten list.

use std::sync::Arc;

use crate::authority::{Authority, AuthorityClass};
use crate::budget::{CostTier, DiscoveryBudget};
use crate::candidate::{
    Description, DiscoveredCandidate, Display, OrderPolicy, SafetyAnnotation, TextSpan,
};
use crate::context::{CommandPosition, ProviderContext};
use crate::identity::{EvidenceIdentity, ProvenanceKey, SemanticKey, SemanticNamespace};
use crate::kind::Kind;
use crate::outcome::{DeclineReason, ProviderOutcome};
use crate::provider::{
    Determinism, DiscoveryProvider, FreshnessPolicy, ProviderId, TriggerDecision,
};
use crate::providers::knowledge::OmenKnowledge;

/// Discovers canonical Omen `:action` names and their first-argument
/// subcommands.
pub struct OmenActionProvider {
    knowledge: Arc<OmenKnowledge>,
}

impl OmenActionProvider {
    pub fn new(knowledge: Arc<OmenKnowledge>) -> Self {
        Self { knowledge }
    }
}

impl DiscoveryProvider for OmenActionProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("omen-actions")
    }

    fn supported_kinds(&self) -> &'static [Kind] {
        &[Kind::OmenAction, Kind::Subcommand, Kind::ArgumentValue]
    }

    fn triggers(&self, ctx: &ProviderContext) -> TriggerDecision {
        let q = ctx.query();
        let in_command_name = matches!(ctx.command_position, CommandPosition::CommandName);
        let in_action_arg = matches!(ctx.command_position, CommandPosition::ActionArg { .. });
        let applies = (in_command_name && (q.starts_with(':') || q.is_empty())) || in_action_arg;
        if applies {
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
        &[AuthorityClass::OmenFact]
    }

    fn freshness(&self) -> FreshnessPolicy {
        FreshnessPolicy::Never
    }

    fn discover(&mut self, ctx: &ProviderContext, _budget: &DiscoveryBudget) -> ProviderOutcome {
        let span = ctx.replacement_span;
        let mut candidates: Vec<DiscoveredCandidate> = Vec::new();

        match &ctx.command_position {
            CommandPosition::CommandName => {
                let q = ctx.query();
                // `:action` names. Only offered when the token starts with `:`
                // or is empty (so we never steal plain command-name completion).
                if q.is_empty() || q.starts_with(':') {
                    for (i, action) in self.knowledge.actions.iter().enumerate() {
                        let value = format!(":{action}");
                        candidates.push(action_candidate(&value, action, span, i as u32));
                    }
                }
            }
            CommandPosition::ActionArg { action } => {
                let subs = (self.knowledge.action_subcommands)(action);
                for (i, s) in subs.iter().enumerate() {
                    let semantic = SemanticKey::new(
                        Kind::Subcommand,
                        *s,
                        SemanticNamespace::Action {
                            action: action.clone(),
                        },
                    );
                    candidates.push(
                        DiscoveredCandidate::new(
                            semantic,
                            ProvenanceKey::new(
                                "omen-actions",
                                AuthorityClass::OmenFact,
                                EvidenceIdentity::OmenFact {
                                    fact_id: format!("omen-action:{action}:{s}"),
                                },
                            ),
                            *s,
                            Authority::OmenFact {
                                fact_id: format!("omen-action:{action}:{s}"),
                            },
                            span,
                        )
                        .with_description(Description::short(format!("':{action}' subcommand")))
                        .with_order_policy(OrderPolicy::Semantic(i as u32)),
                    );
                }
            }
            _ => {}
        }

        if candidates.is_empty() {
            return ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch,
            };
        }
        ProviderOutcome::Answered { candidates }
    }
}

fn action_candidate(
    value: &str,
    action: &str,
    span: TextSpan,
    ordinal: u32,
) -> DiscoveredCandidate {
    DiscoveredCandidate::new(
        SemanticKey::new(Kind::OmenAction, value, SemanticNamespace::Global),
        ProvenanceKey::new(
            "omen-actions",
            AuthorityClass::OmenFact,
            EvidenceIdentity::OmenFact {
                fact_id: format!("omen-action:{action}"),
            },
        ),
        value,
        Authority::OmenFact {
            fact_id: format!("omen-action:{action}"),
        },
        span,
    )
    .with_display(Display::new(value))
    .with_description(Description::short(format!(
        "Omen semantic action '{action}'"
    )))
    .with_order_policy(OrderPolicy::Semantic(ordinal))
    .with_safety(SafetyAnnotation::SafeReadOnly)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knowledge() -> Arc<OmenKnowledge> {
        Arc::new(OmenKnowledge {
            actions: &["status", "agent", "tools"],
            action_subcommands: |a| match a {
                "agent" => &["providers", "status"],
                _ => &[],
            },
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
    fn offers_colon_actions_in_command_name_position() {
        let mut p = OmenActionProvider::new(knowledge());
        let out = p.discover(&cmd_ctx(":"), &DiscoveryBudget::inline_only());
        let c = out.candidates();
        assert!(c.iter().any(|x| x.value.insert == ":status"));
        assert!(
            c.iter()
                .all(|x| matches!(x.authority, Authority::OmenFact { .. }))
        );
    }

    #[test]
    fn does_not_steal_plain_command_name_completion() {
        let p = OmenActionProvider::new(knowledge());
        assert_eq!(
            p.triggers(&cmd_ctx("gi")),
            TriggerDecision::Skip,
            "plain tokens must not surface :actions"
        );
    }

    #[test]
    fn action_subcommands_use_injected_grammar() {
        let mut p = OmenActionProvider::new(knowledge());
        let mut c = cmd_ctx("");
        c.command_position = CommandPosition::ActionArg {
            action: "agent".into(),
        };
        let out = p.discover(&c, &DiscoveryBudget::inline_only());
        let names: Vec<String> = out
            .candidates()
            .iter()
            .map(|x| x.value.insert.clone())
            .collect();
        assert_eq!(names, vec!["providers", "status"]);
    }

    #[test]
    fn tier_is_memory_only() {
        assert_eq!(
            OmenActionProvider::new(knowledge()).cost_tier(),
            CostTier::MemoryOnly
        );
    }
}
