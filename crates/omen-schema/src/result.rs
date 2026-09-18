use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION_RESULT: &str = "omen.result/0.2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
