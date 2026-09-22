use serde_json::Value;
use std::process::Command;
use tempfile::tempdir;

fn run(args: &[&str]) -> Value {
    let workspace = tempdir().expect("temporary workspace");
    let output = Command::new(env!("CARGO_BIN_EXE_omen"))
        .args(args)
        .arg("--workspace")
        .arg(workspace.path())
        .output()
        .expect("run omen machine surface");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("machine output is JSON")
}

#[test]
fn unknown_digest_does_not_fake_a_contract_delta() {
    let value = run(&["orient", "--machine", "--since", "sha256:unknown"]);
    assert_eq!(value["changed"], true);
    assert_eq!(value["delta_available"], false);
    assert!(value.get("capabilities_added").is_none());
    assert_eq!(value["next_actions"][0]["operation"], "orient");
}

#[test]
fn discovery_status_is_runtime_unknown_and_not_static_contract_truth() {
    let value = run(&["capabilities", "semantic", "--machine"]);
    let entries = value["capabilities"].as_array().expect("capability array");
    assert!(!entries.is_empty());
    for entry in entries {
        assert_eq!(entry["availability"], "unknown");
        assert_eq!(entry["admission"], "unknown");
        assert!(entry["describe_ref"].is_string());
        assert!(entry["summary"].is_string());
        assert!(entry.get("input_schema").is_none());
    }
}

#[test]
fn orient_digest_match_is_a_bounded_no_delta_response() {
    let initial = run(&["orient", "--machine"]);
    let digest = initial["contract_digest"].as_str().unwrap();
    let matched = run(&["orient", "--machine", "--since", digest]);
    assert_eq!(
        matched,
        serde_json::json!({"changed": false, "contract_digest": digest})
    );
}


#[test]
fn capability_catalogue_exposes_real_invocation_routes() {
    let history = run(&["capabilities", "history", "--machine"]);
    let history_entry = &history["capabilities"][0];
    assert_eq!(history_entry["id"], "history.query");
    assert_eq!(history_entry["invocation"]["cli"], "history --machine");
    assert_eq!(
        history_entry["invocation"]["mcp_tool"],
        "omen_history_query"
    );

    let semantic = run(&["capabilities", "semantic", "--machine"]);
    let definition = semantic["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "semantic.definition")
        .unwrap();
    assert!(definition["invocation"]["cli"].is_null());
    assert_eq!(
        definition["invocation"]["mcp_tool"],
        "omen_symbol_definition"
    );

    let diagnostics = semantic["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "semantic.diagnostics")
        .unwrap();
    assert!(diagnostics["invocation"]["cli"].is_null());
    assert!(diagnostics["invocation"]["mcp_tool"].is_null());
}
