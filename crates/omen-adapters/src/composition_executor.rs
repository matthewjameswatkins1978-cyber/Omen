use omen_core::composition::{
    ActionExecutionError, ActionPlan, ActionPlanStep, ActionRunReport, ActionRunStatus,
    ActionStepRunResult, ActionStepRunStatus, StateChange,
};
use omen_core::{ExecutionContract, ProcessExit};
use omen_engine::ProcessSupervisor;
use omen_knowledge::{ContentAddressedStore, Database};
use omen_semantic::{SemanticLookupResult, SemanticProviderRegistry};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

pub const MAX_FILE_READ_BYTES: usize = 64 * 1024;
pub const MAX_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ArtifactPayload {
    pub media_type: String,
    pub producer: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct CapabilityExecutionResult {
    pub output: Value,
    pub artifacts: Vec<ArtifactPayload>,
    pub process_exit: Option<ProcessExit>,
    pub enforcement: Option<omen_core::EnforcementReport>,
    pub state_changed: StateChange,
}

pub struct CapabilityRequest<'a> {
    pub capability_id: &'a str,
    pub inputs: &'a BTreeMap<String, Value>,
    pub authority: Option<&'a ExecutionContract>,
}

pub trait CapabilityExecutor: Send + Sync {
    fn supports(&self, capability_id: &str) -> bool;
    fn available(&self, capability_id: &str) -> bool;
    fn execute<'a>(
        &'a self,
        request: CapabilityRequest<'a>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<CapabilityExecutionResult, ActionExecutionError>>
                + Send
                + 'a,
        >,
    >;
}

pub struct DefaultCapabilityExecutor {
    workspace_root: PathBuf,
    semantic_registry: Arc<SemanticProviderRegistry>,
    supervisor: ProcessSupervisor,
}

impl DefaultCapabilityExecutor {
    pub fn new(
        workspace_root: PathBuf,
        semantic_registry: Arc<SemanticProviderRegistry>,
        supervisor: ProcessSupervisor,
    ) -> Self {
        Self {
            workspace_root,
            semantic_registry,
            supervisor,
        }
    }

