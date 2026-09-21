use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

/// Independent version of the public Omen machine-error envelope.
pub const ERROR_SCHEMA_VERSION: u32 = 1;

/// Machine-readable canonical error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidUri,
    InvalidId,
    SchemaViolation,
    UnsupportedSchemaVersion,
    NotFound,
    FactDirty,
    FactStale,
    AssuranceNotSatisfied,
    ExecutionFailed,
    SpawnFailed,
    TimedOut,
    Cancelled,
    ContainmentFailed,
    IoFailed,
    Refusal,
    Internal,
    AuthorityRequired,
    AuthorityContractInvalid,
    AuthorityContractMismatch,
    PlanChanged,
    SemanticHintMismatch,
    ProviderFailure,
    Timeout,
    UnknownCapability,
    Unsupported,
    PersistenceFailure,
    InvalidMcpRequest,
    #[serde(rename = "OMEN_CONFIG_VERSION_UNSUPPORTED")]
    ConfigVersionUnsupported,
    #[serde(rename = "OMEN_CONFIG_TOO_MANY_ACTIONS")]
    ConfigTooManyActions,
    #[serde(rename = "OMEN_CONFIG_TOO_MANY_STEPS")]
    ConfigTooManySteps,
    #[serde(rename = "OMEN_CONFIG_BOUNDS")]
    ConfigBounds,
    InvalidActionId,
    InvalidStepId,
    DuplicateStepId,
    SelfStepReference,
    RuntimeStatusUnavailable,
    InputFieldNotFound,
    TypeCompatibilityUnknown,
    TypeMismatch,
    ForwardStepReference,
    StepReferenceNotFound,
    OutputFieldNotFound,
    ActionNotFound,
    CapabilityExecutionFailed,
    CapabilityExecutionUnsupported,
    CapabilityOutputSchemaViolation,
    CapabilityUnavailable,
    ContractSemanticsUnsupported,
    EvidenceSerializationFailed,
    NestedCompositionUnsupported,
    NetworkConstraintUnenforceable,
    ResultBoundsExceeded,
    StepFailed,
    StepInputSchemaViolation,
    StepOutputMissing,
    WorkspaceScopeViolation,
    DeltaUnavailable,
    ExecutionStateUnavailable,
    UnjournaledLocalExecution,
    CapabilityNotFound,
    RecipeNotFound,
    #[serde(rename = "OMEN_CONFIG_INVALID")]
    ConfigInvalid,
    #[serde(rename = "OMEN_CONFIG_TOO_LARGE")]
    ConfigTooLarge,
    #[serde(rename = "OMEN_CONFIG_PATH_ESCAPE")]
    ConfigPathEscape,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidUri => "INVALID_URI",
            Self::InvalidId => "INVALID_ID",
            Self::SchemaViolation => "SCHEMA_VIOLATION",
            Self::UnsupportedSchemaVersion => "UNSUPPORTED_SCHEMA_VERSION",
            Self::NotFound => "NOT_FOUND",
            Self::FactDirty => "FACT_DIRTY",
            Self::FactStale => "FACT_STALE",
            Self::AssuranceNotSatisfied => "ASSURANCE_NOT_SATISFIED",
            Self::ExecutionFailed => "EXECUTION_FAILED",
            Self::SpawnFailed => "SPAWN_FAILED",
            Self::TimedOut => "TIMED_OUT",
            Self::Cancelled => "CANCELLED",
            Self::ContainmentFailed => "CONTAINMENT_FAILED",
            Self::IoFailed => "IO_FAILED",
            Self::Refusal => "REFUSAL",
            Self::Internal => "INTERNAL",
            Self::AuthorityRequired => "AUTHORITY_REQUIRED",
            Self::AuthorityContractInvalid => "AUTHORITY_CONTRACT_INVALID",
            Self::AuthorityContractMismatch => "AUTHORITY_CONTRACT_MISMATCH",
            Self::PlanChanged => "PLAN_CHANGED",
            Self::SemanticHintMismatch => "SEMANTIC_HINT_MISMATCH",
            Self::ProviderFailure => "PROVIDER_FAILURE",
            Self::Timeout => "TIMEOUT",
            Self::UnknownCapability => "UNKNOWN_CAPABILITY",
            Self::Unsupported => "UNSUPPORTED",
            Self::PersistenceFailure => "PERSISTENCE_FAILURE",
            Self::InvalidMcpRequest => "INVALID_MCP_REQUEST",
            Self::ConfigVersionUnsupported => "OMEN_CONFIG_VERSION_UNSUPPORTED",
            Self::ConfigTooManyActions => "OMEN_CONFIG_TOO_MANY_ACTIONS",
            Self::ConfigTooManySteps => "OMEN_CONFIG_TOO_MANY_STEPS",
            Self::ConfigBounds => "OMEN_CONFIG_BOUNDS",
            Self::InvalidActionId => "INVALID_ACTION_ID",
            Self::InvalidStepId => "INVALID_STEP_ID",
            Self::DuplicateStepId => "DUPLICATE_STEP_ID",
            Self::SelfStepReference => "SELF_STEP_REFERENCE",
            Self::RuntimeStatusUnavailable => "RUNTIME_STATUS_UNAVAILABLE",
            Self::InputFieldNotFound => "INPUT_FIELD_NOT_FOUND",
            Self::TypeCompatibilityUnknown => "TYPE_COMPATIBILITY_UNKNOWN",
            Self::TypeMismatch => "TYPE_MISMATCH",
            Self::ForwardStepReference => "FORWARD_STEP_REFERENCE",
            Self::StepReferenceNotFound => "STEP_REFERENCE_NOT_FOUND",
            Self::OutputFieldNotFound => "OUTPUT_FIELD_NOT_FOUND",
            Self::ActionNotFound => "ACTION_NOT_FOUND",
            Self::CapabilityExecutionFailed => "CAPABILITY_EXECUTION_FAILED",
            Self::CapabilityExecutionUnsupported => "CAPABILITY_EXECUTION_UNSUPPORTED",
            Self::CapabilityOutputSchemaViolation => "CAPABILITY_OUTPUT_SCHEMA_VIOLATION",
            Self::CapabilityUnavailable => "CAPABILITY_UNAVAILABLE",
            Self::ContractSemanticsUnsupported => "CONTRACT_SEMANTICS_UNSUPPORTED",
            Self::EvidenceSerializationFailed => "EVIDENCE_SERIALIZATION_FAILED",
            Self::NestedCompositionUnsupported => "NESTED_COMPOSITION_UNSUPPORTED",
            Self::NetworkConstraintUnenforceable => "NETWORK_CONSTRAINT_UNENFORCEABLE",
            Self::ResultBoundsExceeded => "RESULT_BOUNDS_EXCEEDED",
            Self::StepFailed => "STEP_FAILED",
            Self::StepInputSchemaViolation => "STEP_INPUT_SCHEMA_VIOLATION",
            Self::StepOutputMissing => "STEP_OUTPUT_MISSING",
            Self::WorkspaceScopeViolation => "WORKSPACE_SCOPE_VIOLATION",
            Self::DeltaUnavailable => "DELTA_UNAVAILABLE",
            Self::ExecutionStateUnavailable => "EXECUTION_STATE_UNAVAILABLE",
            Self::UnjournaledLocalExecution => "UNJOURNALED_LOCAL_EXECUTION",
            Self::CapabilityNotFound => "CAPABILITY_NOT_FOUND",
            Self::RecipeNotFound => "RECIPE_NOT_FOUND",
            Self::ConfigInvalid => "OMEN_CONFIG_INVALID",
            Self::ConfigTooLarge => "OMEN_CONFIG_TOO_LARGE",
            Self::ConfigPathEscape => "OMEN_CONFIG_PATH_ESCAPE",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|code| code.as_str() == value)
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::InvalidUri,
            Self::InvalidId,
            Self::SchemaViolation,
            Self::UnsupportedSchemaVersion,
            Self::NotFound,
            Self::FactDirty,
            Self::FactStale,
            Self::AssuranceNotSatisfied,
            Self::ExecutionFailed,
            Self::SpawnFailed,
            Self::TimedOut,
            Self::Cancelled,
            Self::ContainmentFailed,
            Self::IoFailed,
            Self::Refusal,
            Self::Internal,
            Self::AuthorityRequired,
            Self::AuthorityContractInvalid,
            Self::AuthorityContractMismatch,
            Self::PlanChanged,
            Self::SemanticHintMismatch,
            Self::ProviderFailure,
            Self::Timeout,
            Self::UnknownCapability,
            Self::Unsupported,
            Self::PersistenceFailure,
            Self::InvalidMcpRequest,
            Self::ConfigVersionUnsupported,
            Self::ConfigTooManyActions,
            Self::ConfigTooManySteps,
            Self::ConfigBounds,
            Self::InvalidActionId,
            Self::InvalidStepId,
            Self::DuplicateStepId,
            Self::SelfStepReference,
            Self::RuntimeStatusUnavailable,
            Self::InputFieldNotFound,
            Self::TypeCompatibilityUnknown,
            Self::TypeMismatch,
            Self::ForwardStepReference,
            Self::StepReferenceNotFound,
            Self::OutputFieldNotFound,
            Self::ActionNotFound,
            Self::CapabilityExecutionFailed,
            Self::CapabilityExecutionUnsupported,
            Self::CapabilityOutputSchemaViolation,
            Self::CapabilityUnavailable,
            Self::ContractSemanticsUnsupported,
            Self::EvidenceSerializationFailed,
            Self::NestedCompositionUnsupported,
            Self::NetworkConstraintUnenforceable,
            Self::ResultBoundsExceeded,
            Self::StepFailed,
            Self::StepInputSchemaViolation,
            Self::StepOutputMissing,
            Self::WorkspaceScopeViolation,
            Self::DeltaUnavailable,
            Self::ExecutionStateUnavailable,
            Self::UnjournaledLocalExecution,
            Self::CapabilityNotFound,
            Self::RecipeNotFound,
            Self::ConfigInvalid,
            Self::ConfigTooLarge,
            Self::ConfigPathEscape,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCategory {
    Authority,
    Validation,
    Semantic,
    Execution,
    Resource,
    Evidence,
    Persistence,
    Protocol,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Retryability {
    Never,
    AfterChange,
    Transient,
    Conditional,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ErrorEvidence {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<crate::ResourceUri>,
}

/// The only Omen domain-error wire model.  JSON-RPC protocol errors remain
/// separate and are represented by the MCP protocol types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OmenError {
    pub schema_version: u32,
    pub code: ErrorCode,
    pub message: String,
    pub category: ErrorCategory,
    pub state_changed: crate::composition::StateChange,
    pub retryability: Retryability,
    pub evidence: ErrorEvidence,
    #[serde(default)]
    pub details: Value,
}

impl OmenError {
    pub fn from_code(code: ErrorCode, message: impl Into<String>) -> Self {
        let category = match code {
            ErrorCode::AuthorityRequired
            | ErrorCode::AuthorityContractInvalid
            | ErrorCode::AuthorityContractMismatch
            | ErrorCode::Refusal => ErrorCategory::Authority,
            ErrorCode::InvalidUri
            | ErrorCode::InvalidId
            | ErrorCode::SchemaViolation
            | ErrorCode::UnsupportedSchemaVersion
            | ErrorCode::InvalidMcpRequest
            | ErrorCode::ConfigVersionUnsupported
            | ErrorCode::ConfigTooManyActions
            | ErrorCode::ConfigTooManySteps
            | ErrorCode::ConfigBounds
            | ErrorCode::InvalidActionId
            | ErrorCode::InvalidStepId
            | ErrorCode::DuplicateStepId
            | ErrorCode::SelfStepReference
            | ErrorCode::InputFieldNotFound
            | ErrorCode::TypeCompatibilityUnknown
            | ErrorCode::TypeMismatch
            | ErrorCode::ForwardStepReference
            | ErrorCode::StepReferenceNotFound
            | ErrorCode::OutputFieldNotFound
            | ErrorCode::PlanChanged
            | ErrorCode::ConfigInvalid
            | ErrorCode::ConfigTooLarge
            | ErrorCode::ConfigPathEscape => ErrorCategory::Validation,
            ErrorCode::SemanticHintMismatch
            | ErrorCode::ProviderFailure
            | ErrorCode::Unsupported => ErrorCategory::Semantic,
            ErrorCode::PersistenceFailure | ErrorCode::ExecutionStateUnavailable => {
                ErrorCategory::Persistence
            }
            ErrorCode::EvidenceSerializationFailed => ErrorCategory::Evidence,
            ErrorCode::NotFound
            | ErrorCode::FactDirty
            | ErrorCode::FactStale
            | ErrorCode::AssuranceNotSatisfied
            | ErrorCode::CapabilityUnavailable
            | ErrorCode::CapabilityNotFound
            | ErrorCode::RecipeNotFound
            | ErrorCode::DeltaUnavailable => ErrorCategory::Resource,
            _ => ErrorCategory::Execution,
        };
        let retryability = match code {
            ErrorCode::PlanChanged
            | ErrorCode::FactDirty
            | ErrorCode::FactStale
            | ErrorCode::AuthorityContractMismatch => Retryability::AfterChange,
            ErrorCode::Timeout | ErrorCode::TimedOut | ErrorCode::ProviderFailure => {
                Retryability::Transient
            }
            ErrorCode::AuthorityRequired | ErrorCode::CapabilityUnavailable => {
                Retryability::Conditional
            }
            ErrorCode::InvalidMcpRequest | ErrorCode::NotFound | ErrorCode::Unsupported => {
                Retryability::Never
            }
            _ => Retryability::Unknown,
        };
        Self::new(
            code,
            message,
            category,
            crate::composition::StateChange::No,
            retryability,
        )
    }

    pub fn new(
        code: ErrorCode,
        message: impl Into<String>,
        category: ErrorCategory,
        state_changed: crate::composition::StateChange,
        retryability: Retryability,
    ) -> Self {
        Self {
            schema_version: ERROR_SCHEMA_VERSION,
            code,
            message: message.into(),
            category,
            state_changed,
            retryability,
            evidence: ErrorEvidence::default(),
            details: json!({}),
        }
    }

    pub fn from_core(error: &CoreError) -> Self {
        let code = error.code();
        Self::from_code(code, error.to_string())
    }
}

/// Canonical core error representations.
#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoreError {
    #[error("Invalid resource URI: {0}")]
    InvalidUri(String),

    #[error("Invalid identifier: {0}")]
    InvalidId(String),

    #[error("Schema violation: {0}")]
    SchemaViolation(String),

    #[error("Unsupported schema version '{version}' (expected '{expected}')")]
    UnsupportedSchemaVersion { version: String, expected: String },

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Fact is dirty and explicit revalidation is required: {0}")]
    FactDirty(String),

    #[error("Fact is stale: {0}")]
    FactStale(String),

    #[error("Required assurance level '{required}' cannot be satisfied (available: '{available}')")]
    AssuranceNotSatisfied { required: String, available: String },

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Execution failed: {message}")]
    ExecutionFailedCode { code: ErrorCode, message: String },

    #[error("Internal error: {0}")]
    Internal(String),
}

