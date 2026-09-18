use omen_core::{ActionId, ExecutionContract};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use omen_knowledge::{ContentAddressedStore, Database};
use omen_schema::{ExecutionContractWire, ExecutionResultWire, SCHEMA_VERSION_RESULT};
use tempfile::tempdir;

#[tokio::test]
async fn test_tethers_contract_boundary_execution() {
    // 1. Construct a valid Tethers Execution Contract wire JSON
    // With external Tethers execution identity, actor, leases, constraints, and required assurance
    let contract_json = r#"{
        "schema_version": "omen.execution/0.2",
        "execution_id": "exec-7f8e9d0a1b2c",
        "actor": "actor://tethers/agent/ci-builder",
        "intent": {
            "tool": "tool://rust/cargo",
            "operation": "version",
            "args": ["--version"]
        },
        "leases": [
            {
                "resource": "workspace:///tmp/build",
                "rights": ["read", "write"],
                "native_access": true
            }
        ],
        "stdio": {
            "stdin": "closed",
            "stdout": "inline",
            "stderr": "inline"
        },
        "constraints": {
            "timeout_ms": 10000,
            "network": "allow"
        },
        "required_assurance": {
            "filesystem": "observed",
            "network": "observed",
            "descendants": "observed"
        }
    }"#;

    // 2. Parse wire schema into strongly typed ExecutionContract
    let wire: ExecutionContractWire =
        serde_json::from_str(contract_json).expect("Contract JSON should parse");
    let contract: ExecutionContract =
        ExecutionContract::try_from(wire).expect("Should convert to domain ExecutionContract");

    // Verify boundaries: Omen receives external identity and actor
    assert_eq!(contract.execution_id.as_str(), "exec-7f8e9d0a1b2c");
    assert_eq!(contract.actor.as_str(), "actor://tethers/agent/ci-builder");
    assert_eq!(contract.intent.tool.as_str(), "tool://rust/cargo");

    // 3. Execute through Omen ProcessSupervisor
    let supervisor = ProcessSupervisor::new();
    let temp_dir = tempdir().unwrap();
    let mut db = Database::open(&temp_dir.path().join("state.sqlite")).unwrap();
    let cas = ContentAddressedStore::new(temp_dir.path().join("cas"));

    let req = ExecutionRequest {
        argv: vec!["cargo".into(), "--version".into()],
        cwd: temp_dir.path().to_path_buf(),
        env: vec![],
        stdin_mode: contract.stdio.stdin,
        stdin_payload: None,
        timeout_ms: contract.constraints.timeout_ms,
        inline_budget: 4096,
        required_assurance: contract.required_assurance,
    };

    let exec_output = supervisor
        .execute(req)
        .await
        .expect("Execution should run to completion");

    // 4. Store complete unreduced transcript into CAS
    let artifact = cas
        .store(
            &mut db,
            &exec_output.stdout_all,
            "text/plain",
            "omen://execution/output",
            omen_core::RetentionClass::Referenced,
        )
        .expect("Artifact should store in CAS");

    // 5. Produce Omen ExecutionResult wire object
    // Demonstrating strict doctrine:
    // Omen reports physical runtime evidence, containment status, exit code, and artifacts.
    // Omen leaves durable policy, approval, and semantic provider outcome truth to Tethers.
    let backend_caps = supervisor.backend().capabilities();

    let result_wire = ExecutionResultWire {
        schema_version: SCHEMA_VERSION_RESULT.to_string(),
        execution_id: contract.execution_id.to_string(),
        action_id: ActionId::new("act-1234567890ab").unwrap().to_string(),
        runtime_status: "COMPLETED".to_string(),
        process_exit: omen_schema::ProcessExitWire {
            code: exec_output.process_exit.code,
            signal: None,
        },
        adapter_classification: "CLEAN_PASS".to_string(),
        enforcement: omen_schema::EnforcementReportWire {
            filesystem: format!("{:?}", backend_caps.filesystem).to_uppercase(),
            network: format!("{:?}", backend_caps.network).to_uppercase(),
            descendant_processes: format!("{:?}", backend_caps.descendants).to_uppercase(),
            symlink_escape: "OBSERVED".to_string(),
        },
        observations: vec!["cargo installed and functional".to_string()],
        fact_updates: vec![],
        artifacts: vec![artifact.uri.to_string()],
        reduced_summary: String::from_utf8_lossy(&exec_output.stdout_bounded).to_string(),
    };

    // Serialize to JSON and verify structure conforms to wire schema
    let serialized_result =
        serde_json::to_string_pretty(&result_wire).expect("Should serialize result wire");
    assert!(serialized_result.contains("omen.result/0.2"));
    assert!(serialized_result.contains("exec-7f8e9d0a1b2c"));
    assert!(serialized_result.contains("artifact://"));

    // Verify round-trip parsing of result
    let deserialized_res: ExecutionResultWire =
        serde_json::from_str(&serialized_result).expect("Should deserialize result wire");
    assert_eq!(deserialized_res.execution_id, "exec-7f8e9d0a1b2c");
    assert_eq!(deserialized_res.runtime_status, "COMPLETED");
}