    fn input<'a>(
        inputs: &'a BTreeMap<String, Value>,
        name: &str,
    ) -> Result<&'a Value, ActionExecutionError> {
        inputs.get(name).ok_or_else(|| ActionExecutionError {
            code: "STEP_INPUT_SCHEMA_VIOLATION".into(),
            message: format!("required input '{name}' is missing"),
        })
    }

    fn string_input(
        inputs: &BTreeMap<String, Value>,
        name: &str,
    ) -> Result<String, ActionExecutionError> {
        Self::input(inputs, name)?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ActionExecutionError {
                code: "STEP_INPUT_SCHEMA_VIOLATION".into(),
                message: format!("input '{name}' must be a string"),
            })
    }

    fn bounded_workspace_path(&self, raw: &str) -> Result<PathBuf, ActionExecutionError> {
        let candidate = self.workspace_root.join(raw);
        let canonical = candidate.canonicalize().map_err(|e| ActionExecutionError {
            code: "CAPABILITY_EXECUTION_FAILED".into(),
            message: format!("cannot resolve filesystem.read path: {e}"),
        })?;
        let root = self
            .workspace_root
            .canonicalize()
            .map_err(|e| ActionExecutionError {
                code: "CAPABILITY_EXECUTION_FAILED".into(),
                message: e.to_string(),
            })?;
        if !canonical.starts_with(&root) {
            return Err(ActionExecutionError {
                code: "WORKSPACE_SCOPE_VIOLATION".into(),
                message: "filesystem.read path escapes the workspace".into(),
            });
        }
        Ok(canonical)
    }

    async fn run(
        &self,
        request: CapabilityRequest<'_>,
    ) -> Result<CapabilityExecutionResult, ActionExecutionError> {
        match request.capability_id {
            "semantic.definition" => {
                let symbol = Self::string_input(request.inputs, "symbol")?;
                let result = self
                    .semantic_registry
                    .find_definition(&symbol, None, None, None, None)
                    .await
                    .map_err(core_error)?;
                Ok(CapabilityExecutionResult {
                    output: semantic_output(result),
                    artifacts: vec![],
                    process_exit: None,
                    enforcement: None,
                    state_changed: StateChange::No,
                })
            }
            "semantic.references" => {
                let symbol = Self::string_input(request.inputs, "symbol")?;
                let result = self
                    .semantic_registry
                    .find_references(&symbol, None, None, None, None, None)
                    .await
                    .map_err(core_error)?;
                Ok(CapabilityExecutionResult {
                    output: semantic_output(result),
                    artifacts: vec![],
                    process_exit: None,
                    enforcement: None,
                    state_changed: StateChange::No,
                })
            }
            "semantic.diagnostics" => {
                let result = self
                    .semantic_registry
                    .diagnostics(None)
                    .await
                    .map_err(core_error)?;
                Ok(CapabilityExecutionResult {
                    output: json!({"status":"resolved","results":result}),
                    artifacts: vec![],
                    process_exit: None,
                    enforcement: None,
                    state_changed: StateChange::No,
                })
            }
            "structure.search" => {
                let pattern = Self::string_input(request.inputs, "pattern")?;
                let result = self
                    .semantic_registry
                    .structural_search(&pattern, "rust", None, None)
                    .await
                    .map_err(core_error)?;
                Ok(CapabilityExecutionResult {
                    output: json!({"status":"resolved","results":result}),
                    artifacts: vec![],
                    process_exit: None,
                    enforcement: None,
                    state_changed: StateChange::No,
                })
            }
            "filesystem.read" => {
                let path =
                    self.bounded_workspace_path(&Self::string_input(request.inputs, "path")?)?;
                let bytes = std::fs::read(&path).map_err(|e| ActionExecutionError {
                    code: "CAPABILITY_EXECUTION_FAILED".into(),
                    message: e.to_string(),
                })?;
                if bytes.len() > MAX_FILE_READ_BYTES {
                    return Err(ActionExecutionError {
                        code: "RESULT_BOUNDS_EXCEEDED".into(),
                        message: format!("filesystem.read exceeds {MAX_FILE_READ_BYTES} bytes"),
                    });
                }
                let content = String::from_utf8(bytes).map_err(|_| ActionExecutionError {
                    code: "CAPABILITY_OUTPUT_SCHEMA_VIOLATION".into(),
                    message: "filesystem.read returned non-UTF-8 content".into(),
                })?;
                Ok(CapabilityExecutionResult {
                    output: json!({"status":"resolved","content":content}),
                    artifacts: vec![],
                    process_exit: None,
                    enforcement: None,
                    state_changed: StateChange::No,
                })
            }
            "execution.run" => {
                let argv = Self::input(request.inputs, "argv")?
                    .as_array()
                    .ok_or_else(|| ActionExecutionError {
                        code: "STEP_INPUT_SCHEMA_VIOLATION".into(),
                        message: "argv must be an array".into(),
                    })?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| ActionExecutionError {
                                code: "STEP_INPUT_SCHEMA_VIOLATION".into(),
                                message: "argv items must be strings".into(),
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let contract = request.authority.ok_or_else(|| ActionExecutionError {
                    code: "AUTHORITY_REQUIRED".into(),
                    message: "execution.run requires a current external execution contract".into(),
                })?;
                self.supervisor
                    .validate_contract(contract, Some(&argv), true)
                    .map_err(core_error)?;
                let output = self
                    .supervisor
                    .execute_contract(
                        contract,
                        self.workspace_root.clone(),
                        omen_engine::DEFAULT_INLINE_BUDGET,
                        true,
                    )
                    .await
                    .map_err(core_error)?;
                if output.runtime_status == omen_core::RuntimeStatus::TimedOut {
                    return Err(ActionExecutionError {
                        code: "TIMEOUT".into(),
                        message: "execution.run timed out; the supervisor cleaned up the child"
                            .into(),
                    });
                }
                if output.stdout_all.len() > MAX_ARTIFACT_BYTES
                    || output.stderr_all.len() > MAX_ARTIFACT_BYTES
                {
                    return Err(ActionExecutionError {
                        code: "RESULT_BOUNDS_EXCEEDED".into(),
                        message: format!("execution output exceeds {MAX_ARTIFACT_BYTES} bytes"),
                    });
                }
                let mut artifacts = Vec::new();
                if output.stdout_all.len() > omen_engine::DEFAULT_INLINE_BUDGET {
                    artifacts.push(ArtifactPayload {
                        media_type: "text/plain".into(),
                        producer: "omen://composition/execution.stdout".into(),
                        bytes: output.stdout_all.clone(),
                    });
                }
                if output.stderr_all.len() > omen_engine::DEFAULT_INLINE_BUDGET {
                    artifacts.push(ArtifactPayload {
                        media_type: "text/plain".into(),
                        producer: "omen://composition/execution.stderr".into(),
                        bytes: output.stderr_all.clone(),
                    });
                }
                Ok(CapabilityExecutionResult {
                    output: json!({"status": if output.process_exit.is_zero() {"resolved"} else {"failed"}, "stdout":String::from_utf8_lossy(&output.stdout_bounded), "stderr":String::from_utf8_lossy(&output.stderr_bounded)}),
                    artifacts,
                    process_exit: Some(output.process_exit),
                    enforcement: Some(output.enforcement),
                    state_changed: StateChange::Possible,
                })
            }
            _ => Err(ActionExecutionError {
                code: "CAPABILITY_EXECUTION_UNSUPPORTED".into(),
                message: format!(
                    "capability '{}' has no composition executor",
                    request.capability_id
                ),
            }),
        }
    }
}

