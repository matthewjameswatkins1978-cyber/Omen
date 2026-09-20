use serde_json::Value;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn run(workspace: &std::path::Path, args: &[&str]) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_omen"))
        .args(args)
        .arg("--workspace")
        .arg(workspace)
        .output()
        .expect("run omen action surface");
    let value = serde_json::from_slice(&output.stdout).expect("machine output is JSON");
    (output.status.success(), value)
}

const VALID_CONFIG: &str = r#"
schema_version = 1

[project]
name = "Planning Fixture"

[actions.inspect-auth]
description = "Inspect the SessionToken definition."
[[actions.inspect-auth.steps]]
id = "definition"
capability = "semantic.definition"
input = { symbol = { kind = "literal", value = "SessionToken" } }
"#;

#[test]
fn optional_config_and_valid_plan_are_inert() {
    let workspace = tempdir().unwrap();
    let (ok, absent) = run(workspace.path(), &["action", "list", "--machine"]);
    assert!(ok);
    assert_eq!(absent["config_present"], false);

    fs::write(workspace.path().join("Omen.toml"), VALID_CONFIG).unwrap();
    let (ok, plan) = run(
        workspace.path(),
        &["action", "plan", "inspect-auth", "--machine"],
    );
    assert!(ok);
    assert_eq!(plan["planning_valid"], true);
    assert_eq!(plan["admission_snapshot_only"], true);
    assert_eq!(plan["steps"][0]["capability_id"], "semantic.definition");
    assert!(plan["plan_digest"].as_str().unwrap().starts_with("sha256:"));
    assert_eq!(
        plan["steps"][0]["inputs"]["symbol"]["literal"]["value"],
        "SessionToken"
    );
}

#[test]
fn strict_config_rejects_unknown_fields_and_unknown_capabilities() {
    let workspace = tempdir().unwrap();
    fs::write(
        workspace.path().join("Omen.toml"),
        "schema_version = 1\nunknown = true\n",
    )
    .unwrap();
    let (ok, error) = run(workspace.path(), &["action", "list", "--machine"]);
    assert!(!ok);
    assert_eq!(error["error"], "OMEN_CONFIG_INVALID");

    fs::write(
        workspace.path().join("Omen.toml"),
        VALID_CONFIG.replace("semantic.definition", "make.everything.awesome"),
    )
    .unwrap();
    let (ok, error) = run(
        workspace.path(),
        &["action", "plan", "inspect-auth", "--machine"],
    );
    assert!(!ok);
    assert_eq!(error["error"], "UNKNOWN_CAPABILITY");
}

#[test]
fn wrong_literal_type_refuses_before_any_execution() {
    let workspace = tempdir().unwrap();
    let invalid = VALID_CONFIG.replace("value = \"SessionToken\"", "value = [\"SessionToken\"]");
    fs::write(workspace.path().join("Omen.toml"), invalid).unwrap();
    let (ok, error) = run(
        workspace.path(),
        &["action", "plan", "inspect-auth", "--machine"],
    );
    assert!(!ok);
    assert_eq!(error["error"], "TYPE_MISMATCH");
}

#[test]
fn shell_and_environment_looking_values_remain_literal() {
    let workspace = tempdir().unwrap();
    let config = VALID_CONFIG.replace("SessionToken", "$(echo should-not-run) ${HOME}");
    fs::write(workspace.path().join("Omen.toml"), config).unwrap();
    let (ok, plan) = run(
        workspace.path(),
        &["action", "plan", "inspect-auth", "--machine"],
    );
    assert!(ok);
    assert_eq!(
        plan["steps"][0]["inputs"]["symbol"]["literal"]["value"],
        "$(echo should-not-run) ${HOME}"
    );
}
