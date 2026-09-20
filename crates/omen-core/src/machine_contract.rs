//! Canonical, bounded machine-facing Omen contract.
//!
//! This module deliberately contains static contract truth only.  Workspace and
//! provider state belongs to the runtime overlay in the CLI/daemon layers.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const CONTRACT_VERSION: &str = "0.8";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityDefinition {
    pub id: String,
    pub group: String,
    pub summary: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub effect: String,
    pub read_only: bool,
    pub idempotent: bool,
    pub reversible: bool,
    pub network: bool,
    pub authority: String,
    pub bounds: String,
    pub timeout: String,
    pub examples: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityStatus {
    pub id: String,
    pub available: bool,
    pub admitted: bool,
    pub provider: String,
    pub assurance: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityEntry {
    pub definition: CapabilityDefinition,
    pub status: CapabilityStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineContract {
    pub contract_version: String,
    pub capabilities: Vec<CapabilityEntry>,
    pub recipes: Vec<String>,
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn make_capability(
    id: &str,
    group: &str,
    summary: &str,
    input_schema: Value,
    output_schema: Value,
    effect: &str,
    read_only: bool,
    idempotent: bool,
    reversible: bool,
    network: bool,
    authority: &str,
    bounds: &str,
    timeout: &str,
    examples: Vec<Value>,
    available: bool,
    admitted: bool,
    provider: &str,
    assurance: &str,
    reason: Option<&str>,
) -> CapabilityEntry {
    CapabilityEntry {
        definition: CapabilityDefinition {
            id: id.into(),
            group: group.into(),
            summary: summary.into(),
            input_schema,
            output_schema,
            effect: effect.into(),
            read_only,
            idempotent,
            reversible,
            network,
            authority: authority.into(),
            bounds: bounds.into(),
            timeout: timeout.into(),
            examples,
        },
        status: CapabilityStatus {
            id: id.into(),
            available,
            admitted,
            provider: provider.into(),
            assurance: assurance.into(),
            reason: reason.map(str::to_owned),
        },
    }
}

pub fn contract() -> MachineContract {
    let empty = object_schema(json!({}), &[]);
    let result = object_schema(
        json!({
            "status": {"type": "string"},
            "results": {"type": "array", "maxItems": 100},
            "references": {"type": "array", "maxItems": 16}
        }),
        &["status"],
    );
    MachineContract {
        contract_version: CONTRACT_VERSION.into(),
        capabilities: vec![
            make_capability(
                "semantic.references",
                "semantic",
                "Find references to a known symbol.",
                object_schema(
                    json!({"symbol": {"type": "string", "minLength": 1}}),
                    &["symbol"],
                ),
                result.clone(),
                "read workspace semantic index",
                true,
                true,
                true,
                false,
                "none",
                "max 100 results",
                "bounded provider timeout",
                vec![json!({"symbol": "refresh_token"})],
                true,
                true,
                "semantic-registry",
                "DETERMINISTIC",
                None,
            ),
            make_capability(
                "semantic.definition",
                "semantic",
                "Resolve the definition of a known symbol.",
                object_schema(
                    json!({"symbol": {"type": "string", "minLength": 1}}),
                    &["symbol"],
                ),
                result.clone(),
                "read workspace semantic index",
                true,
                true,
                true,
                false,
                "none",
                "one bounded result",
                "bounded provider timeout",
                vec![json!({"symbol": "refresh_token"})],
                true,
                true,
                "semantic-registry",
                "DETERMINISTIC",
                None,
            ),
            make_capability(
                "semantic.diagnostics",
                "semantic",
                "Return deterministic workspace diagnostics when a provider is available.",
                empty.clone(),
                result.clone(),
                "read workspace diagnostics",
                true,
                true,
                true,
                false,
                "none",
                "bounded diagnostics",
                "bounded provider timeout",
                vec![json!({})],
                false,
                false,
                "semantic-provider",
                "UNKNOWN",
                Some("provider not probed"),
            ),
            make_capability(
                "structure.search",
                "structure",
                "Search source using the structural adapter.",
                object_schema(
                    json!({"pattern": {"type": "string", "minLength": 1}}),
                    &["pattern"],
                ),
                result.clone(),
                "read workspace source",
                true,
                true,
                true,
                false,
                "none",
                "bounded matches",
                "bounded adapter timeout",
                vec![json!({"pattern": "fn $NAME($$$ARGS) { $$$BODY }"})],
                true,
                true,
                "ast-grep",
                "OBSERVED",
                None,
            ),
            make_capability(
                "execution.run",
                "execution",
                "Run an explicitly supplied argv under an execution contract.",
                object_schema(
                    json!({"argv": {"type": "array", "minItems": 1, "maxItems": 64, "items": {"type": "string"}}}),
                    &["argv"],
                ),
                result.clone(),
                "spawn process",
                false,
                false,
                false,
                false,
                "Tethers admission",
                "bounded output and timeout",
                "caller supplied timeout",
                vec![json!({"argv": ["cargo", "check"]})],
                true,
                false,
                "execution-engine",
                "OBSERVED",
                Some("Tethers capability not admitted"),
            ),
            make_capability(
                "mutation.threadmoth",
                "mutation",
                "Apply a deterministic ThreadMoth mutation.",
                object_schema(
                    json!({"plan": {"type": "string", "minLength": 1}}),
                    &["plan"],
                ),
                result.clone(),
                "mutate workspace",
                false,
                false,
                true,
                false,
                "Tethers admission",
                "bounded diff and artifact evidence",
                "bounded mutation timeout",
                vec![json!({"plan": "threadmoth://plan/example"})],
                true,
                false,
                "threadmoth",
                "OBSERVED",
                Some("Tethers capability not admitted"),
            ),
            make_capability(
                "filesystem.read",
                "filesystem",
                "Read bounded workspace data.",
                object_schema(
                    json!({"path": {"type": "string", "minLength": 1}}),
                    &["path"],
                ),
                result.clone(),
                "read filesystem",
                true,
                true,
                true,
                false,
                "none",
                "bounded bytes or artifact reference",
                "bounded filesystem read",
                vec![json!({"path": "src/lib.rs"})],
                true,
                true,
                "filesystem",
                "OBSERVED",
                None,
            ),
            make_capability(
                "filesystem.write",
                "filesystem",
                "Write workspace data when separately admitted by Tethers.",
                object_schema(
                    json!({"path": {"type": "string", "minLength": 1}, "content": {"type": "string"}}),
                    &["path", "content"],
                ),
                result,
                "write filesystem",
                false,
                false,
                true,
                false,
                "Tethers admission",
                "bounded content",
                "bounded filesystem write",
                vec![json!({"path": "src/lib.rs", "content": "..."})],
                true,
                false,
                "filesystem",
                "UNKNOWN",
                Some("Tethers capability not admitted"),
            ),
        ],
        recipes: vec!["investigate-failure".into(), "safe-mutation".into()],
    }
}

pub fn canonical_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("machine contract is serializable")
}

pub fn contract_digest() -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical_json(&contract()).as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

pub fn capability(id: &str) -> Option<CapabilityEntry> {
    contract()
        .capabilities
        .into_iter()
        .find(|entry| entry.definition.id == id)
}

pub fn recipe(name: &str) -> Option<Value> {
    match name {
        "investigate-failure" => Some(json!({
            "name": name,
            "steps": ["filesystem.read", "semantic.diagnostics", "semantic.definition"],
            "advisory": true
        })),
        "safe-mutation" => Some(json!({
            "name": name,
            "steps": ["structure.search", "mutation.threadmoth", "execution.run"],
            "advisory": true
        })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_stable_and_runtime_independent() {
        assert_eq!(contract_digest(), contract_digest());
        assert!(contract_digest().starts_with("sha256:"));
    }

    #[test]
    fn every_recipe_references_a_capability() {
        let c = contract();
        for recipe_name in c.recipes {
            let recipe = recipe(&recipe_name).expect("built-in recipe exists");
            for step in recipe["steps"].as_array().unwrap() {
                let step_id = step.as_str().expect("recipe step is a string");
                assert!(
                    c.capabilities
                        .iter()
                        .any(|entry| entry.definition.id == step_id)
                );
            }
        }
    }
}
