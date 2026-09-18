use omen_core::{
    Assurance, CoreError, ExecutionConstraints, ExecutionContract, ExecutionId, Intent,
    LeaseRequest, LeaseRights, RequiredAssurance, ResourceUri, StdioConfig, StdioMode,
};
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
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

impl TryFrom<ExecutionContractWire> for ExecutionContract {
    type Error = CoreError;

    fn try_from(wire: ExecutionContractWire) -> Result<Self, Self::Error> {
        if wire.schema_version != SCHEMA_VERSION_EXECUTION {
            return Err(CoreError::UnsupportedSchemaVersion {
                version: wire.schema_version,
                expected: SCHEMA_VERSION_EXECUTION.into(),
            });
        }

        let execution_id = ExecutionId::new(wire.execution_id)?;
        let actor = ResourceUri::parse(&wire.actor)?;
        let tool = ResourceUri::parse(&wire.intent.tool)?;

        let intent = Intent {
            tool,
            operation: wire.intent.operation,
            args: wire.intent.args,
        };

        let mut leases = Vec::with_capacity(wire.leases.len());
        for l in wire.leases {
            let resource = ResourceUri::parse(&l.resource)?;
            let mut rights = Vec::with_capacity(l.rights.len());
            for r in l.rights {
                match r.to_lowercase().as_str() {
                    "read" => rights.push(LeaseRights::Read),
                    "write" => rights.push(LeaseRights::Write),
                    "execute" => rights.push(LeaseRights::Execute),
                    other => {
                        return Err(CoreError::SchemaViolation(format!(
                            "Invalid lease right '{other}'"
                        )));
                    }
                }
            }
            leases.push(LeaseRequest {
                resource,
                rights,
                native_access: l.native_access,
            });
        }

        let parse_stdio_mode = |s: &str| -> Result<StdioMode, CoreError> {
            match s.to_lowercase().as_str() {
                "closed" => Ok(StdioMode::Closed),
                "inline" | "capture" => Ok(StdioMode::Inline),
                "artifact" => Ok(StdioMode::Artifact),
                "interactive" => Ok(StdioMode::Interactive),
                "inherit" => Ok(StdioMode::Inherit),
                other => Err(CoreError::SchemaViolation(format!(
                    "Invalid stdio mode '{other}'"
                ))),
            }
        };

        let stdio = StdioConfig {
            stdin: parse_stdio_mode(&wire.stdio.stdin)?,
            stdout: parse_stdio_mode(&wire.stdio.stdout)?,
            stderr: parse_stdio_mode(&wire.stdio.stderr)?,
        };

        let network_denied = match wire.constraints.network.as_deref() {
            Some("deny") => true,
            Some("allow") | None => false,
            Some(other) => {
                return Err(CoreError::SchemaViolation(format!(
                    "Invalid network constraint '{other}'"
                )));
            }
        };

        let parse_assurance = |opt: Option<String>| -> Result<Option<Assurance>, CoreError> {
            match opt {
                None => Ok(None),
                Some(s) => match s.to_uppercase().as_str() {
                    "DETERMINISTIC" => Ok(Some(Assurance::Deterministic)),
                    "VERIFIED" => Ok(Some(Assurance::Verified)),
                    "ENFORCED" => Ok(Some(Assurance::Enforced)),
                    "OBSERVED" => Ok(Some(Assurance::Observed)),
                    "CLAIMED" => Ok(Some(Assurance::Claimed)),
                    "INFERRED" => Ok(Some(Assurance::Inferred)),
                    "UNKNOWN" => Ok(Some(Assurance::Unknown)),
                    other => Err(CoreError::SchemaViolation(format!(
                        "Invalid assurance string '{other}'"
                    ))),
                },
            }
        };

        let required_assurance = RequiredAssurance {
            filesystem: parse_assurance(wire.required_assurance.filesystem)?,
            network: parse_assurance(wire.required_assurance.network)?,
            descendants: parse_assurance(wire.required_assurance.descendants)?,
        };

        Ok(ExecutionContract {
            execution_id,
            actor,
            intent,
            leases,
            stdio,
            constraints: ExecutionConstraints {
                timeout_ms: wire.constraints.timeout_ms,
                network_denied,
            },
            required_assurance,
        })
    }
}

impl From<&ExecutionContract> for ExecutionContractWire {
    fn from(domain: &ExecutionContract) -> Self {
        Self {
            schema_version: SCHEMA_VERSION_EXECUTION.into(),
            execution_id: domain.execution_id.as_str().into(),
            actor: domain.actor.as_str().into(),
            intent: IntentWire {
                tool: domain.intent.tool.as_str().into(),
                operation: domain.intent.operation.clone(),
                args: domain.intent.args.clone(),
            },
            leases: domain
                .leases
                .iter()
                .map(|l| LeaseRequestWire {
                    resource: l.resource.as_str().into(),
                    rights: l
                        .rights
                        .iter()
                        .map(|r| match r {
                            LeaseRights::Read => "read".into(),
                            LeaseRights::Write => "write".into(),
                            LeaseRights::Execute => "execute".into(),
                        })
                        .collect(),
                    native_access: l.native_access,
                })
                .collect(),
            stdio: StdioConfigWire {
                stdin: match domain.stdio.stdin {
                    StdioMode::Closed => "closed".into(),
                    StdioMode::Inline => "inline".into(),
                    StdioMode::Artifact => "artifact".into(),
                    StdioMode::Interactive => "interactive".into(),
                    StdioMode::Inherit => "inherit".into(),
                },
                stdout: match domain.stdio.stdout {
                    StdioMode::Closed => "closed".into(),
                    StdioMode::Inline => "inline".into(),
                    StdioMode::Artifact => "artifact".into(),
                    StdioMode::Interactive => "interactive".into(),
                    StdioMode::Inherit => "inherit".into(),
                },
                stderr: match domain.stdio.stderr {
                    StdioMode::Closed => "closed".into(),
                    StdioMode::Inline => "inline".into(),
                    StdioMode::Artifact => "artifact".into(),
                    StdioMode::Interactive => "interactive".into(),
                    StdioMode::Inherit => "inherit".into(),
                },
            },
            constraints: ExecutionConstraintsWire {
                timeout_ms: domain.constraints.timeout_ms,
                network: if domain.constraints.network_denied {
                    Some("deny".into())
                } else {
                    Some("allow".into())
                },
            },
            required_assurance: RequiredAssuranceWire {
                filesystem: domain
                    .required_assurance
                    .filesystem
                    .map(|a| format!("{a:?}").to_uppercase()),
                network: domain
                    .required_assurance
                    .network
                    .map(|a| format!("{a:?}").to_uppercase()),
                descendants: domain
                    .required_assurance
                    .descendants
                    .map(|a| format!("{a:?}").to_uppercase()),
            },
        }
    }
}
