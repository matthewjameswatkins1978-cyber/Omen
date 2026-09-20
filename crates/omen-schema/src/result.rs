use omen_core::{
    ActionId, AdapterClassification, CoreError, EnforcementLevel, EnforcementReport, ExecutionId,
    ExecutionResult, ProcessExit, ResourceId, ResourceUri, RuntimeStatus,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION_RESULT: &str = "omen.result/0.2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct ProcessExitWire {
    #[serde(default)]
    pub code: Option<i32>,
    #[serde(default)]
    pub signal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnforcementReportWire {
    pub filesystem: String,
    pub network: String,
    pub descendant_processes: String,
    pub symlink_escape: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionResultWire {
    pub schema_version: String,
    pub execution_id: String,
    pub action_id: String,
    pub runtime_status: String,
    pub process_exit: ProcessExitWire,
    pub adapter_classification: String,
    pub enforcement: EnforcementReportWire,
    pub observations: Vec<String>,
    pub fact_updates: Vec<String>,
    pub artifacts: Vec<String>,
    pub reduced_summary: String,
}

impl TryFrom<ExecutionResultWire> for ExecutionResult {
    type Error = CoreError;

    fn try_from(wire: ExecutionResultWire) -> Result<Self, Self::Error> {
        if wire.schema_version != SCHEMA_VERSION_RESULT {
            return Err(CoreError::UnsupportedSchemaVersion {
                version: wire.schema_version,
                expected: SCHEMA_VERSION_RESULT.into(),
            });
        }

        let execution_id = ExecutionId::new(wire.execution_id)?;
        let action_id = ActionId::new(wire.action_id)?;

        let runtime_status = match wire.runtime_status.to_uppercase().as_str() {
            "COMPLETED" => RuntimeStatus::Completed,
            "SPAWN_FAILED" => RuntimeStatus::SpawnFailed,
            "TIMED_OUT" => RuntimeStatus::TimedOut,
            "CANCELLED" => RuntimeStatus::Cancelled,
            "CONTAINMENT_FAILED" => RuntimeStatus::ContainmentFailed,
            "IO_FAILED" => RuntimeStatus::IoFailed,
            other => {
                return Err(CoreError::SchemaViolation(format!(
                    "Invalid runtime_status '{other}'"
                )));
            }
        };

        let adapter_classification = match wire.adapter_classification.to_uppercase().as_str() {
            "SUCCESS" => AdapterClassification::Success,
            "FAILURE" => AdapterClassification::Failure,
            "REFUSAL" => AdapterClassification::Refusal,
            "UNKNOWN" => AdapterClassification::Unknown,
            other => {
                return Err(CoreError::SchemaViolation(format!(
                    "Invalid adapter_classification '{other}'"
                )));
            }
        };

        let parse_enforcement = |s: &str| -> Result<EnforcementLevel, CoreError> {
            match s.to_uppercase().as_str() {
                "ENFORCED" => Ok(EnforcementLevel::Enforced),
                "MEDIATED" => Ok(EnforcementLevel::Mediated),
                "OBSERVED" => Ok(EnforcementLevel::Observed),
                "BEST_EFFORT" => Ok(EnforcementLevel::BestEffort),
                "UNSUPPORTED" => Ok(EnforcementLevel::Unsupported),
                "PREVENTED" => Ok(EnforcementLevel::Enforced),
                other => Err(CoreError::SchemaViolation(format!(
                    "Invalid enforcement level '{other}'"
                ))),
            }
        };

        let enforcement = EnforcementReport {
            filesystem: parse_enforcement(&wire.enforcement.filesystem)?,
            network: parse_enforcement(&wire.enforcement.network)?,
            descendant_processes: parse_enforcement(&wire.enforcement.descendant_processes)?,
            symlink_escape: parse_enforcement(&wire.enforcement.symlink_escape)?,
        };

        let mut fact_updates = Vec::with_capacity(wire.fact_updates.len());
        for f in wire.fact_updates {
            fact_updates.push(ResourceId::new(f)?);
        }

        let mut artifacts = Vec::with_capacity(wire.artifacts.len());
        for a in wire.artifacts {
            artifacts.push(ResourceUri::parse(&a)?);
        }

        Ok(ExecutionResult {
            execution_id,
            action_id,
            runtime_status,
            process_exit: ProcessExit {
                code: wire.process_exit.code,
                signal: wire.process_exit.signal,
            },
            adapter_classification,
            enforcement,
            observations: wire.observations,
            fact_updates,
            artifacts,
            reduced_summary: wire.reduced_summary,
        })
    }
}

impl From<&ExecutionResult> for ExecutionResultWire {
    fn from(domain: &ExecutionResult) -> Self {
        Self {
            schema_version: SCHEMA_VERSION_RESULT.into(),
            execution_id: domain.execution_id.as_str().into(),
            action_id: domain.action_id.as_str().into(),
            runtime_status: format!("{:?}", domain.runtime_status).to_uppercase(),
            process_exit: ProcessExitWire {
                code: domain.process_exit.code,
                signal: domain.process_exit.signal.clone(),
            },
            adapter_classification: format!("{:?}", domain.adapter_classification).to_uppercase(),
            enforcement: EnforcementReportWire {
                filesystem: format!("{:?}", domain.enforcement.filesystem).to_uppercase(),
                network: format!("{:?}", domain.enforcement.network).to_uppercase(),
                descendant_processes: format!("{:?}", domain.enforcement.descendant_processes)
                    .to_uppercase(),
                symlink_escape: format!("{:?}", domain.enforcement.symlink_escape).to_uppercase(),
            },
            observations: domain.observations.clone(),
            fact_updates: domain
                .fact_updates
                .iter()
                .map(|f| f.as_str().into())
                .collect(),
            artifacts: domain.artifacts.iter().map(|a| a.as_str().into()).collect(),
            reduced_summary: domain.reduced_summary.clone(),
        }
    }
}
