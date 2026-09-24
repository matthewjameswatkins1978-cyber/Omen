//! Structured / declarative tool option specs.
//!
//! **Authority ladder rung 1–3:** structured tool-native metadata, declarative
//! inert installed specs, and Omen-owned metadata.
//!
//! These are **inert data** parsed under a bounded explicit schema. Executable
//! foreign completion logic (bash/zsh/fish/PowerShell scripts, closures, eval,
//! dynamic conditions) is **never** run to populate this.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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

/// Where a spec came from. Determines the candidate's concrete authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecOrigin {
    /// Structured tool-native metadata (clap, `__complete` protocol, Omen registry).
    ToolNative { spec_version: Option<String> },
    /// Declarative inert installed spec, parsed under a bounded schema.
    InstalledSpec { digest: String },
    /// Omen-owned metadata.
    OmenOwned,
}

/// How many values an option consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Arity {
    /// Boolean flag; no value.
    None,
    /// Requires a value.
    Required,
    /// Value optional.
    Optional,
    /// May repeat.
    Repeatable,
}

/// Typed hint for what a value looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueHint {
    Path,
    Directory,
    Executable,
    CommandName,
    Url,
    Other,
}

/// One option declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionSpec {
    pub long: Option<String>,
    pub short: Option<String>,
    pub description: Option<Description>,
    pub arity: Arity,
    pub required: bool,
    pub value_hint: Option<ValueHint>,
    pub order_ordinal: u32,
    pub safety: SafetyAnnotation,
    pub deprecated: bool,
    pub hidden: bool,
}

impl OptionSpec {
    /// The canonical insertion text for this option (long preferred).
    pub fn insert_text(&self) -> String {
        match (&self.long, &self.short) {
            (Some(l), _) => format!("--{l}"),
            (None, Some(s)) => format!("-{s}"),
            (None, None) => String::new(),
        }
    }

    pub fn primary_name(&self) -> String {
        self.long
            .clone()
            .or_else(|| self.short.clone())
            .unwrap_or_default()
    }
}

/// One subcommand declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubcommandSpec {
    pub name: String,
    pub description: Option<Description>,
    pub order_ordinal: u32,
}

/// An inert tool grammar description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub tool: String,
    pub origin: SpecOrigin,
    pub options: Vec<OptionSpec>,
    pub subcommands: Vec<SubcommandSpec>,
}

/// Registry of inert tool specs. Bounded and explicit.
#[derive(Debug, Clone, Default)]
pub struct ToolSpecRegistry {
    specs: HashMap<String, ToolSpec>,
}

impl ToolSpecRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an inert spec. Replaces any existing spec for the same tool.
    pub fn register(&mut self, spec: ToolSpec) {
        self.specs.insert(spec.tool.clone(), spec);
    }

    pub fn get(&self, tool: &str) -> Option<&ToolSpec> {
        self.specs.get(tool)
    }

    pub fn len(&self) -> usize {
        self.specs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }
}

fn authority_for(origin: &SpecOrigin, tool: &str) -> Authority {
    match origin {
        SpecOrigin::ToolNative { spec_version } => Authority::ToolNative {
            tool: tool.to_string(),
            spec_version: spec_version.clone(),
        },
        SpecOrigin::InstalledSpec { digest } => Authority::InstalledSpec {
            origin: format!("spec:{tool}"),
            digest: digest.clone(),
        },
        SpecOrigin::OmenOwned => Authority::OmenFact {
            fact_id: format!("omen-spec:{tool}"),
        },
    }
}

fn authority_class(origin: &SpecOrigin) -> AuthorityClass {
    match origin {
        SpecOrigin::ToolNative { .. } => AuthorityClass::ToolNative,
        SpecOrigin::InstalledSpec { .. } => AuthorityClass::InstalledSpec,
        SpecOrigin::OmenOwned => AuthorityClass::OmenFact,
    }
}

fn evidence_for(origin: &SpecOrigin, tool: &str) -> EvidenceIdentity {
    match origin {
        SpecOrigin::ToolNative { spec_version } => EvidenceIdentity::ToolNative {
            tool: tool.to_string(),
            spec_version: spec_version.clone(),
        },
        SpecOrigin::InstalledSpec { digest } => EvidenceIdentity::InstalledSpec {
            digest: digest.clone(),
        },
        SpecOrigin::OmenOwned => EvidenceIdentity::OmenFact {
            fact_id: format!("omen-spec:{tool}"),
        },
    }
}

/// Answers option/subcommand queries from inert structured specs.
///
/// **TIER 1A**: pure in-memory lookup. Never touches disk or spawns a process.
pub struct ToolSpecProvider {
    registry: ToolSpecRegistry,
}

impl ToolSpecProvider {
    pub fn new(registry: ToolSpecRegistry) -> Self {
        Self { registry }
    }

    pub fn registry(&self) -> &ToolSpecRegistry {
        &self.registry
    }
}