impl CapabilityExecutor for DefaultCapabilityExecutor {
    fn supports(&self, capability_id: &str) -> bool {
        matches!(
            capability_id,
            "semantic.definition"
                | "semantic.references"
                | "semantic.diagnostics"
                | "structure.search"
                | "filesystem.read"
                | "execution.run"
        )
    }
    fn available(&self, capability_id: &str) -> bool {
        match capability_id {
            "filesystem.read" | "execution.run" => true,
            "semantic.definition" => self
                .semantic_registry
                .list_providers()
                .iter()
                .any(|(_, _, _, a)| *a),
            "semantic.references" => self
                .semantic_registry
                .list_providers()
                .iter()
                .any(|(_, _, _, a)| *a),
            "semantic.diagnostics" => self
                .semantic_registry
                .list_providers()
                .iter()
                .any(|(_, _, _, a)| *a),
            "structure.search" => self
                .semantic_registry
                .list_providers()
                .iter()
                .any(|(_, _, kind, a)| *a && *kind == omen_semantic::ProviderKind::Structural),
            _ => false,
        }
    }
    fn execute<'a>(
        &'a self,
        request: CapabilityRequest<'a>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<CapabilityExecutionResult, ActionExecutionError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(self.run(request))
    }
}

pub async fn execute_plan(
    plan: &ActionPlan,
    executor: &dyn CapabilityExecutor,
    authorities: &BTreeMap<String, ExecutionContract>,
    db: &mut Database,
    cas: &ContentAddressedStore,
    context_generation: Option<i64>,
) -> Result<ActionRunReport, ActionExecutionError> {
    preflight(plan, executor, authorities)?;
    let started = std::time::Instant::now();
    let mut outputs = BTreeMap::<String, Value>::new();
    let mut steps = Vec::new();
    let mut report_artifacts = Vec::new();
    let mut overall_change = StateChange::No;
    let mut failed_step = None;
    for step in &plan.steps {
        let input_start = std::time::Instant::now();
        let inputs = resolve_inputs(step, &outputs)?;
        let result = executor
            .execute(CapabilityRequest {
                capability_id: &step.capability_id,
                inputs: &inputs,
                authority: authorities.get(&step.step_id),
            })
            .await;
        match result {
            Ok(result) => {
                validate_output_shape(&step.capability_id, &result.output)?;
                let mut step_artifacts = Vec::new();
                for payload in result.artifacts {
                    let artifact = cas
                        .store(
                            db,
                            &payload.bytes,
                            &payload.media_type,
                            &payload.producer,
                            omen_core::RetentionClass::Referenced,
                        )
                        .map_err(core_error)?;
                    let uri = artifact.uri.to_string();
                    report_artifacts.push(uri.clone());
                    step_artifacts.push(uri);
                }
                outputs.insert(step.step_id.clone(), result.output.clone());
                overall_change = merge_change(overall_change, result.state_changed);
                steps.push(ActionStepRunResult {
                    step_id: step.step_id.clone(),
                    capability_id: step.capability_id.clone(),
                    status: if result.process_exit.as_ref().is_some_and(|e| !e.is_zero()) {
                        ActionStepRunStatus::Failed
                    } else {
                        ActionStepRunStatus::Completed
                    },
                    duration_ms: input_start.elapsed().as_millis() as u64,
                    output: Some(result.output),
                    error: None,
                    artifacts: step_artifacts,
                    process_exit: result.process_exit,
                    enforcement: result.enforcement,
                    state_changed: result.state_changed,
                });
                if steps
                    .last()
                    .is_some_and(|s| s.status == ActionStepRunStatus::Failed)
                {
                    failed_step = Some(step.step_id.clone());
                    break;
                }
            }
            Err(error) => {
                failed_step = Some(step.step_id.clone());
                let status = if error.code == "TIMEOUT" {
                    ActionStepRunStatus::TimedOut
                } else if is_refusal_code(&error.code) {
                    ActionStepRunStatus::Refused
                } else {
                    ActionStepRunStatus::Failed
                };
                let state_changed = if is_refusal_code(&error.code) {
                    StateChange::No
                } else {
                    StateChange::Possible
                };
                steps.push(ActionStepRunResult {
                    step_id: step.step_id.clone(),
                    capability_id: step.capability_id.clone(),
                    status,
                    duration_ms: input_start.elapsed().as_millis() as u64,
                    output: None,
                    error: Some(error),
                    artifacts: vec![],
                    process_exit: None,
                    enforcement: None,
                    state_changed,
                });
                break;
            }
        }
    }
    let status = match failed_step {
        None => ActionRunStatus::Completed,
        Some(_)
            if steps
                .iter()
                .any(|s| s.status == ActionStepRunStatus::Completed) =>
        {
            ActionRunStatus::Partial
        }
        Some(_)
            if steps
                .iter()
                .any(|s| s.status == ActionStepRunStatus::Refused) =>
        {
            ActionRunStatus::Refused
        }
        Some(_) => ActionRunStatus::Failed,
    };
    let mut report = ActionRunReport {
        action_id: plan.action_id.clone(),
        plan_digest: plan.plan_digest.clone(),
        contract_digest: plan.contract_digest.clone(),
        context_generation_before: context_generation,
        context_generation_after: context_generation,
        status,
        state_changed: overall_change,
        completed_steps: steps
            .iter()
            .filter(|s| s.status == ActionStepRunStatus::Completed)
            .count(),
        failed_step,
        steps,
        artifacts: report_artifacts,
        duration_ms: started.elapsed().as_millis() as u64,
        evidence_artifact: None,
    };
    let evidence = serde_json::to_vec(&report).map_err(|e| ActionExecutionError {
        code: "EVIDENCE_SERIALIZATION_FAILED".into(),
        message: e.to_string(),
    })?;
    let artifact = cas
        .store(
            db,
            &evidence,
            "application/json",
            "omen://composition/action-run",
            omen_core::RetentionClass::Referenced,
        )
        .map_err(core_error)?;
    report.evidence_artifact = Some(artifact.uri.to_string());
    Ok(report)
}

