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

    /// Static flags for one builtin at option position.
    fn discover_options(&self, ctx: &ProviderContext, command: &str) -> ProviderOutcome {
        let span = ctx.replacement_span;
        let mut candidates = Vec::new();
        for (i, flag) in (self.knowledge.builtin_options)(command).iter().enumerate() {
            candidates.push(
                DiscoveredCandidate::new(
                    SemanticKey::new(
                        Kind::Option,
                        *flag,
                        SemanticNamespace::Tool {
                            tool: command.to_string(),
                        },
                    ),
                    ProvenanceKey::new(
                        "builtins",
                        AuthorityClass::Static,
                        EvidenceIdentity::Static,
                    ),
                    *flag,
                    Authority::Static,
                    span,
                )
                .with_description(Description::short(format!("{command} option")))
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

    /// `help <topic>` candidates from the help authority.
    fn discover_help_topics(&self, ctx: &ProviderContext) -> ProviderOutcome {
        let span = ctx.replacement_span;
        let mut candidates = Vec::new();
        for (i, topic) in (self.knowledge.help_topics)().iter().enumerate() {
            candidates.push(
                DiscoveredCandidate::new(
                    SemanticKey::new(Kind::Intrinsic, *topic, SemanticNamespace::Global),
                    ProvenanceKey::new(
                        "builtins",
                        AuthorityClass::Static,
                        EvidenceIdentity::Static,
                    ),
                    *topic,
                    Authority::Static,
                    span,
                )
                .with_description(Description::short("help topic"))
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

impl DiscoveryProvider for BuiltinsProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("builtins")
    }

    fn supported_kinds(&self) -> &'static [Kind] {
        &[Kind::Intrinsic, Kind::Option]
    }

    fn triggers(&self, ctx: &ProviderContext) -> TriggerDecision {
        match &ctx.command_position {
            CommandPosition::CommandName if !ctx.query().starts_with(':') => TriggerDecision::Apply,
            // Option position, but only when the command in scope is one of
            // ours — external tools keep their own spec/harvest sources.
            CommandPosition::OptionName { command, .. }
                if self.knowledge.shell_builtins.contains(&command.as_str()) =>
            {
                TriggerDecision::Apply
            }
            // `help <topic>`: topics are builtin (and session-word) names.
            CommandPosition::ExecArg { command, .. } if command == "help" => TriggerDecision::Apply,
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
        // `help <topic>`: topics come from the help authority.
        if let CommandPosition::ExecArg { command, .. } = &ctx.command_position
            && command == "help"
        {
            return self.discover_help_topics(ctx);
        }
        // Option position serves one command's static flags.
        if let CommandPosition::OptionName { command, .. } = &ctx.command_position {
            return self.discover_options(ctx, command);
        }
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
            builtin_options: |name| match name {
                "ls" => &["-a", "-l"],
                _ => &[],
            },
            help_topics: || vec!["ls", "grep", "sort", "cd"],
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

    #[test]
    fn option_position_serves_builtin_flags_only() {
        use crate::budget::DiscoveryDepth;
        fn opt_ctx(command: &str, q: &str) -> ProviderContext {
            let mut ctx = cmd_ctx(q);
            ctx.command_position = CommandPosition::OptionName {
                command: command.to_string(),
                chain: vec![],
            };
            ctx.depth = DiscoveryDepth::Normal;
            ctx
        }
        let mut p = BuiltinsProvider::new(knowledge());
        assert_eq!(p.triggers(&opt_ctx("ls", "--")), TriggerDecision::Apply);
        let out = p.discover(&opt_ctx("ls", "--"), &DiscoveryBudget::inline_only());
        let names: Vec<String> = out
            .candidates()
            .iter()
            .map(|c| c.value.insert.clone())
            .collect();
        assert_eq!(names, vec!["-a", "-l"]);
        // External commands keep their own sources: skipped here.
        assert_eq!(p.triggers(&opt_ctx("cargo", "--")), TriggerDecision::Skip);
    }

    #[test]
    fn help_exec_arg_serves_topics() {
        let mut ctx = cmd_ctx("");
        ctx.command_position = CommandPosition::ExecArg {
            command: "help".to_string(),
            chain: vec![],
            option_value_of: None,
        };
        let p = BuiltinsProvider::new(knowledge());
        assert_eq!(p.triggers(&ctx), TriggerDecision::Apply);
        let mut p = p;
        let out = p.discover(&ctx, &DiscoveryBudget::inline_only());
        let names: Vec<String> = out
            .candidates()
            .iter()
            .map(|c| c.value.insert.clone())
            .collect();
        assert_eq!(names, vec!["ls", "grep", "sort", "cd"]);
        // Other commands' args are not topics.
        let mut other = cmd_ctx("");
        other.command_position = CommandPosition::ExecArg {
            command: "cat".to_string(),
            chain: vec![],
            option_value_of: None,
        };
        assert_eq!(p.triggers(&other), TriggerDecision::Skip);
    }
}
