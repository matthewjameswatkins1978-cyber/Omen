//! Hot semantic index provider — bounded session knowledge injected
//! out-of-band by the interactive layer.
//!
//! Carries M0's hot-index completion sources under the one provider contract:
//! `@fact.*` handles, `@service.*` handles, and per-action semantic values
//! (symbols, packages, tasks, services).
//!
//! **Authority:**
//! - facts / symbols / packages / tasks -> [`Authority::OmenFact`] (Omen's own
//!   typed knowledge),
//! - services -> [`Authority::Environment`] (observed service registry).
//!
//! TIER 0 (memory-only): the snapshot is already in memory; refreshes happen
//! out-of-band, never on the keystroke path.

use std::sync::{Arc, Mutex};

use crate::authority::{Authority, AuthorityClass};
use crate::budget::{CostTier, DiscoveryBudget};
use crate::candidate::{Description, DiscoveredCandidate, Display, SafetyAnnotation};
use crate::context::{CommandPosition, ProviderContext};
use crate::identity::{EvidenceIdentity, ProvenanceKey, SemanticKey, SemanticNamespace};
use crate::kind::Kind;
use crate::outcome::{DeclineReason, ProviderOutcome};
use crate::provider::{
    Determinism, DiscoveryProvider, FreshnessPolicy, ProviderId, TriggerDecision,
};

/// One cached fact observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotFact {
    /// `fact://...` resource URI.
    pub resource_uri: String,
    /// Validity label as the interactive layer presents it (`CURRENT`,
    /// `DIRTY`, `STALE`, ...). Presentation-only; never validity evidence.
    pub validity: String,
}

/// Bounded snapshot of the session's hot semantic index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HotIndexSnapshot {
    pub facts: Vec<HotFact>,
    pub symbols: Vec<String>,
    pub packages: Vec<String>,
    pub tasks: Vec<String>,
    pub services: Vec<String>,
}

/// Actions whose arguments are completed from the hot index.
const SYMBOL_ACTIONS: &[&str] = &["symbol", "def", "refs"];
const SERVICE_ACTIONS: &[&str] = &["stop", "status", "services"];

/// Discovers candidates from the injected hot semantic snapshot.
pub struct HotIndexProvider {
    snapshot: Arc<Mutex<HotIndexSnapshot>>,
}

impl HotIndexProvider {
    pub fn new(snapshot: Arc<Mutex<HotIndexSnapshot>>) -> Self {
        Self { snapshot }
    }

    fn snap(&self) -> HotIndexSnapshot {
        self.snapshot.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl DiscoveryProvider for HotIndexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("hot-semantic-index")
    }