fn preflight(
    plan: &ActionPlan,
    executor: &dyn CapabilityExecutor,
    authorities: &BTreeMap<String, ExecutionContract>,
) -> Result<(), ActionExecutionError> {
    let ids: BTreeSet<_> = plan.steps.iter().map(|s| s.step_id.as_str()).collect();
    for step in &plan.steps {
        if step.capability_id == "composition.plan" || step.capability_id == "composition.run" {
            return Err(ActionExecutionError {
                code: "NESTED_COMPOSITION_UNSUPPORTED".into(),
                message: "composition capabilities cannot be child steps".into(),
            });
        }
        if !executor.supports(&step.capability_id) {
            return Err(ActionExecutionError {
                code: "CAPABILITY_EXECUTION_UNSUPPORTED".into(),
                message: format!("no executor for '{}'", step.capability_id),
            });
        }
        if !executor.available(&step.capability_id) {
            return Err(ActionExecutionError {
                code: "CAPABILITY_UNAVAILABLE".into(),
                message: format!("executor for '{}' is unavailable", step.capability_id),
            });
        }
        if step.authority != "none" && !authorities.contains_key(&step.step_id) {
            return Err(ActionExecutionError {
                code: "AUTHORITY_REQUIRED".into(),
                message: format!("step '{}' requires current authority", step.step_id),
            });
        }
    }
    for step_id in authorities.keys() {
        if !ids.contains(step_id.as_str()) {
            return Err(ActionExecutionError {
                code: "AUTHORITY_CONTRACT_INVALID".into(),
                message: format!("authority supplied for unrelated step '{step_id}'"),
            });
        }
        if let Some(step) = plan.steps.iter().find(|step| step.step_id == *step_id)
            && step.authority == "none"
        {
            return Err(ActionExecutionError {
                code: "AUTHORITY_CONTRACT_INVALID".into(),
                message: format!("step '{step_id}' does not require external authority"),
            });
        }
    }
    Ok(())
}

