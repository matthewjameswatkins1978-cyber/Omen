use serde_json::Value;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn run(
    workspace: &std::path::Path,
    state_home: &std::path::Path,
    args: &[String],
) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_omen"))
        .args(args)
        .arg("--workspace")
        .arg(workspace)
        .env("OMEN_STATE_HOME", state_home)
        .output()
        .expect("run omen action execution");
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "machine output is JSON: {e}; stderr={}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), value)
}

fn config(executable: &std::path::Path, arg: &str) -> String {
    format!(
        "schema_version = 1\n\n[actions.run-it]\n\n[[actions.run-it.steps]]\nid = \"process\"\ncapability = \"execution.run\"\ninput = {{ argv = {{ kind = \"literal\", value = ['{}', '{}'] }} }}\n",
        executable.display(),
        arg
    )
}

fn contract(path: &std::path::Path, args: Vec<String>, network: &str) {
    let wire = serde_json::json!({
        "schema_version": "omen.execution/0.2",
        "execution_id": "exec-composition-proof",
        "actor": "actor://tethers/composition-proof",
        "intent": {"tool":"tool://omen/self","operation":"run","args":args},
        "leases": [],
        "stdio": {"stdin":"closed","stdout":"inline","stderr":"inline"},
        "constraints": {"timeout_ms":10000,"network":network},
        "required_assurance": {}
    });
    fs::write(path, serde_json::to_vec_pretty(&wire).unwrap()).unwrap();
}

#[test]
fn real_cli_plan_digest_authority_and_execution_proof() {
    let workspace = tempdir().unwrap();
    let state_home = workspace.path().join("state");
    let executable = std::path::PathBuf::from(env!("CARGO_BIN_EXE_omen"));
    fs::write(
        workspace.path().join("Omen.toml"),
        config(&executable, "--version"),
    )
    .unwrap();

    let (ok, plan) = run(
        workspace.path(),
        &state_home,
        &[
            "action".into(),
            "plan".into(),
            "run-it".into(),
            "--machine".into(),
        ],
    );
    assert!(ok);
    let digest = plan["plan_digest"].as_str().unwrap().to_owned();
    assert_eq!(
        plan["steps"][0]["inputs"]["argv"]["literal"]["value"][0],
        executable.display().to_string(),
        "{plan}"
    );

    let (ok, missing) = run(
        workspace.path(),
        &state_home,
        &[
            "action".into(),
            "run".into(),
            "run-it".into(),
            "--expect-plan".into(),
            digest.clone(),
            "--machine".into(),
        ],
    );
    assert!(!ok);
    assert_eq!(missing["error"], "AUTHORITY_REQUIRED");

    let mismatch_path = workspace.path().join("mismatch.json");
    contract(
        &mismatch_path,
        vec![executable.display().to_string(), "--help".into()],
        "allow",
    );
    let (ok, mismatch) = run(
        workspace.path(),
        &state_home,
        &[
            "action".into(),
            "run".into(),
            "run-it".into(),
            "--expect-plan".into(),
            digest.clone(),
            "--execution-contract".into(),
            format!("process={}", mismatch_path.display()),
            "--machine".into(),
        ],
    );
    assert!(!ok, "{mismatch}");
    assert_eq!(
        mismatch["steps"][0]["error"]["code"], "AUTHORITY_CONTRACT_MISMATCH",
        "{mismatch}"
    );

    let contract_path = workspace.path().join("current.json");
    contract(
        &contract_path,
        vec![executable.display().to_string(), "--version".into()],
        "allow",
    );
    let (ok, report) = run(
        workspace.path(),
        &state_home,
        &[
            "action".into(),
            "run".into(),
            "run-it".into(),
            "--expect-plan".into(),
            digest,
            "--execution-contract".into(),
            format!("process={}", contract_path.display()),
            "--machine".into(),
        ],
    );
    assert!(ok, "execution report: {report}");
    assert_eq!(report["status"], "COMPLETED");
    assert_eq!(report["completed_steps"], 1);
    assert!(
        report["steps"][0]["process_exit"]["code"]
            .as_i64()
            .unwrap_or(-1)
            == 0
    );
    assert!(
        report["evidence_artifact"]
            .as_str()
            .unwrap()
            .starts_with("artifact://sha256/")
    );
}

#[test]
fn stale_plan_refuses_before_authority_or_spawn() {
    let workspace = tempdir().unwrap();
    let state_home = workspace.path().join("state");
    let executable = std::path::PathBuf::from(env!("CARGO_BIN_EXE_omen"));
    fs::write(
        workspace.path().join("Omen.toml"),
        config(&executable, "--version"),
    )
    .unwrap();
    let (_, plan) = run(
        workspace.path(),
        &state_home,
        &[
            "action".into(),
            "plan".into(),
            "run-it".into(),
            "--machine".into(),
        ],
    );
    let digest = plan["plan_digest"].as_str().unwrap().to_owned();
    fs::write(
        workspace.path().join("Omen.toml"),
        config(&executable, "--help"),
    )
    .unwrap();
    let (ok, error) = run(
        workspace.path(),
        &state_home,
        &[
            "action".into(),
            "run".into(),
            "run-it".into(),
            "--expect-plan".into(),
            digest,
            "--machine".into(),
        ],
    );
    assert!(!ok);
    assert_eq!(error["error"], "PLAN_CHANGED");
    assert!(error["expected_plan_digest"].as_str().is_some(), "{error}");
    assert!(error["actual_plan_digest"].as_str().is_some(), "{error}");
}

#[test]
fn network_denial_refuses_before_spawn() {
    let workspace = tempdir().unwrap();
    let state_home = workspace.path().join("state");
    let executable = std::path::PathBuf::from(env!("CARGO_BIN_EXE_omen"));
    fs::write(
        workspace.path().join("Omen.toml"),
        config(&executable, "--version"),
    )
    .unwrap();
    let (_, plan) = run(
        workspace.path(),
        &state_home,
        &[
            "action".into(),
            "plan".into(),
            "run-it".into(),
            "--machine".into(),
        ],
    );
    let contract_path = workspace.path().join("denied.json");
    contract(
        &contract_path,
        vec![executable.display().to_string(), "--version".into()],
        "deny",
    );
    let (ok, error) = run(
        workspace.path(),
        &state_home,
        &[
            "action".into(),
            "run".into(),
            "run-it".into(),
            "--expect-plan".into(),
            plan["plan_digest"].as_str().unwrap().into(),
            "--execution-contract".into(),
            format!("process={}", contract_path.display()),
            "--machine".into(),
        ],
    );
    assert!(!ok, "{error}");
    assert_eq!(
        error["steps"][0]["error"]["code"], "NETWORK_CONSTRAINT_UNENFORCEABLE",
        "{error}"
    );
}
