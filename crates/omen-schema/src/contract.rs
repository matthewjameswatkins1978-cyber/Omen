use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION_EXECUTION: &str = "omen.execution/0.2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaseRequestWire {
    pub resource: String,
    pub rights: Vec<String>,
    pub native_access: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StdioConfigWire {
    pub stdin: String,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IntentWire {
    pub tool: String,
    pub operation: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionConstraintsWire {
    pub timeout_ms: u64,
    #[serde(default)]
    pub network: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequiredAssuranceWire {
    #[serde(default)]
    pub filesystem: Option<String>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub descendants: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionContractWire {
    pub schema_version: String,
    pub execution_id: String,
    pub actor: String,
    pub intent: IntentWire,
    pub leases: Vec<LeaseRequestWire>,
    pub stdio: StdioConfigWire,
    pub constraints: ExecutionConstraintsWire,
    pub required_assurance: RequiredAssuranceWire,
}