fn resolve_inputs(
    step: &ActionPlanStep,
    outputs: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, ActionExecutionError> {
    let mut resolved = BTreeMap::new();
    for (field, input) in &step.inputs {
        let value = match input {
            omen_core::composition::PlannedInput::Literal { value } => value.clone(),
            omen_core::composition::PlannedInput::StepOutput {
                step,
                field: source_field,
            } => outputs
                .get(step)
                .and_then(|v| v.get(source_field))
                .cloned()
                .ok_or_else(|| ActionExecutionError {
                    code: "STEP_OUTPUT_MISSING".into(),
                    message: format!("output '{step}.{source_field}' is unavailable"),
                })?,
        };
        resolved.insert(field.clone(), value);
    }
    Ok(resolved)
}

fn semantic_output<T: serde::Serialize>(result: SemanticLookupResult<T>) -> Value {
    match result {
        SemanticLookupResult::Resolved(value) => json!({"status":"resolved","results":[value]}),
        SemanticLookupResult::Ambiguous(values) => json!({"status":"ambiguous","results":values}),
        SemanticLookupResult::Stale(value) => json!({"status":"stale","results":[value]}),
        SemanticLookupResult::NotFound => json!({"status":"not_found","results":[]}),
        SemanticLookupResult::Unsupported => json!({"status":"unsupported","results":[]}),
    }
}

fn is_refusal_code(code: &str) -> bool {
    matches!(
        code,
        "AUTHORITY_REQUIRED"
            | "AUTHORITY_CONTRACT_INVALID"
            | "AUTHORITY_CONTRACT_MISMATCH"
            | "CONTRACT_SEMANTICS_UNSUPPORTED"
            | "NETWORK_CONSTRAINT_UNENFORCEABLE"
            | "NESTED_COMPOSITION_UNSUPPORTED"
            | "CAPABILITY_EXECUTION_UNSUPPORTED"
            | "CAPABILITY_UNAVAILABLE"
            | "WORKSPACE_SCOPE_VIOLATION"
    )
}

fn validate_output_shape(capability_id: &str, output: &Value) -> Result<(), ActionExecutionError> {
    let Some(definition) = omen_core::machine_contract::capability(capability_id) else {
        return Err(ActionExecutionError {
            code: "CAPABILITY_OUTPUT_SCHEMA_VIOLATION".into(),
            message: format!("no output schema exists for '{capability_id}'"),
        });
    };
    let object = output.as_object().ok_or_else(|| ActionExecutionError {
        code: "CAPABILITY_OUTPUT_SCHEMA_VIOLATION".into(),
        message: format!("'{capability_id}' returned a non-object output"),
    })?;
    if let Some(required) = definition
        .output_schema
        .get("required")
        .and_then(Value::as_array)
    {
        for field in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(field) {
                return Err(ActionExecutionError {
                    code: "CAPABILITY_OUTPUT_SCHEMA_VIOLATION".into(),
                    message: format!(
                        "'{capability_id}' output is missing required field '{field}'"
                    ),
                });
            }
        }
    }
    if let Some(properties) = definition
        .output_schema
        .get("properties")
        .and_then(Value::as_object)
    {
        for (field, schema) in properties {
            let Some(value) = object.get(field) else {
                continue;
            };
            let valid = match schema.get("type").and_then(Value::as_str) {
                Some("string") => value.is_string(),
                Some("array") => value.is_array(),
                Some("object") => value.is_object(),
                Some("boolean") => value.is_boolean(),
                Some("number") => value.is_number(),
                _ => true,
            };
            if !valid {
                return Err(ActionExecutionError {
                    code: "CAPABILITY_OUTPUT_SCHEMA_VIOLATION".into(),
                    message: format!("'{capability_id}' output field '{field}' has the wrong type"),
                });
            }
        }
    }
    Ok(())
}
fn merge_change(a: StateChange, b: StateChange) -> StateChange {
    if a == StateChange::Yes || b == StateChange::Yes {
        StateChange::Yes
    } else if a == StateChange::Possible || b == StateChange::Possible {
        StateChange::Possible
    } else {
        StateChange::No
    }
}
fn core_error(error: omen_core::CoreError) -> ActionExecutionError {
    let message = error.to_string();
    let code = message
        .strip_prefix("Execution failed: ")
        .and_then(|rest| rest.split(':').next())
        .or_else(|| message.split(':').next())
        .unwrap_or("CAPABILITY_EXECUTION_FAILED")
        .to_string();
    ActionExecutionError { code, message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omen_core::composition::{
        InputBinding, NamedActionDefinition, OmenWorkspaceConfig, ProjectConfig, plan_action,
    };
    use omen_core::machine_contract::{self, context_with_generation};
    use std::sync::Mutex;

    type FakeCall = (String, BTreeMap<String, Value>);

    struct FakeExecutor {
        calls: Arc<Mutex<Vec<FakeCall>>>,
        failure: Option<String>,
        invalid_output: bool,
    }

    impl CapabilityExecutor for FakeExecutor {
        fn supports(&self, capability_id: &str) -> bool {
            capability_id == "filesystem.read"
        }
        fn available(&self, capability_id: &str) -> bool {
            self.supports(capability_id)
        }
        fn execute<'a>(
            &'a self,
            request: CapabilityRequest<'a>,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<CapabilityExecutionResult, ActionExecutionError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push((request.capability_id.to_owned(), request.inputs.clone()));
                if self.failure.as_deref()
                    == Some(
                        request
                            .inputs
                            .get("path")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
                {
                    return Err(ActionExecutionError {
                        code: "STEP_FAILED".into(),
                        message: "fake failure".into(),
                    });
                }
                let output = if self.invalid_output {
                    json!({"status": []})
                } else {
                    json!({"status":"ok","content":"x"})
                };
                Ok(CapabilityExecutionResult {
                    output,
                    artifacts: vec![],
                    process_exit: None,
                    enforcement: None,
                    state_changed: StateChange::No,
                })
            })
        }
    }

    fn plan_with_steps(steps: Vec<omen_core::composition::ActionStepDefinition>) -> ActionPlan {
        let mut actions = BTreeMap::new();
        actions.insert(
            "test".into(),
            NamedActionDefinition {
                description: None,
                steps,
            },
        );
        let config = OmenWorkspaceConfig {
            schema_version: 1,
            project: Some(ProjectConfig { name: None }),
            actions,
        };
        plan_action(
            &config,
            "test",
            &machine_contract::contract(),
            &context_with_generation(None),
        )
        .unwrap()
    }

    fn step(id: &str, path: &str) -> omen_core::composition::ActionStepDefinition {
        let mut input = BTreeMap::new();
        input.insert("path".into(), InputBinding::Literal { value: json!(path) });
        omen_core::composition::ActionStepDefinition {
            id: id.into(),
            capability: "filesystem.read".into(),
            input,
        }
    }

    #[tokio::test]
    async fn fake_executor_stops_on_failure_and_reports_partial() {
        let plan = plan_with_steps(vec![step("a", "one"), step("b", "two"), step("c", "three")]);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let executor = FakeExecutor {
            calls: calls.clone(),
            failure: Some("two".into()),
            invalid_output: false,
        };
        let temp = tempfile::tempdir().unwrap();
        let mut db = Database::open_in_memory().unwrap();
        let cas = ContentAddressedStore::new(temp.path().join("cas"));
        let report = execute_plan(&plan, &executor, &BTreeMap::new(), &mut db, &cas, None)
            .await
            .unwrap();
        assert_eq!(report.status, ActionRunStatus::Partial);
        assert_eq!(report.failed_step.as_deref(), Some("b"));
        assert_eq!(calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn fake_executor_routes_prior_output_and_rejects_bad_runtime_shape() {
        let mut second_input = BTreeMap::new();
        second_input.insert(
            "path".into(),
            InputBinding::StepOutput {
                step: "a".into(),
                field: "content".into(),
            },
        );
        let second = omen_core::composition::ActionStepDefinition {
            id: "b".into(),
            capability: "filesystem.read".into(),
            input: second_input,
        };
        let plan = plan_with_steps(vec![step("a", "one"), second]);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let executor = FakeExecutor {
            calls: calls.clone(),
            failure: None,
            invalid_output: false,
        };
        let temp = tempfile::tempdir().unwrap();
        let mut db = Database::open_in_memory().unwrap();
        let cas = ContentAddressedStore::new(temp.path().join("cas"));
        execute_plan(&plan, &executor, &BTreeMap::new(), &mut db, &cas, None)
            .await
            .unwrap();
        assert_eq!(calls.lock().unwrap()[1].1["path"], json!("x"));

        let bad = FakeExecutor {
            calls: Arc::new(Mutex::new(Vec::new())),
            failure: None,
            invalid_output: true,
        };
        let error = execute_plan(&plan, &bad, &BTreeMap::new(), &mut db, &cas, None)
            .await
            .unwrap_err();
        assert_eq!(error.code, "CAPABILITY_OUTPUT_SCHEMA_VIOLATION");
    }
}
