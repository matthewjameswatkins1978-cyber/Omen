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
    /// Static deterministic routing: which product surfaces can invoke this
    /// capability. `None` means the surface cannot invoke it directly
    /// (it may still be reachable as a composition action step).
    pub invocation: Invocation,
}

/// Static invocation routing for one capability.
///
/// This is contract metadata, not runtime probing: a client must be able to
/// determine how to invoke a described capability without guessing command
/// syntax and without parsing prose.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Invocation {
    /// CLI invocation, e.g. `"exec -- ..."`. `None` when the CLI surface
    /// cannot invoke this capability directly.
    pub cli: Option<String>,
    /// MCP tool name, e.g. `"omen_execute"`. `None` when no MCP tool
    /// invokes this capability directly.
    pub mcp_tool: Option<String>,
    /// Interactive shell verb, e.g. `":history"`. `None` when the
    /// interactive shell has no direct verb (execution.run is invoked by
    /// typing the command itself, not by a verb).
    pub interactive: Option<String>,
}

impl Invocation {
    fn new(cli: Option<&str>, mcp_tool: Option<&str>, interactive: Option<&str>) -> Self {
        Self {
            cli: cli.map(str::to_string),
            mcp_tool: mcp_tool.map(str::to_string),
            interactive: interactive.map(str::to_string),
        }
    }
}