    fn supported_kinds(&self) -> &'static [Kind] {
        &[Kind::Resource, Kind::Service]
    }

    fn triggers(&self, ctx: &ProviderContext) -> TriggerDecision {
        match &ctx.command_position {
            CommandPosition::CommandName if ctx.query().starts_with('@') => TriggerDecision::Apply,
            CommandPosition::ActionArg { action }
                if SYMBOL_ACTIONS.contains(&action.as_str())
                    || SERVICE_ACTIONS.contains(&action.as_str())
                    || action == "packages"
                    || action == "tasks" =>
            {
                TriggerDecision::Apply
            }
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
        &[AuthorityClass::OmenFact, AuthorityClass::Environment]
    }

    fn freshness(&self) -> FreshnessPolicy {
        FreshnessPolicy::IdentityBound
    }

    fn discover(&mut self, ctx: &ProviderContext, _budget: &DiscoveryBudget) -> ProviderOutcome {
        let snap = self.snap();
        let span = ctx.replacement_span;
        let q = ctx.query().to_string();
        let mut candidates = Vec::new();

        match &ctx.command_position {
            CommandPosition::CommandName => {
                // `@fact.<name>` handles. Emitted whenever the query is inside
                // (or a prefix of) the `@fact.` namespace; the matcher applies
                // the final prefix filter — matching is not identity.
                let show_facts = q.starts_with("@fact.") || "@fact.".starts_with(&q);
                if show_facts {
                    for f in &snap.facts {
                        let name = f
                            .resource_uri
                            .strip_prefix("fact://")
                            .unwrap_or(&f.resource_uri)
                            .to_string();
                        let value = format!("@fact.{name}");
                        candidates.push(
                            DiscoveredCandidate::new(
                                SemanticKey::new(Kind::Resource, &value, SemanticNamespace::Global),
                                ProvenanceKey::new(
                                    "hot-semantic-index",
                                    AuthorityClass::OmenFact,
                                    EvidenceIdentity::OmenFact {
                                        fact_id: f.resource_uri.clone(),
                                    },
                                ),
                                value.clone(),
                                Authority::OmenFact {
                                    fact_id: f.resource_uri.clone(),
                                },
                                span,
                            )
                            .with_display(Display::new(&value))
                            .with_description(
                                Description::short(format!("fact {name}"))
                                    .with_detail(format!("fact ({})", f.validity)),
                            )
                            .with_safety(SafetyAnnotation::SafeReadOnly),
                        );
                    }
                }

                let show_services = q.starts_with("@service.") || "@service.".starts_with(&q);
                if show_services {
                    for s in &snap.services {
                        push_service(&mut candidates, s, span);
                    }
                }
            }
            CommandPosition::ActionArg { action } => {
                let list: &[String] = match action.as_str() {
                    a if SYMBOL_ACTIONS.contains(&a) => &snap.symbols,
                    "packages" => &snap.packages,
                    "tasks" => &snap.tasks,
                    a if SERVICE_ACTIONS.contains(&a) => &snap.services,
                    _ => &[],
                };
                let as_service = SERVICE_ACTIONS.contains(&action.as_str());
                for item in list {
                    if as_service {
                        push_service(&mut candidates, item, span);
                    } else {
                        let value = item.clone();
                        candidates.push(
                            DiscoveredCandidate::new(
                                SemanticKey::new(
                                    Kind::Resource,
                                    &value,
                                    SemanticNamespace::Action {
                                        action: action.clone(),
                                    },
                                ),
                                ProvenanceKey::new(
                                    "hot-semantic-index",
                                    AuthorityClass::OmenFact,
                                    EvidenceIdentity::OmenFact {
                                        fact_id: format!("semantic-index:{value}"),
                                    },
                                ),
                                value.clone(),
                                Authority::OmenFact {
                                    fact_id: format!("semantic-index:{value}"),
                                },
                                span,
                            )
                            .with_display(Display::new(&value))
                            .with_description(Description::short("resource"))
                            .with_safety(SafetyAnnotation::SafeReadOnly),
                        );
                    }
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

fn push_service(
    candidates: &mut Vec<DiscoveredCandidate>,
    name: &str,
    span: crate::candidate::TextSpan,
) {
    let value = format!("@service.{name}");
    candidates.push(
        DiscoveredCandidate::new(
            SemanticKey::new(Kind::Service, &value, SemanticNamespace::Global),
            ProvenanceKey::new(
                "hot-semantic-index",
                AuthorityClass::Environment,
                EvidenceIdentity::Environment {
                    fact: format!("service:{name}"),
                },
            ),
            value.clone(),
            Authority::Environment {
                fact: format!("service:{name}"),
            },
            span,
        )
        .with_display(Display::new(&value))
        .with_description(Description::short("service"))
        .with_safety(SafetyAnnotation::UnknownImpact),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::TextSpan;

    fn snapshot() -> Arc<Mutex<HotIndexSnapshot>> {
        Arc::new(Mutex::new(HotIndexSnapshot {
            facts: vec![HotFact {
                resource_uri: "fact://git/branch".into(),
                validity: "DIRTY".into(),
            }],
            symbols: vec!["resolve_me".into()],
            packages: vec!["omen".into()],
            tasks: vec!["build".into()],
            services: vec!["daemon".into()],
        }))
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
    fn fact_handles_carry_validity_detail() {
        let mut p = HotIndexProvider::new(snapshot());
        let out = p.discover(
            &ctx("@fact.git", CommandPosition::CommandName),
            &DiscoveryBudget::inline_only(),
        );
        let c = &out.candidates()[0];
        assert_eq!(c.value.insert, "@fact.git/branch");
        let d = c.description.as_ref().unwrap();
        assert_eq!(d.detail.as_deref(), Some("fact (DIRTY)"));
        assert!(matches!(c.authority, Authority::OmenFact { .. }));
    }

    #[test]
    fn service_handles_use_environment_authority() {
        let mut p = HotIndexProvider::new(snapshot());
        let out = p.discover(
            &ctx("@service.", CommandPosition::CommandName),
            &DiscoveryBudget::inline_only(),
        );
        assert_eq!(out.candidates()[0].value.insert, "@service.daemon");
        assert!(matches!(
            out.candidates()[0].authority,
            Authority::Environment { .. }
        ));
    }

    #[test]
    fn action_arguments_draw_from_their_own_lists() {
        let mut p = HotIndexProvider::new(snapshot());
        let out = p.discover(
            &ctx(
                "",
                CommandPosition::ActionArg {
                    action: "symbol".into(),
                },
            ),
            &DiscoveryBudget::inline_only(),
        );
        assert_eq!(out.candidates()[0].value.insert, "resolve_me");

        let out = p.discover(
            &ctx(
                "",
                CommandPosition::ActionArg {
                    action: "packages".into(),
                },
            ),
            &DiscoveryBudget::inline_only(),
        );
        assert_eq!(out.candidates()[0].value.insert, "omen");
    }

    #[test]
    fn plain_command_name_tokens_do_not_trigger() {
        let p = HotIndexProvider::new(snapshot());
        assert_eq!(
            p.triggers(&ctx("gi", CommandPosition::CommandName)),
            TriggerDecision::Skip
        );
    }
}
