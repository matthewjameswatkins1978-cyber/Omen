use omen_knowledge::{Database, FactRegistry, deterministic_workspace_id};
use serde_json::Value;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn run(workspace: &std::path::Path, state_home: &std::path::Path, args: &[&str]) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_omen"))
        .args(args)
        .arg("--workspace")
        .arg(workspace)
        .env("OMEN_STATE_HOME", state_home)
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
    let state_home = workspace.path().join("omen-state");
    let (ok, absent) = run(
        workspace.path(),
        &state_home,
        &["action", "list", "--machine"],
    );
    assert!(ok);
    assert_eq!(absent["config_present"], false);

    fs::write(workspace.path().join("Omen.toml"), VALID_CONFIG).unwrap();
    let (ok, plan) = run(
        workspace.path(),
        &state_home,
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
fn machine_discovery_path_is_cold_start_learnable() {
    let workspace = tempdir().unwrap();
    let state_home = workspace.path().join("omen-state");

    let (ok, orient) = run(workspace.path(), &state_home, &["orient", "--machine"]);
    assert!(ok);
    assert_eq!(orient["contract_version"], "0.8");
    assert!(orient["next"].as_array().unwrap().len() >= 3);

    let (ok, capabilities) = run(
        workspace.path(),
        &state_home,
        &["capabilities", "mutation", "--machine"],
    );
    assert!(ok);
    assert_eq!(capabilities["capabilities"].as_array().unwrap().len(), 1);
    assert_eq!(
        capabilities["capabilities"][0]["definition"]["id"],
        "mutation.threadmoth"
    );

    let (ok, described) = run(
        workspace.path(),
        &state_home,
        &["describe", "mutation.threadmoth", "--machine"],
    );
    assert!(ok);
    assert_eq!(described["definition"]["authority"], "Tethers admission");

    let (ok, recipe) = run(
        workspace.path(),
        &state_home,
        &["how", "safe-mutation", "--machine"],
    );
    assert!(ok);
    assert_eq!(recipe["id"], "safe-mutation");

    let (ok, context) = run(workspace.path(), &state_home, &["context", "--machine"]);
    assert!(ok);
    assert_eq!(context["delta"], "CURRENT_SNAPSHOT");
}

#[test]
fn strict_config_rejects_unknown_fields_and_unknown_capabilities() {
    let workspace = tempdir().unwrap();
    fs::write(
        workspace.path().join("Omen.toml"),
        "schema_version = 1\nunknown = true\n",
    )
    .unwrap();
    let state_home = workspace.path().join("omen-state");
    let (ok, error) = run(
        workspace.path(),
        &state_home,
        &["action", "list", "--machine"],
    );
    assert!(!ok);
    assert_eq!(error["error"], "OMEN_CONFIG_INVALID");

    fs::write(
        workspace.path().join("Omen.toml"),
        VALID_CONFIG.replace("semantic.definition", "make.everything.awesome"),
    )
    .unwrap();
    let (ok, error) = run(
        workspace.path(),
        &state_home,
        &["action", "plan", "inspect-auth", "--machine"],
    );
    assert!(!ok);
    assert_eq!(error["error"], "UNKNOWN_CAPABILITY");
}

#[test]
fn wrong_literal_type_refuses_before_any_execution() {
    let workspace = tempdir().unwrap();
    let state_home = workspace.path().join("omen-state");
    let invalid = VALID_CONFIG.replace("value = \"SessionToken\"", "value = [\"SessionToken\"]");
    fs::write(workspace.path().join("Omen.toml"), invalid).unwrap();
    let (ok, error) = run(
        workspace.path(),
        &state_home,
        &["action", "plan", "inspect-auth", "--machine"],
    );
    assert!(!ok);
    assert_eq!(error["error"], "TYPE_MISMATCH");
}

#[test]
fn shell_and_environment_looking_values_remain_literal() {
    let workspace = tempdir().unwrap();
    let state_home = workspace.path().join("omen-state");
    let config = VALID_CONFIG.replace("SessionToken", "$(echo should-not-run) ${HOME}");
    fs::write(workspace.path().join("Omen.toml"), config).unwrap();
    let (ok, plan) = run(
        workspace.path(),
        &state_home,
        &["action", "plan", "inspect-auth", "--machine"],
    );
    assert!(ok);
    assert_eq!(
        plan["steps"][0]["inputs"]["symbol"]["literal"]["value"],
        "$(echo should-not-run) ${HOME}"
    );
}

#[test]
fn listing_showing_and_planning_do_not_create_any_omen_state() {
    let workspace = tempdir().unwrap();
    let state_home = workspace.path().join("omen-state");
    fs::write(workspace.path().join("Omen.toml"), VALID_CONFIG).unwrap();
    let before = fs::read_dir(workspace.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();

    for args in [
        vec!["action", "list", "--machine"],
        vec!["action", "show", "inspect-auth", "--machine"],
        vec!["action", "plan", "inspect-auth", "--machine"],
    ] {
        let (ok, _) = run(workspace.path(), &state_home, &args);
        assert!(ok, "command failed: {args:?}");
    }

    assert!(!state_home.exists(), "planning created Omen state");
    assert!(!workspace.path().join("state.sqlite").exists());
    assert!(!workspace.path().join("state.sqlite-wal").exists());
    assert!(!workspace.path().join("state.sqlite-shm").exists());
    assert!(!workspace.path().join("cas").exists());
    let after = fs::read_dir(workspace.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(before, after, "planning mutated the workspace directory");
}

#[test]
fn planning_reads_existing_generation_without_writing_state() {
    let workspace = tempdir().unwrap();
    let state_home = workspace.path().join("omen-state");
    fs::write(workspace.path().join("Omen.toml"), VALID_CONFIG).unwrap();

    let state_dir = state_home
        .join("workspaces")
        .join(deterministic_workspace_id(workspace.path()));
    let db_path = state_dir.join("state.sqlite");
    {
        let mut db = Database::open(&db_path).unwrap();
        FactRegistry::set_generation(&mut db, "fs:workspace", 7).unwrap();
    }
    let before = fs::read(&db_path).unwrap();
    let before_entries = fs::read_dir(&state_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();

    let (ok, plan) = run(
        workspace.path(),
        &state_home,
        &["action", "plan", "inspect-auth", "--machine"],
    );
    assert!(ok);
    assert_eq!(plan["context_generation"], 7);
    assert_eq!(fs::read(&db_path).unwrap(), before);
    let after_entries = fs::read_dir(&state_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(before_entries, after_entries);
    assert!(!state_dir.join("state.sqlite-wal").exists());
    assert!(!state_dir.join("state.sqlite-shm").exists());
}

#[test]
fn config_bounds_version_and_authority_looking_fields_refuse() {
    let workspace = tempdir().unwrap();
    let state_home = workspace.path().join("omen-state");

    fs::write(
        workspace.path().join("Omen.toml"),
        format!("schema_version = 2\n{}", " ".repeat(32)),
    )
    .unwrap();
    let (ok, error) = run(
        workspace.path(),
        &state_home,
        &["action", "list", "--machine"],
    );
    assert!(!ok);
    assert_eq!(error["error"], "OMEN_CONFIG_INVALID");

    fs::write(
        workspace.path().join("Omen.toml"),
        "schema_version = 1\n[policy]\nallow = true\n",
    )
    .unwrap();
    let (ok, error) = run(
        workspace.path(),
        &state_home,
        &["action", "list", "--machine"],
    );
    assert!(!ok);
    assert_eq!(error["error"], "OMEN_CONFIG_INVALID");

    fs::write(
        workspace.path().join("Omen.toml"),
        "x".repeat(256 * 1024 + 1),
    )
    .unwrap();
    let (ok, error) = run(
        workspace.path(),
        &state_home,
        &["action", "list", "--machine"],
    );
    assert!(!ok);
    assert_eq!(error["error"], "OMEN_CONFIG_TOO_LARGE");
}

#[cfg(windows)]
#[test]
fn escaping_omen_config_symlink_refuses() {
    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let state_home = workspace.path().join("omen-state");
    fs::write(outside.path().join("Omen.toml"), VALID_CONFIG).unwrap();
    if std::os::windows::fs::symlink_file(
        outside.path().join("Omen.toml"),
        workspace.path().join("Omen.toml"),
    )
    .is_err()
    {
        return;
    }
    let (ok, error) = run(
        workspace.path(),
        &state_home,
        &["action", "list", "--machine"],
    );
    assert!(!ok);
    assert_eq!(error["error"], "OMEN_CONFIG_PATH_ESCAPE");
}