/// Static deterministic routing table: capability id -> supported surfaces.
///
/// CLI spellings are the canonical `omen` subcommand fragments; MCP names are
/// the `omen mcp` tool names; interactive names are the REPL verbs. Entries
/// with all three surfaces `None` are reachable only as composition action
/// steps (see `composition.plan` / `composition.run`).
pub fn invocation_for(capability_id: &str) -> Invocation {
    match capability_id {
        "semantic.references" => {
            Invocation::new(None, Some("omen_symbol_references"), Some(":refs"))
        }
        "semantic.definition" => {
            Invocation::new(None, Some("omen_symbol_definition"), Some(":def"))
        }
        "semantic.diagnostics" => Invocation::new(None, None, None),
        "structure.search" => {
            Invocation::new(None, Some("omen_structure_search"), Some(":structure"))
        }
        "history.query" => Invocation::new(
            Some("history --machine"),
            Some("omen_history_query"),
            Some(":history"),
        ),
        "execution.run" => {
            Invocation::new(Some("exec --machine -- <argv>"), Some("omen_execute"), None)
        }
        "execution.cancel" => Invocation::new(
            Some("cancel <execution-id>"),
            Some("omen_cancel_execution"),
            None,
        ),
        "mutation.threadmoth" => Invocation::new(None, None, None),
        "composition.run" => Invocation::new(Some("action run"), None, None),
        "filesystem.read" => Invocation::new(None, None, None),
        "filesystem.write" => Invocation::new(None, None, None),
        "composition.plan" => {
            Invocation::new(Some("action plan"), Some("omen_action_plan"), Some(":plan"))
        }
        _ => Invocation::new(None, None, None),
    }
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

/// The compact static-plus-runtime entry returned by broad discovery.
/// Detailed schemas remain behind `describe_ref`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityCatalogueEntry {
    pub id: String,
    pub group: String,
    pub summary: String,
    pub availability: AvailabilityState,
    pub admission: AdmissionState,
    pub provider: String,
    pub reason: Option<String>,
    /// The stable capability id accepted by `describe` / `omen_describe`.
    pub describe_ref: String,
    pub related_capabilities: Vec<String>,
    /// Static invocation routing (mirrors the capability definition).
    pub invocation: Invocation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecipeStep {
    pub capability_id: String,
    pub purpose: String,
    /// Static invocation routing for the step's capability, so callers on any
    /// surface can resolve how to perform the step without guessing syntax.
    pub invocation: Invocation,
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
            invocation: invocation_for("semantic.references"),
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
            invocation: invocation_for("semantic.definition"),
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
            invocation: invocation_for("semantic.diagnostics"),
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
            invocation: invocation_for("structure.search"),
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
            invocation: invocation_for("history.query"),
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
            invocation: invocation_for("execution.run"),
            group: "execution".into(),
            summary: "Run an explicitly supplied argv under an execution contract; Omen mints the physical execution_id.".into(),
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
            examples: vec![json!({"argv":["cargo","check"],"result_identity":"Omen-generated execution_id; upstream references remain external_reference"})],
        },
        CapabilityDefinition {
            id: "execution.cancel".into(),
            invocation: invocation_for("execution.cancel"),
            group: "execution".into(),
            summary: "Request cancellation of a live brokered execution by canonical execution_id. Intent and proof are distinct: only observed physical death reports TerminationConfirmed; a stop that arrives before dispatch reports DispatchPrevented (no spawn, no tree-stop, no death claimed); unconfirmed stops report OutcomeUnknown; finished executions report AlreadyFinished without rewriting history.".into(),
            input_schema: object_schema(
                json!({"execution_id":{"type":"string","minLength":1}}),
                &["execution_id"],
            ),
            output_schema: result.clone(),
            effect_class: EffectClass::Mutate,
            network_effect: NetworkEffect::None,
            reversibility: Reversibility::Irreversible,
            idempotent: true,
            authority: "none".into(),
            bounds: "one bounded cancel request; bounded terminal observation".into(),
            timeout: "bounded stop grace plus bounded observation".into(),
            examples: vec![json!({"execution_id":"exec_01K9F82A"})],
        },
        CapabilityDefinition {
            id: "mutation.threadmoth".into(),
            invocation: invocation_for("mutation.threadmoth"),
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
            invocation: invocation_for("composition.run"),
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
            invocation: invocation_for("filesystem.read"),
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
            invocation: invocation_for("filesystem.write"),
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
            invocation: invocation_for("composition.plan"),
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
    let mut recipe_definitions = vec![
        RecipeDefinition {
            id: "investigate-failure".into(),
            summary: "Inspect a failure using bounded evidence before interpreting it.".into(),
            steps: vec![
                RecipeStep {
                    capability_id: "filesystem.read".into(),
                    invocation: invocation_for("filesystem.read"),
                    purpose: "inspect @failed or its bounded artifact reference".into(),
                },
                RecipeStep {
                    capability_id: "semantic.diagnostics".into(),
                    invocation: invocation_for("semantic.diagnostics"),
                    purpose: "resolve deterministic diagnostics when available".into(),
                },
                RecipeStep {
                    capability_id: "semantic.definition".into(),
                    invocation: invocation_for("semantic.definition"),
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
                    invocation: invocation_for("structure.search"),
                    purpose: "discover the target structurally".into(),
                },
                RecipeStep {
                    capability_id: "mutation.threadmoth".into(),
                    invocation: invocation_for("mutation.threadmoth"),
                    purpose: "apply only an explicitly previewed and admitted mutation plan".into(),
                },
                RecipeStep {
                    capability_id: "execution.run".into(),
                    invocation: invocation_for("execution.run"),
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
    recipe_definitions.extend([
        RecipeDefinition {
            id: "find-symbol".into(),
            summary: "Search, resolve, and inspect references for a known symbol.".into(),
            steps: vec![
                RecipeStep {
                    capability_id: "semantic.definition".into(),
                    invocation: invocation_for("semantic.definition"),
                    purpose: "resolve the exact symbol definition".into(),
                },
                RecipeStep {
                    capability_id: "semantic.references".into(),
                    invocation: invocation_for("semantic.references"),
                    purpose: "retrieve bounded references after identity is known".into(),
                },
            ],
            advisory: true,
            notes: vec!["Provider availability is reported in each capability projection.".into()],
        },
        RecipeDefinition {
            id: "execute-and-inspect".into(),
            summary: "Execute a bounded command, then inspect its recorded evidence.".into(),
            steps: vec![
                RecipeStep {
                    capability_id: "execution.run".into(),
                    invocation: invocation_for("execution.run"),
                    purpose: "run the explicitly supplied argv under current authority".into(),
                },
                RecipeStep {
                    capability_id: "history.query".into(),
                    invocation: invocation_for("history.query"),
                    purpose: "retrieve the durable execution record and artifact references".into(),
                },
            ],
            advisory: true,
            notes: vec![
                "Execution requires current external authority; the recipe grants none.".into(),
                "Standalone local execution is UNJOURNALED_LOCAL_EXECUTION: durable history entries require daemon-brokered execution; local evidence is preserved as artifacts.".into(),
            ],
        },
        RecipeDefinition {
            id: "inspect-history".into(),
            summary: "Query bounded durable history and follow recorded evidence.".into(),
            steps: vec![RecipeStep {
                capability_id: "history.query".into(),
                invocation: invocation_for("history.query"),
                purpose: "query the current or session-scoped execution history".into(),
            }],
            advisory: true,
            notes: vec![
                "History is Omen evidence, not a complete OS audit log.".into(),
                "Direct local executions are reported as UNJOURNALED_LOCAL_EXECUTION rather than durable entries.".into(),
            ],
        },
        RecipeDefinition {
            id: "plan-action".into(),
            summary: "Discover an action, inspect it, and build a deterministic read-only plan."
                .into(),
            steps: vec![
                RecipeStep {
                    capability_id: "composition.plan".into(),
                    invocation: invocation_for("composition.plan"),
                    purpose: "validate the named action and compute its plan digest".into(),
                },
                RecipeStep {
                    capability_id: "composition.run".into(),
                    invocation: invocation_for("composition.run"),
                    purpose: "execute only after the plan and current authority are rechecked"
                        .into(),
                },
            ],
            advisory: true,
            notes: vec![
                "Planning is read-only; execution is a separate consequential operation.".into(),
            ],
        },
    ]);
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

pub fn catalogue(
    contract: &MachineContract,
    context: &MachineContext,
) -> Vec<CapabilityCatalogueEntry> {
    contract
        .capability_definitions
        .iter()
        .filter_map(|definition| {
            let status = context
                .capability_statuses
                .iter()
                .find(|status| status.id == definition.id)?;
            let related_capabilities = contract
                .capability_definitions
                .iter()
                .filter(|other| other.group == definition.group && other.id != definition.id)
                .map(|other| other.id.clone())
                .collect();
            Some(CapabilityCatalogueEntry {
                id: definition.id.clone(),
                group: definition.group.clone(),
                summary: definition.summary.clone(),
                availability: status.availability,
                admission: status.admission,
                provider: status.provider.clone(),
                reason: status.reason.clone(),
                describe_ref: definition.id.clone(),
                related_capabilities,
                invocation: definition.invocation.clone(),
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

    #[test]
    fn every_capability_carries_deterministic_invocation_routing() {
        let c = contract();
        for definition in &c.capability_definitions {
            // Static table must agree with the embedded routing.
            assert_eq!(
                definition.invocation,
                invocation_for(&definition.id),
                "routing drift for {}",
                definition.id
            );
        }
        for recipe in &c.recipe_definitions {
            for step in &recipe.steps {
                assert_eq!(
                    step.invocation,
                    invocation_for(&step.capability_id),
                    "step routing drift for {}",
                    step.capability_id
                );
            }
        }
        // Representative review-visible routings.
        let execution = invocation_for("execution.run");
        assert_eq!(execution.cli.as_deref(), Some("exec --machine -- <argv>"));
        assert_eq!(execution.mcp_tool.as_deref(), Some("omen_execute"));
        let history = invocation_for("history.query");
        assert_eq!(history.cli.as_deref(), Some("history --machine"));
        assert_eq!(history.mcp_tool.as_deref(), Some("omen_history_query"));
        assert_eq!(history.interactive.as_deref(), Some(":history"));
        let definition = invocation_for("semantic.definition");
        assert_eq!(definition.cli, None);
        assert_eq!(
            definition.mcp_tool.as_deref(),
            Some("omen_symbol_definition")
        );
        assert_eq!(definition.interactive.as_deref(), Some(":def"));
        // Composition-only capabilities advertise no direct surface rather
        // than a guessed command string.
        let read = invocation_for("filesystem.read");
        assert_eq!(read.cli, None);
        assert_eq!(read.mcp_tool, None);
        assert_eq!(read.interactive, None);
        // Unknown ids never invent a route.
        let unknown = invocation_for("no.such.capability");
        assert_eq!(unknown.cli, None);
        assert_eq!(unknown.mcp_tool, None);
        assert_eq!(unknown.interactive, None);
    }
}