impl CoreError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidUri(_) => ErrorCode::InvalidUri,
            Self::InvalidId(_) => ErrorCode::InvalidId,
            Self::SchemaViolation(_) => ErrorCode::SchemaViolation,
            Self::UnsupportedSchemaVersion { .. } => ErrorCode::UnsupportedSchemaVersion,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::FactDirty(_) => ErrorCode::FactDirty,
            Self::FactStale(_) => ErrorCode::FactStale,
            Self::AssuranceNotSatisfied { .. } => ErrorCode::AssuranceNotSatisfied,
            Self::ExecutionFailed(_) => ErrorCode::ExecutionFailed,
            Self::ExecutionFailedCode { code, .. } => *code,
            Self::Internal(_) => ErrorCode::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_error_envelope_is_versioned_and_bounded() {
        let mut error = OmenError::from_code(ErrorCode::PlanChanged, "plan changed");
        error.details = json!({"expected": "old", "actual": "new"});
        let value = serde_json::to_value(error).unwrap();
        assert_eq!(value["schema_version"], ERROR_SCHEMA_VERSION);
        assert_eq!(value["code"], "PLAN_CHANGED");
        assert_eq!(value["category"], "VALIDATION");
        assert_eq!(value["state_changed"], "NO");
        assert_eq!(value["retryability"], "AFTER_CHANGE");
        assert!(value["evidence"]["available"].is_boolean());
        assert!(value["details"].is_object());
    }

    #[test]
    fn error_code_registry_round_trips_public_names() {
        for code in ErrorCode::all() {
            assert_eq!(ErrorCode::parse(code.as_str()), Some(*code));
            assert_eq!(serde_json::to_value(code).unwrap(), code.as_str());
        }
    }

    #[test]
    fn composition_errors_project_the_same_envelope() {
        let error = crate::composition::ActionExecutionError {
            code: "TIMEOUT".into(),
            message: "capability timed out".into(),
        };
        let value = serde_json::to_value(error).unwrap();
        assert_eq!(value["code"], "TIMEOUT");
        assert_eq!(value["error"]["code"], "TIMEOUT");
        assert_eq!(value["error"]["schema_version"], ERROR_SCHEMA_VERSION);
    }
}