/// Builds the Omen-owned declarative tool specs (authority-ladder rung 3).
///
/// These are inert data under a bounded schema. They assert *syntax* (which
/// options/subcommands exist and what they mean), not executable behaviour.
/// Where a structured tool-native source exists it outranks these on merge; the
/// strict help harvest fills gaps and is outranked by both.
pub fn default_tool_specs() -> ToolSpecRegistry {
    let mut reg = ToolSpecRegistry::new();

    reg.register(ToolSpec {
        tool: "git".into(),
        origin: SpecOrigin::OmenOwned,
        options: vec![
            opt("verbose", Some('v'), "be more verbose", 0),
            opt("quiet", Some('q'), "be quiet", 1),
            opt("help", None, "show help", 2),
            opt("version", None, "show version", 3),
            opt("git-dir", None, "set the path to the repository", 4),
            opt("work-tree", None, "set the working tree", 5),
        ],
        subcommands: vec![
            sub("status", "show the working tree status", 0),
            sub("commit", "record changes to the repository", 1),
            sub("checkout", "switch branches or restore files", 2),
            sub("push", "update remote refs", 3),
            sub("log", "show commit logs", 4),
        ],
    });

    reg.register(ToolSpec {
        tool: "cargo".into(),
        origin: SpecOrigin::OmenOwned,
        options: vec![
            opt("help", None, "show help", 0),
            opt("version", Some('V'), "show version", 1),
            opt("verbose", Some('v'), "use verbose output", 2),
            opt("quiet", Some('q'), "do not print cargo log messages", 3),
        ],
        subcommands: vec![
            sub("build", "compile the current package", 0),
            sub("check", "check the current package", 1),
            sub("test", "run tests", 2),
            sub("run", "run a binary or example", 3),
            sub("clippy", "run clippy lints", 4),
            sub("fmt", "format the source", 5),
        ],
    });

    reg
}

fn opt(long: &str, short: Option<char>, description: &str, ordinal: u32) -> OptionSpec {
    OptionSpec {
        long: Some(long.to_string()),
        short: short.map(|c| c.to_string()),
        description: Some(Description::short(description)),
        arity: Arity::None,
        required: false,
        value_hint: None,
        order_ordinal: ordinal,
        safety: SafetyAnnotation::UnknownImpact,
        deprecated: false,
        hidden: false,
    }
}

fn sub(name: &str, description: &str, ordinal: u32) -> SubcommandSpec {
    SubcommandSpec {
        name: name.to_string(),
        description: Some(Description::short(description)),
        order_ordinal: ordinal,
    }
}

