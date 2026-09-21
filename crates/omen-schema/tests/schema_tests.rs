use omen_core::{
    ActionId, AdapterClassification, Assurance, CoreError, EnforcementLevel, EnforcementReport,
    ExecutionConstraints, ExecutionContract, ExecutionId, ExecutionResult, Intent, LeaseRequest,
    LeaseRights, ProcessExit, RequiredAssurance, ResourceId, ResourceUri, RuntimeStatus,
    StdioConfig, StdioMode,
};
use omen_schema::{
    ExecutionContractWire, ExecutionResultWire, SCHEMA_VERSION_EXECUTION, SCHEMA_VERSION_RESULT,
};

#[test]
fn roundtrip_execution_contract() {
    let contract = ExecutionContract {
        execution_id: ExecutionId::new("tethers://exec/01K9F82A").unwrap(),
        actor: ResourceUri::parse("actor://agent/codex/session-42").unwrap(),
        intent: Intent {
            tool: ResourceUri::parse("tool://cargo").unwrap(),
            operation: "test".into(),
            args: vec!["--test".into(), "auth_suite".into()],
        },
        leases: vec![
            LeaseRequest {
                resource: ResourceUri::parse("workspace://").unwrap(),
                rights: vec![LeaseRights::Read],
                native_access: true,
            },
            LeaseRequest {
                resource: ResourceUri::parse("workspace://target").unwrap(),
                rights: vec![LeaseRights::Read, LeaseRights::Write],
                native_access: true,
            },
        ],
        stdio: StdioConfig {
            stdin: StdioMode::Closed,
            stdout: StdioMode::Inline,
            stderr: StdioMode::Inline,
        },
        constraints: ExecutionConstraints {
            timeout_ms: 30000,
            network_denied: true,
        },
        required_assurance: RequiredAssurance {
            filesystem: Some(Assurance::Enforced),
            network: Some(Assurance::Enforced),
            descendants: Some(Assurance::Enforced),
        },
    };

    let wire = ExecutionContractWire::from(&contract);
    let serialized = serde_json::to_string_pretty(&wire).unwrap();
    let deserialized_wire: ExecutionContractWire = serde_json::from_str(&serialized).unwrap();
    let recovered_contract: ExecutionContract = deserialized_wire.try_into().unwrap();

    assert_eq!(contract, recovered_contract);
}

#[test]
fn roundtrip_execution_result() {
    let result = ExecutionResult {
        execution_id: ExecutionId::new("tethers://exec/01K9F82A").unwrap(),
        external_reference: None,
        action_id: ActionId::new("omen://action/01K9F82B").unwrap(),
        runtime_status: RuntimeStatus::Completed,
        process_exit: ProcessExit {
            code: Some(101),
            signal: None,
        },
        adapter_classification: AdapterClassification::Failure,
        enforcement: EnforcementReport {
            filesystem: EnforcementLevel::Enforced,
            network: EnforcementLevel::Enforced,
            descendant_processes: EnforcementLevel::Enforced,
            symlink_escape: EnforcementLevel::Enforced,
        },
        observations: vec!["test timeout warning".into()],
        fact_updates: vec![ResourceId::new("fact://test/status").unwrap()],
        artifacts: vec![ResourceUri::parse(
            "artifact://sha256/ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .unwrap()],
        reduced_summary: "Auth test failed with assertion mismatch".into(),
    };

    let wire = ExecutionResultWire::from(&result);
    let serialized = serde_json::to_string_pretty(&wire).unwrap();
    let deserialized_wire: ExecutionResultWire = serde_json::from_str(&serialized).unwrap();
    let recovered_result: ExecutionResult = deserialized_wire.try_into().unwrap();

    assert_eq!(result, recovered_result);
}

#[test]
fn reject_unsupported_schema_version() {
    let json = r#"{
        "schema_version": "omen.execution/999.0",
        "execution_id": "exec-1",
        "actor": "actor://test",
        "intent": { "tool": "tool://cargo", "operation": "build", "args": [] },
        "leases": [],
        "stdio": { "stdin": "closed", "stdout": "capture", "stderr": "capture" },
        "constraints": { "timeout_ms": 1000 },
        "required_assurance": {}
    }"#;

    let wire: ExecutionContractWire = serde_json::from_str(json).unwrap();
    let res: Result<ExecutionContract, CoreError> = wire.try_into();
    match res {
        Err(CoreError::UnsupportedSchemaVersion { version, expected }) => {
            assert_eq!(version, "omen.execution/999.0");
            assert_eq!(expected, SCHEMA_VERSION_EXECUTION);
        }
        other => panic!("Expected UnsupportedSchemaVersion, got {:?}", other),
    }

    let result_json = r#"{
        "schema_version": "omen.result/0.1",
        "execution_id": "exec-1",
        "action_id": "act-1",
        "runtime_status": "COMPLETED",
        "process_exit": { "code": 0 },
        "adapter_classification": "SUCCESS",
        "enforcement": {
            "filesystem": "OBSERVED",
            "network": "OBSERVED",
            "descendant_processes": "OBSERVED",
            "symlink_escape": "OBSERVED"
        },
        "observations": [],
        "fact_updates": [],
        "artifacts": [],
        "reduced_summary": "ok"
    }"#;

    let res_wire: ExecutionResultWire = serde_json::from_str(result_json).unwrap();
    let res_domain: Result<ExecutionResult, CoreError> = res_wire.try_into();
    match res_domain {
        Err(CoreError::UnsupportedSchemaVersion { version, expected }) => {
            assert_eq!(version, "omen.result/0.1");
            assert_eq!(expected, SCHEMA_VERSION_RESULT);
        }
        other => panic!("Expected UnsupportedSchemaVersion, got {:?}", other),
    }
}

#[test]
fn reject_unknown_fields_in_wire() {
    let json = r#"{
        "schema_version": "omen.execution/0.2",
        "execution_id": "exec-1",
        "actor": "actor://test",
        "intent": { "tool": "tool://cargo", "operation": "build", "args": [] },
        "leases": [],
        "stdio": { "stdin": "closed", "stdout": "capture", "stderr": "capture" },
        "constraints": { "timeout_ms": 1000 },
        "required_assurance": {},
        "unexpected_field": "intruder"
    }"#;

    let res: Result<ExecutionContractWire, _> = serde_json::from_str(json);
    assert!(res.is_err(), "Must reject unknown fields");
}

#[test]
fn reject_traversal_in_wire_resource() {
    let json = r#"{
        "schema_version": "omen.execution/0.2",
        "execution_id": "exec-1",
        "actor": "actor://test",
        "intent": { "tool": "tool://cargo", "operation": "build", "args": [] },
        "leases": [
            { "resource": "workspace://../../etc/shadow", "rights": ["read"], "native_access": true }
        ],
        "stdio": { "stdin": "closed", "stdout": "capture", "stderr": "capture" },
        "constraints": { "timeout_ms": 1000 },
        "required_assurance": {}
    }"#;

    let wire: ExecutionContractWire = serde_json::from_str(json).unwrap();
    let res: Result<ExecutionContract, CoreError> = wire.try_into();
    assert!(matches!(res, Err(CoreError::InvalidUri(_))));
}
