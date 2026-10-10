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
fn orient_carries_agent_guidance_and_shell_truth() {
    let value = run(&["orient", "--machine"]);
    let guidance = value["guidance"].as_array().expect("guidance array");
    assert!(!guidance.is_empty());
    let text = serde_json::to_string(guidance).unwrap();
    assert!(
        text.contains("typed routes"),
        "guidance prefers typed routes"
    );
    assert!(
        text.contains("ONE external argv"),
        "guidance states single-argv"
    );
    // shell.pipeline describe: no machine route, and the summary says so.
    let described = run(&["describe", "shell.pipeline", "--machine"]);
    let invocation = &described["definition"]["invocation"];
    assert_eq!(invocation["cli"], serde_json::Value::Null);
    assert_eq!(invocation["mcp_tool"], serde_json::Value::Null);
    assert_eq!(invocation["interactive"], serde_json::Value::Null);
    assert!(
        described["definition"]["summary"]
            .as_str()
            .unwrap()
            .starts_with("Interactive-shell"),
        "summary states its surface truth"
    );
}

#[test]
fn exec_refuses_pasted_shell_syntax_with_refusal_code() {
    let workspace = tempdir().expect("temporary workspace");
    for argv in [["git --version | sort"], [":history"]] {
        let mut args = vec!["exec", "--machine", "--workspace"];
        let dir = workspace.path().to_string_lossy().to_string();
        args.push(&dir);
        args.push("--");
        args.extend(argv);
        let output = Command::new(env!("CARGO_BIN_EXE_omen"))
            .args(&args)
            .output()
            .expect("run omen exec refusal");
        assert_eq!(output.status.code(), Some(2), "refusal exit code");
        let body: Value = serde_json::from_slice(&output.stdout).expect("refusal is JSON");
        assert_eq!(body["code"], "REFUSAL");
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .contains("one external argv"),
            "refusal names the supported alternative"
        );
    }
}
