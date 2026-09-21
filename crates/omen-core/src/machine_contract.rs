//! Canonical, bounded machine-facing Omen contract.
//!
//! `MachineContract` contains static semantic truth only. Runtime availability,
//! admission, workspace and provider state live in `MachineContext` and are
//! joined only when a caller asks for a projected discovery response.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const CONTRACT_VERSION: &str = "0.8";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    Read,
    Compute,
    SpawnProcess,
    Mutate,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkEffect {
    None,
    External,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reversibility {
    Reversible,
    Irreversible,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilityState {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionState {
    Admitted,
    NotAdmitted,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityDefinition {
    pub id: String,
    pub group: String,
    pub summary: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub effect_class: EffectClass,
    pub network_effect: NetworkEffect,
    pub reversibility: Reversibility,
    pub idempotent: bool,
    pub authority: String,
    pub bounds: String,
    pub timeout: String,
    pub examples: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityStatus {
    pub id: String,
    pub availability: AvailabilityState,
    pub admission: AdmissionState,
    pub provider: String,
    pub assurance: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityProjection {
    pub definition: CapabilityDefinition,
    pub status: CapabilityStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecipeStep {
    pub capability_id: String,
    pub purpose: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecipeDefinition {
    pub id: String,
    pub summary: String,
    pub steps: Vec<RecipeStep>,
    pub advisory: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineContract {
    pub contract_version: String,
    pub capability_definitions: Vec<CapabilityDefinition>,
    pub recipe_definitions: Vec<RecipeDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineContext {
    pub context_generation: Option<i64>,
    pub generation_status: String,
    pub capability_statuses: Vec<CapabilityStatus>,
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":properties,"required":required,"additionalProperties":false})
}

fn result_schema() -> Value {
    object_schema(
        json!({"status":{"type":"string"},"results":{"type":"array","maxItems":100},"references":{"type":"array","maxItems":16},"content":{"type":"string"},"stdout":{"type":"string"},"stderr":{"type":"string"}}),
        &["status"],
    )
}

fn history_schema() -> Value {
    object_schema(
        json!({
            "schema_version": {"type":"integer"},
            "entries": {"type":"array","maxItems":100},
            "limit": {"type":"integer","minimum":1,"maximum":100},
            "all_sessions": {"type":"boolean"},
            "ordering": {"type":"string"},
            "pagination": {"type":"string"}
        }),
        &[
            "schema_version",
            "entries",
            "limit",
            "all_sessions",
            "ordering",
            "pagination",
        ],
    )
}

pub fn contract() -> MachineContract {
    let empty = object_schema(json!({}), &[]);
    let result = result_schema();
    let plan_result = object_schema(
        json!({"plan":{"type":"object"},"status":{"type":"string"}}),
        &["plan", "status"],
    );
    let capability_definitions = vec![
        CapabilityDefinition {
            id: "semantic.references".into(),
            group: "semantic".into(),
            summary: "Find references to a known symbol.".into(),
            input_schema: object_schema(
                json!({"symbol":{"type":"string","minLength":1}}),
                &["symbol"],
            ),
            output_schema: result.clone(),
            effect_class: EffectClass::Read,
            network_effect: NetworkEffect::None,
            reversibility: Reversibility::NotApplicable,
            idempotent: true,
            authority: "none".into(),
            bounds: "max 100 results".into(),
            timeout: "bounded provider timeout".into(),
            examples: vec![json!({"symbol":"refresh_token"})],
        },
        CapabilityDefinition {
            id: "semantic.definition".into(),
            group: "semantic".into(),
            summary: "Resolve the definition of a known symbol.".into(),
            input_schema: object_schema(
                json!({"symbol":{"type":"string","minLength":1}}),
                &["symbol"],
            ),
            output_schema: result.clone(),
            effect_class: EffectClass::Read,
            network_effect: NetworkEffect::None,
            reversibility: Reversibility::NotApplicable,
            idempotent: true,
            authority: "none".into(),
            bounds: "one bounded result".into(),
            timeout: "bounded provider timeout".into(),
            examples: vec![json!({"symbol":"refresh_token"})],
        },
        CapabilityDefinition {
            id: "semantic.diagnostics".into(),
            group: "semantic".into(),
            summary: "Return deterministic workspace diagnostics when a provider is available."
                .into(),
            input_schema: empty.clone(),
            output_schema: result.clone(),
            effect_class: EffectClass::Read,
            network_effect: NetworkEffect::None,
            reversibility: Reversibility::NotApplicable,
            idempotent: true,
            authority: "none".into(),
            bounds: "bounded diagnostics".into(),
            timeout: "bounded provider timeout".into(),
            examples: vec![json!({})],
        },
        CapabilityDefinition {
            id: "structure.search".into(),
            group: "structure".into(),
            summary: "Search source using the structural adapter.".into(),
            input_schema: object_schema(
                json!({"pattern":{"type":"string","minLength":1}}),
                &["pattern"],
            ),
            output_schema: result.clone(),
            effect_class: EffectClass::Read,
            network_effect: NetworkEffect::None,
            reversibility: Reversibility::NotApplicable,
            idempotent: true,
            authority: "none".into(),
            bounds: "bounded matches".into(),
            timeout: "bounded adapter timeout".into(),
            examples: vec![json!({"pattern":"fn $NAME($$$ARGS) { $$$BODY }"})],
        },
        CapabilityDefinition {
            id: "history.query".into(),
            group: "history".into(),
            summary: "Query bounded durable Omen execution history.".into(),
            input_schema: object_schema(
                json!({
                    "all_sessions":{"type":"boolean","default":true},
                    "session_id":{"type":"string"},
                    "limit":{"type":"integer","minimum":1,"maximum":100,"default":20}
                }),
                &[],
            ),
            output_schema: history_schema(),
            effect_class: EffectClass::Read,
            network_effect: NetworkEffect::None,
            reversibility: Reversibility::NotApplicable,
            idempotent: true,
            authority: "durable Omen execution history".into(),
            bounds: "default 20 entries; maximum 100".into(),
            timeout: "bounded local database query".into(),
            examples: vec![json!({"limit":20})],
        },
        CapabilityDefinition {
            id: "execution.run".into(),
            group: "execution".into(),
            summary: "Run an explicitly supplied argv under an execution contract.".into(),
            input_schema: object_schema(
                json!({"argv":{"type":"array","minItems":1,"maxItems":64,"items":{"type":"string"}}}),
                &["argv"],
            ),
            output_schema: result.clone(),
            effect_class: EffectClass::SpawnProcess,
            network_effect: NetworkEffect::External,
            reversibility: Reversibility::Irreversible,
            idempotent: false,
            authority: "Tethers admission".into(),
            bounds: "bounded output and timeout".into(),
            timeout: "caller supplied timeout".into(),
            examples: vec![json!({"argv":["cargo","check"]})],
        },
        CapabilityDefinition {
            id: "mutation.threadmoth".into(),
            group: "mutation".into(),
            summary: "Apply a deterministic ThreadMoth mutation.".into(),
            input_schema: object_schema(json!({"plan":{"type":"string","minLength":1}}), &["plan"]),
            output_schema: result.clone(),
            effect_class: EffectClass::Mutate,
            network_effect: NetworkEffect::None,
            reversibility: Reversibility::Reversible,
            idempotent: false,
            authority: "Tethers admission".into(),
            bounds: "bounded diff and artifact evidence".into(),
            timeout: "bounded mutation timeout".into(),
            examples: vec![json!({"plan":"threadmoth://plan/example"})],
        },
        CapabilityDefinition {
            id: "composition.run".into(),
            group: "composition".into(),
            summary: "Execute one explicitly planned sequential action.".into(),
            input_schema: object_schema(
                json!({"action_id":{"type":"string","minLength":1},"expect_plan":{"type":"string","minLength":1}}),
                &["action_id", "expect_plan"],
            ),
            output_schema: object_schema(
                json!({"status":{"type":"string"},"steps":{"type":"array"}}),
                &["status", "steps"],
            ),
            effect_class: EffectClass::SpawnProcess,
            network_effect: NetworkEffect::External,
            reversibility: Reversibility::Irreversible,
            idempotent: false,
            authority: "current external execution contracts".into(),
            bounds: "bounded sequential action; no nested actions, retry or rollback".into(),
            timeout: "bounded by child capability timeouts".into(),
            examples: vec![json!({"action_id":"verify-core","expect_plan":"sha256:..."})],
        },
        CapabilityDefinition {
            id: "filesystem.read".into(),
            group: "filesystem".into(),
            summary: "Read bounded workspace data.".into(),
            input_schema: object_schema(json!({"path":{"type":"string","minLength":1}}), &["path"]),
            output_schema: result.clone(),
            effect_class: EffectClass::Read,
            network_effect: NetworkEffect::None,
            reversibility: Reversibility::NotApplicable,
            idempotent: true,
            authority: "none".into(),
            bounds: "bounded bytes or artifact reference".into(),
            timeout: "bounded filesystem read".into(),
            examples: vec![json!({"path":"src/lib.rs"})],
        },
        CapabilityDefinition {
            id: "filesystem.write".into(),
            group: "filesystem".into(),
            summary: "Write workspace data when separately admitted by Tethers.".into(),
            input_schema: object_schema(
                json!({"path":{"type":"string","minLength":1},"content":{"type":"string"}}),
                &["path", "content"],
            ),
            output_schema: result,
            effect_class: EffectClass::Mutate,
            network_effect: NetworkEffect::None,
            reversibility: Reversibility::Reversible,
            idempotent: false,
            authority: "Tethers admission".into(),
            bounds: "bounded content".into(),
            timeout: "bounded filesystem write".into(),
            examples: vec![json!({"path":"src/lib.rs","content":"..."})],
        },
        CapabilityDefinition {
            id: "composition.plan".into(),
            group: "composition".into(),
            summary: "Validate a named action and build a deterministic read-only plan.".into(),
            input_schema: object_schema(
                json!({"action_id":{"type":"string","minLength":1}}),
                &["action_id"],
            ),
            output_schema: plan_result,
            effect_class: EffectClass::Compute,
            network_effect: NetworkEffect::None,
            reversibility: Reversibility::NotApplicable,
            idempotent: true,
            authority: "none".into(),
            bounds: "bounded action and plan size".into(),
            timeout: "bounded local parse and validation".into(),
            examples: vec![json!({"action_id":"inspect-auth"})],
        },
    ];
    let recipe_definitions = vec![
        RecipeDefinition {
            id: "investigate-failure".into(),
            summary: "Inspect a failure using bounded evidence before interpreting it.".into(),
            steps: vec![
                RecipeStep {
                    capability_id: "filesystem.read".into(),
                    purpose: "inspect @failed or its bounded artifact reference".into(),
                },
                RecipeStep {
                    capability_id: "semantic.diagnostics".into(),
                    purpose: "resolve deterministic diagnostics when available".into(),
                },
                RecipeStep {
                    capability_id: "semantic.definition".into(),
                    purpose: "resolve relevant symbols only when needed".into(),
                },
            ],
            advisory: true,
            notes: vec!["Read large artifacts only through bounded slices or references.".into()],
        },
        RecipeDefinition {
            id: "safe-mutation".into(),
            summary: "Discover, preview, re-admit, mutate, and verify deterministically.".into(),
            steps: vec![
                RecipeStep {
                    capability_id: "structure.search".into(),
                    purpose: "discover the target structurally".into(),
                },
                RecipeStep {
                    capability_id: "mutation.threadmoth".into(),
                    purpose: "apply only an explicitly previewed and admitted mutation plan".into(),
                },
                RecipeStep {
                    capability_id: "execution.run".into(),
                    purpose: "run relevant verification after current authority is re-checked"
                        .into(),
                },
            ],
            advisory: true,
            notes: vec![
                "Preview and Tethers admission are required gates; this recipe grants neither."
                    .into(),
            ],
        },
    ];
    MachineContract {
        contract_version: CONTRACT_VERSION.into(),
        capability_definitions,
        recipe_definitions,
    }
}

pub fn unknown_context() -> MachineContext {
    let capability_statuses = contract()
        .capability_definitions
        .into_iter()
        .map(|definition| CapabilityStatus {
            id: definition.id,
            availability: AvailabilityState::Unknown,
            admission: AdmissionState::Unknown,
            provider: "not-probed".into(),
            assurance: "UNKNOWN".into(),
            reason: Some("runtime status not probed during discovery".into()),
        })
        .collect();
    MachineContext {
        context_generation: None,
        generation_status: "unavailable".into(),
        capability_statuses,
    }
}

pub fn context_with_generation(generation: Option<i64>) -> MachineContext {
    let mut context = unknown_context();
    context.context_generation = generation;
    context.generation_status = if generation.is_some() {
        "known".into()
    } else {
        "unavailable".into()
    };
    context
}

pub fn project(contract: &MachineContract, context: &MachineContext) -> Vec<CapabilityProjection> {
    contract
        .capability_definitions
        .iter()
        .filter_map(|definition| {
            context
                .capability_statuses
                .iter()
                .find(|status| status.id == definition.id)
                .map(|status| CapabilityProjection {
                    definition: definition.clone(),
                    status: status.clone(),
                })
        })
        .collect()
}

pub fn canonical_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("machine contract is serializable")
}
pub fn contract_digest_of(contract: &MachineContract) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical_json(contract).as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}
pub fn contract_digest() -> String {
    contract_digest_of(&contract())
}
pub fn capability(id: &str) -> Option<CapabilityDefinition> {
    contract()
        .capability_definitions
        .into_iter()
        .find(|definition| definition.id == id)
}
pub fn recipe(name: &str) -> Option<RecipeDefinition> {
    contract()
        .recipe_definitions
        .into_iter()
        .find(|recipe| recipe.id == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_digest_ignores_runtime_status() {
        let digest = contract_digest();
        let mut context = unknown_context();
        context.capability_statuses[0].availability = AvailabilityState::Available;
        context.capability_statuses[0].admission = AdmissionState::Admitted;
        assert_eq!(digest, contract_digest());
        assert_eq!(
            project(&contract(), &context)[0].status.availability,
            AvailabilityState::Available
        );
    }

    #[test]
    fn static_digest_changes_when_definition_changes() {
        let mut changed = contract();
        changed.capability_definitions[0]
            .summary
            .push_str(" (changed)");
        assert_ne!(contract_digest(), contract_digest_of(&changed));
    }

    #[test]
    fn definitions_are_unique_bounded_and_typed() {
        let c = contract();
        let mut ids = std::collections::HashSet::new();
        for definition in &c.capability_definitions {
            assert!(ids.insert(&definition.id));
            assert!(!definition.group.is_empty());
            assert!(!definition.examples.is_empty() && definition.examples.len() <= 2);
            assert!(
                definition.input_schema["$schema"]
                    .as_str()
                    .unwrap()
                    .contains("2020-12")
            );
        }
    }

    #[test]
    fn every_recipe_step_resolves() {
        let c = contract();
        for recipe in &c.recipe_definitions {
            for step in &recipe.steps {
                assert!(
                    c.capability_definitions
                        .iter()
                        .any(|d| d.id == step.capability_id)
                );
            }
        }
    }
}