impl DiscoveryProvider for ToolSpecProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("tool-spec")
    }

    fn supported_kinds(&self) -> &'static [Kind] {
        &[Kind::Option, Kind::Subcommand, Kind::ArgumentValue]
    }

    fn triggers(&self, ctx: &ProviderContext) -> TriggerDecision {
        let Some(tool) = ctx.tool_in_scope() else {
            return TriggerDecision::Skip;
        };
        if self.registry.get(tool).is_none() {
            return TriggerDecision::Skip;
        }
        TriggerDecision::Apply
    }

    fn cost_tier(&self) -> CostTier {
        CostTier::CheapLocal
    }

    fn determinism(&self) -> Determinism {
        Determinism::Deterministic
    }

    fn authority_capabilities(&self) -> &'static [AuthorityClass] {
        &[
            AuthorityClass::ToolNative,
            AuthorityClass::InstalledSpec,
            AuthorityClass::OmenFact,
        ]
    }

    fn freshness(&self) -> FreshnessPolicy {
        FreshnessPolicy::Never
    }

    fn discover(&mut self, ctx: &ProviderContext, _budget: &DiscoveryBudget) -> ProviderOutcome {
        let Some(tool) = ctx.tool_in_scope() else {
            return ProviderOutcome::Declined {
                reason: DeclineReason::NotApplicable,
            };
        };
        let Some(spec) = self.registry.get(tool).cloned() else {
            return ProviderOutcome::Declined {
                reason: DeclineReason::NotApplicable,
            };
        };

        let span = ctx.replacement_span;
        let namespace = SemanticNamespace::Tool {
            tool: tool.to_string(),
        };
        let class = authority_class(&spec.origin);
        let evidence = evidence_for(&spec.origin, tool);
        let authority = authority_for(&spec.origin, tool);

        let mut candidates = Vec::new();
        let wants_options = ctx.is_option_name_position();
        let wants_subcommands = matches!(
            &ctx.command_position,
            CommandPosition::ExecArg { chain, .. } if chain.is_empty()
        );

        if wants_options {
            for o in &spec.options {
                if o.hidden {
                    continue;
                }
                let insert = o.insert_text();
                if insert.is_empty() {
                    continue;
                }
                candidates.push(
                    DiscoveredCandidate::new(
                        SemanticKey::new(Kind::Option, &insert, namespace.clone()),
                        ProvenanceKey::new("tool-spec", class, evidence.clone()),
                        insert,
                        authority.clone(),
                        span,
                    )
                    .with_display(Display::new(o.primary_name()))
                    .with_description(o.description.clone().unwrap_or_else(|| {
                        Description::short(if o.required {
                            "required option"
                        } else {
                            "option"
                        })
                    }))
                    .with_order_policy(OrderPolicy::Semantic(o.order_ordinal))
                    .with_safety(o.safety),
                );
            }
        }

        if wants_subcommands {
            for s in &spec.subcommands {
                candidates.push(
                    DiscoveredCandidate::new(
                        SemanticKey::new(Kind::Subcommand, &s.name, namespace.clone()),
                        ProvenanceKey::new("tool-spec", class, evidence.clone()),
                        s.name.clone(),
                        authority.clone(),
                        span,
                    )
                    .with_display(Display::new(&s.name))
                    .with_description(
                        s.description
                            .clone()
                            .unwrap_or_else(|| Description::short("subcommand")),
                    )
                    .with_order_policy(OrderPolicy::Semantic(s.order_ordinal)),
                );
            }
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

    fn spec() -> ToolSpec {
        ToolSpec {
            tool: "git".into(),
            origin: SpecOrigin::ToolNative {
                spec_version: Some("2".into()),
            },
            options: vec![
                OptionSpec {
                    long: Some("verbose".into()),
                    short: Some("v".into()),
                    description: Some(Description::short("be more verbose")),
                    arity: Arity::None,
                    required: false,
                    value_hint: None,
                    order_ordinal: 0,
                    safety: SafetyAnnotation::SafeReadOnly,
                    deprecated: false,
                    hidden: false,
                },
                OptionSpec {
                    long: Some("force".into()),
                    short: None,
                    description: Some(Description::short("force it")),
                    arity: Arity::None,
                    required: false,
                    value_hint: None,
                    order_ordinal: 1,
                    safety: SafetyAnnotation::Destructive,
                    deprecated: false,
                    hidden: false,
                },
                OptionSpec {
                    long: Some("secret".into()),
                    short: None,
                    description: None,
                    arity: Arity::None,
                    required: false,
                    value_hint: None,
                    order_ordinal: 2,
                    safety: SafetyAnnotation::UnknownImpact,
                    deprecated: false,
                    hidden: true,
                },
            ],
            subcommands: vec![SubcommandSpec {
                name: "commit".into(),
                description: Some(Description::short("record changes")),
                order_ordinal: 0,
            }],
        }
    }

    fn opt_ctx() -> ProviderContext {
        ProviderContext {
            buffer: "git --ver".into(),
            cursor: 9,
            active_token: crate::context::ActiveToken {
                span: crate::candidate::TextSpan::new(4, 9),
                decoded_prefix: "--ver".into(),
                decoded_suffix: String::new(),
                literal: "--ver".into(),
                raw_prefix: "--ver".into(),
                raw_suffix: String::new(),
                split_unsafe: false,
                quote_unclosed: false,
            },
            replacement_span: crate::candidate::TextSpan::new(4, 9),
            command_position: CommandPosition::OptionName {
                command: "git".into(),
                chain: vec![],
            },
            known_executable: None,
            working_dir: ".".into(),
            project: Default::default(),
            env: Default::default(),
            depth: crate::budget::DiscoveryDepth::Normal,
        }
    }

    fn provider() -> ToolSpecProvider {
        let mut reg = ToolSpecRegistry::new();
        reg.register(spec());
        ToolSpecProvider::new(reg)
    }

    #[test]
    fn emits_structured_options_with_tool_native_authority() {
        let mut p = provider();
        let out = p.discover(&opt_ctx(), &DiscoveryBudget::inline_only());
        let verbose = out
            .candidates()
            .iter()
            .find(|c| c.value.insert == "--verbose")
            .unwrap();
        assert!(matches!(verbose.authority, Authority::ToolNative { .. }));
        assert_eq!(
            verbose.description.as_ref().unwrap().short,
            "be more verbose"
        );
    }

    #[test]
    fn hidden_options_are_not_emitted() {
        let mut p = provider();
        let out = p.discover(&opt_ctx(), &DiscoveryBudget::inline_only());
        assert!(
            !out.candidates()
                .iter()
                .any(|c| c.value.insert == "--secret")
        );
    }

    #[test]
    fn insert_text_prefers_long_form() {
        let o = spec().options[0].clone();
        assert_eq!(o.insert_text(), "--verbose");
    }

    #[test]
    fn tier_is_cheap_local() {
        assert_eq!(provider().cost_tier(), CostTier::CheapLocal);
    }

    #[test]
    fn no_spec_declines_without_fabrication() {
        let p = ToolSpecProvider::new(ToolSpecRegistry::new());
        assert_eq!(p.triggers(&opt_ctx()), TriggerDecision::Skip);
    }
}
