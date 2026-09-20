//! Pure, deterministic Omen.toml composition model and planning.
//!
//! Parsing and filesystem access belong to the CLI edge. This module accepts
//! an already parsed configuration and snapshots of the static contract and
//! runtime context; planning never probes, spawns, mutates, or asks a model.

use crate::machine_contract::{
    AdmissionState, CapabilityDefinition, CapabilityStatus, EffectClass, MachineContext,
    MachineContract, NetworkEffect, Reversibility,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_ACTIONS: usize = 128;
pub const MAX_STEPS_PER_ACTION: usize = 64;
pub const MAX_INPUT_BINDINGS_PER_STEP: usize = 64;
pub const MAX_STRING_LENGTH: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OmenWorkspaceConfig {
    pub schema_version: u32,
    pub project: Option<ProjectConfig>,
    #[serde(default)]
    pub actions: BTreeMap<String, NamedActionDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NamedActionDefinition {
    pub description: Option<String>,
    pub steps: Vec<ActionStepDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActionStepDefinition {
    pub id: String,
    pub capability: String,
    #[serde(default)]
    pub input: BTreeMap<String, InputBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum InputBinding {
    Literal { value: Value },
    StepOutput { step: String, field: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionPlan {
    pub action_id: String,
    pub description: Option<String>,
    pub config_schema_version: u32,
    pub contract_digest: String,
    pub context_generation: Option<i64>,
    pub steps: Vec<ActionPlanStep>,
    pub effects: PlanEffectSummary,
    pub planning_valid: bool,
    pub runtime_status: PlanRuntimeStatus,
    pub admission_snapshot_only: bool,
    pub plan_digest: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionPlanStep {
    pub step_id: String,
    pub capability_id: String,
    pub order: usize,
    pub inputs: BTreeMap<String, PlannedInput>,
    pub effect_class: EffectClass,
    pub network_effect: NetworkEffect,
    pub reversibility: Reversibility,
    pub availability: crate::machine_contract::AvailabilityState,
    pub admission: AdmissionState,
    pub authority: String,
    pub bounds: String,
    pub timeout: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlannedInput {
    Literal { value: Value },
    StepOutput { step: String, field: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanEffectSummary {
    pub effect_classes: Vec<EffectClass>,
    pub network_effect: NetworkEffect,
    pub mutating_steps: Vec<String>,
    pub authority_requirements: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanRuntimeStatus {
    pub availability: String,
    pub admission: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanValidationError {
    pub code: String,
    pub message: String,
    pub step_id: Option<String>,
    pub field: Option<String>,
}

impl std::fmt::Display for PlanValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for PlanValidationError {}

fn error(
    code: &str,
    message: impl Into<String>,
    step_id: Option<&str>,
    field: Option<&str>,
) -> PlanValidationError {
    PlanValidationError {
        code: code.into(),
        message: message.into(),
        step_id: step_id.map(str::to_owned),
        field: field.map(str::to_owned),
    }
}

pub fn validate_config(config: &OmenWorkspaceConfig) -> Result<(), PlanValidationError> {
    if config.schema_version != 1 {
        return Err(error(
            "OMEN_CONFIG_VERSION_UNSUPPORTED",
            format!("schema_version {} is not supported", config.schema_version),
            None,
            None,
        ));
    }
    if config.actions.len() > MAX_ACTIONS {
        return Err(error(
            "OMEN_CONFIG_TOO_MANY_ACTIONS",
            "action count exceeds bound",
            None,
            None,
        ));
    }
    if let Some(project) = &config.project
        && project
            .name
            .as_ref()
            .is_some_and(|s| s.len() > MAX_STRING_LENGTH)
    {
        return Err(error(
            "OMEN_CONFIG_BOUNDS",
            "project name exceeds bound",
            None,
            None,
        ));
    }
    for (action_id, action) in &config.actions {
        validate_name(action_id, "INVALID_ACTION_ID")?;
        if action.steps.len() > MAX_STEPS_PER_ACTION {
            return Err(error(
                "OMEN_CONFIG_TOO_MANY_STEPS",
                "step count exceeds bound",
                None,
                None,
            ));
        }
        if action
            .description
            .as_ref()
            .is_some_and(|s| s.len() > MAX_STRING_LENGTH)
        {
            return Err(error(
                "OMEN_CONFIG_BOUNDS",
                "action description exceeds bound",
                None,
                None,
            ));
        }
        let mut ids = BTreeSet::new();
        for step in &action.steps {
            validate_name(&step.id, "INVALID_STEP_ID")?;
            if !ids.insert(&step.id) {
                return Err(error(
                    "DUPLICATE_STEP_ID",
                    format!("duplicate step id '{}'", step.id),
                    Some(&step.id),
                    None,
                ));
            }
            if step.capability.len() > MAX_STRING_LENGTH {
                return Err(error(
                    "OMEN_CONFIG_BOUNDS",
                    "capability id exceeds bound",
                    Some(&step.id),
                    None,
                ));
            }
            if step.input.len() > MAX_INPUT_BINDINGS_PER_STEP {
                return Err(error(
                    "OMEN_CONFIG_BOUNDS",
                    "input binding count exceeds bound",
                    Some(&step.id),
                    None,
                ));
            }
            for (field, binding) in &step.input {
                if field.len() > MAX_STRING_LENGTH {
                    return Err(error(
                        "OMEN_CONFIG_BOUNDS",
                        "input field exceeds bound",
                        Some(&step.id),
                        Some(field),
                    ));
                }
                if let InputBinding::StepOutput {
                    step: source,
                    field: source_field,
                } = binding
                {
                    if source == &step.id {
                        return Err(error(
                            "SELF_STEP_REFERENCE",
                            "a step cannot consume its own output",
                            Some(&step.id),
                            Some(field),
                        ));
                    }
                    if source_field.len() > MAX_STRING_LENGTH {
                        return Err(error(
                            "OMEN_CONFIG_BOUNDS",
                            "source field exceeds bound",
                            Some(&step.id),
                            Some(field),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_name(name: &str, code: &str) -> Result<(), PlanValidationError> {
    if name.is_empty()
        || name.len() > 128
        || !name.bytes().enumerate().all(|(i, b)| {
            if i == 0 {
                b.is_ascii_lowercase()
            } else {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
            }
        })
    {
        return Err(error(
            code,
            format!("invalid identifier '{name}'"),
            None,
            None,
        ));
    }
    Ok(())
}

pub fn plan_action(
    config: &OmenWorkspaceConfig,
    action_id: &str,
    contract: &MachineContract,
    context: &MachineContext,
) -> Result<ActionPlan, PlanValidationError> {
    validate_config(config)?;
    let action = config.actions.get(action_id).ok_or_else(|| {
        error(
            "ACTION_NOT_FOUND",
            format!("action '{action_id}' does not exist"),
            None,
            None,
        )
    })?;
    let definitions: BTreeMap<&str, &CapabilityDefinition> = contract
        .capability_definitions
        .iter()
        .map(|d| (d.id.as_str(), d))
        .collect();
    let statuses: BTreeMap<&str, &CapabilityStatus> = context
        .capability_statuses
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    let mut prior_steps = BTreeMap::<String, &CapabilityDefinition>::new();
    let mut plan_steps = Vec::with_capacity(action.steps.len());
    let mut effect_classes = BTreeSet::new();
    let mut network = NetworkEffect::None;
    let mut mutating_steps = Vec::new();
    let mut authorities = BTreeSet::new();
    let mut warnings = Vec::new();

    for (order, step) in action.steps.iter().enumerate() {
        let definition = definitions.get(step.capability.as_str()).ok_or_else(|| {
            error(
                "UNKNOWN_CAPABILITY",
                format!(
                    "capability '{}' is not in the Machine Contract",
                    step.capability
                ),
                Some(&step.id),
                None,
            )
        })?;
        let status = statuses.get(step.capability.as_str()).ok_or_else(|| {
            error(
                "RUNTIME_STATUS_UNAVAILABLE",
                "capability has no context status",
                Some(&step.id),
                None,
            )
        })?;
        let mut inputs = BTreeMap::new();
        for (target_field, binding) in &step.input {
            let target_type = schema_field_type(&definition.input_schema, target_field)
                .ok_or_else(|| {
                    error(
                        "INPUT_FIELD_NOT_FOUND",
                        format!("capability input has no field '{target_field}'"),
                        Some(&step.id),
                        Some(target_field),
                    )
                })?;
            let planned = match binding {
                InputBinding::Literal { value } => {
                    if !value_matches_type(value, target_type) {
                        return Err(error(
                            "TYPE_MISMATCH",
                            format!("literal for '{target_field}' is not {target_type}"),
                            Some(&step.id),
                            Some(target_field),
                        ));
                    }
                    PlannedInput::Literal {
                        value: value.clone(),
                    }
                }
                InputBinding::StepOutput {
                    step: source,
                    field,
                } => {
                    let source_definition = prior_steps.get(source).ok_or_else(|| {
                        if action.steps.iter().any(|candidate| candidate.id == *source) {
                            error(
                                "FORWARD_STEP_REFERENCE",
                                format!("step '{source}' is not prior to '{}'", step.id),
                                Some(&step.id),
                                Some(target_field),
                            )
                        } else {
                            error(
                                "STEP_REFERENCE_NOT_FOUND",
                                format!("source step '{source}' does not exist"),
                                Some(&step.id),
                                Some(target_field),
                            )
                        }
                    })?;
                    let source_type = schema_field_type(&source_definition.output_schema, field)
                        .ok_or_else(|| {
                            error(
                                "OUTPUT_FIELD_NOT_FOUND",
                                format!("capability output has no field '{field}'"),
                                Some(&step.id),
                                Some(target_field),
                            )
                        })?;
                    if source_type != target_type {
                        return Err(error(
                            "TYPE_MISMATCH",
                            format!(
                                "source '{source}.{field}' ({source_type}) cannot feed '{target_field}' ({target_type})"
                            ),
                            Some(&step.id),
                            Some(target_field),
                        ));
                    }
                    PlannedInput::StepOutput {
                        step: source.clone(),
                        field: field.clone(),
                    }
                }
            };
            inputs.insert(target_field.clone(), planned);
        }
        effect_classes.insert(definition.effect_class);
        if definition.network_effect == NetworkEffect::External {
            network = NetworkEffect::External;
        }
        if definition.effect_class == EffectClass::Mutate {
            mutating_steps.push(step.id.clone());
        }
        if definition.authority != "none" {
            authorities.insert(definition.authority.clone());
        }
        if matches!(
            status.availability,
            crate::machine_contract::AvailabilityState::Unknown
        ) {
            warnings.push(format!("runtime availability for '{}' is unknown", step.id));
        }
        if matches!(status.admission, AdmissionState::Unknown) {
            warnings.push(format!(
                "runtime admission for '{}' is unknown and is not permission",
                step.id
            ));
        }
        prior_steps.insert(step.id.clone(), definition);
        plan_steps.push(ActionPlanStep {
            step_id: step.id.clone(),
            capability_id: step.capability.clone(),
            order,
            inputs,
            effect_class: definition.effect_class,
            network_effect: definition.network_effect,
            reversibility: definition.reversibility,
            availability: status.availability,
            admission: status.admission,
            authority: definition.authority.clone(),
            bounds: definition.bounds.clone(),
            timeout: definition.timeout.clone(),
        });
    }
    let mut plan = ActionPlan {
        action_id: action_id.into(),
        description: action.description.clone(),
        config_schema_version: config.schema_version,
        contract_digest: crate::machine_contract::contract_digest_of(contract),
        context_generation: context.context_generation,
        steps: plan_steps,
        effects: PlanEffectSummary {
            effect_classes: effect_classes.into_iter().collect(),
            network_effect: network,
            mutating_steps,
            authority_requirements: authorities.into_iter().collect(),
        },
        planning_valid: true,
        runtime_status: PlanRuntimeStatus {
            availability: "unknown".into(),
            admission: "unknown".into(),
        },
        admission_snapshot_only: true,
        plan_digest: String::new(),
        warnings,
    };
    plan.plan_digest = plan_digest(&plan);
    Ok(plan)
}

fn plan_digest(plan: &ActionPlan) -> String {
    let mut semantic = plan.clone();
    semantic.plan_digest.clear();
    let mut hasher = Sha256::new();
    hasher.update(
        serde_json::to_string(&semantic)
            .expect("plan serializes")
            .as_bytes(),
    );
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn schema_field_type<'a>(schema: &'a Value, field: &str) -> Option<&'a str> {
    schema.get("properties")?.get(field)?.get("type")?.as_str()
}
fn value_matches_type(value: &Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine_contract;

    fn config(input: InputBinding) -> OmenWorkspaceConfig {
        OmenWorkspaceConfig {
            schema_version: 1,
            project: None,
            actions: [(
                "inspect".into(),
                NamedActionDefinition {
                    description: Some("inspect".into()),
                    steps: vec![ActionStepDefinition {
                        id: "definition".into(),
                        capability: "semantic.definition".into(),
                        input: [("symbol".into(), input)].into(),
                    }],
                },
            )]
            .into_iter()
            .collect(),
        }
    }

    #[test]
    fn literal_planning_is_deterministic() {
        let c = config(InputBinding::Literal {
            value: Value::String("SessionToken".into()),
        });
        let contract = machine_contract::contract();
        let context = machine_contract::unknown_context();
        let a = plan_action(&c, "inspect", &contract, &context).unwrap();
        let b = plan_action(&c, "inspect", &contract, &context).unwrap();
        assert_eq!(a.plan_digest, b.plan_digest);
        assert_eq!(a.steps[0].inputs.len(), 1);
    }

    #[test]
    fn literal_type_mismatch_refuses() {
        let c = config(InputBinding::Literal {
            value: Value::Array(vec![Value::String("wrong".into())]),
        });
        let error = plan_action(
            &c,
            "inspect",
            &machine_contract::contract(),
            &machine_contract::unknown_context(),
        )
        .unwrap_err();
        assert_eq!(error.code, "TYPE_MISMATCH");
    }

    #[test]
    fn unknown_capability_refuses_before_any_execution() {
        let mut c = config(InputBinding::Literal {
            value: Value::String("x".into()),
        });
        c.actions.get_mut("inspect").unwrap().steps[0].capability = "unknown.capability".into();
        let error = plan_action(
            &c,
            "inspect",
            &machine_contract::contract(),
            &machine_contract::unknown_context(),
        )
        .unwrap_err();
        assert_eq!(error.code, "UNKNOWN_CAPABILITY");
    }
}
