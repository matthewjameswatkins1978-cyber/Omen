//! 0.9-D.1 review regressions: the independent Preview 7 review findings,
//! proven against the CLI surface.
//!
//! Regression mapping:
//! - R1 EMPTY-STDIN FALSE RESULT (Workstream A)
//! - R2 CHILD FAILURE TRUTH (Workstream B)
//! - R3 ARTIFACT ROUND-TRIP (Workstream C)
//! - R5 UNJOURNALED HISTORY (Workstream E)
//! - R6 CAPABILITY ROUTING (Workstream F)
//! - R7 INSTALLED IDENTITY (Workstream H)
//!
//! R4 (daemon installed path) is proven against the installed Preview
//! package at release stage, not in this suite.

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::{TempDir, tempdir};

fn omen() -> Command {
    Command::new(env!("CARGO_BIN_EXE_omen"))
}

fn workspace() -> TempDir {
    tempdir().expect("temporary review workspace")
}

fn run_machine(workspace: &Path, args: &[&str]) -> (Output, Value) {
    // Global flags must precede the subcommand: anything after `--` belongs
    // to the child argv.
    let output = omen()
        .arg("--machine")
        .arg("--workspace")
        .arg(workspace)
        .args(args)
        .output()
        .expect("run omen machine surface");
    let value: Value = serde_json::from_slice(&output.stdout).expect("machine output is JSON");
    (output, value)
}

fn rg_available() -> bool {
    Command::new("rg")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn failing_child() -> Vec<&'static str> {
    vec!["cmd", "/c", "echo hello & echo bad 1>&2 & exit 2"]
}

#[cfg(not(windows))]
fn failing_child() -> Vec<&'static str> {
    vec!["sh", "-c", "echo hello; echo bad >&2; exit 2"]
}

#[cfg(windows)]
fn sleeping_child() -> Vec<&'static str> {
    vec!["cmd", "/c", "ping -n 6 127.0.0.1 >nul"]
}

#[cfg(not(windows))]
fn sleeping_child() -> Vec<&'static str> {
    vec!["sh", "-c", "sleep 5"]
}

#[cfg(windows)]
fn echo_child(message: &str) -> Vec<String> {
    vec![
        "cmd".to_string(),
        "/c".to_string(),
        format!("echo {message}"),
    ]
}

#[cfg(not(windows))]
fn echo_child(message: &str) -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("echo {message}"),
    ]
}

fn write_token(workspace: &Path, name: &str) {
    std::fs::write(
        workspace.join(name),
        "review-token-OMEN_RG_PROBE_0123456789abcdef\n",
    )
    .unwrap();
}

// R1: a bare `rg TOKEN` through Omen must behave like native rg, never a
// silent stdin-induced false negative.
#[test]
fn r1_bare_rg_search_matches_native_behavior() {
    if !rg_available() {
        eprintln!("SKIP r1: ripgrep not on PATH");
        return;
    }
    let temp = workspace();
    write_token(temp.path(), "probe.txt");

    let native = Command::new("rg")
        .arg("-l")
        .arg("OMEN_RG_PROBE_0123456789abcdef")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(native.status.success());
    assert!(String::from_utf8_lossy(&native.stdout).contains("probe.txt"));

    for extra in [&[] as &[&str], &["."] as &[&str]] {
        let mut args = vec!["exec", "--", "rg", "-l", "OMEN_RG_PROBE_0123456789abcdef"];
        args.extend_from_slice(extra);
        let (output, value) = run_machine(temp.path(), &args);
        assert!(output.status.success());
        assert_eq!(
            value["exit_code"], 0,
            "omen rg must find the token: {value}"
        );
        assert!(
            value["stdout_bounded"]
                .as_str()
                .unwrap()
                .contains("probe.txt"),
            "omen rg must report the file: {value}"
        );
    }
}

// R2 machine: full child truth is structural.
#[test]
fn r2_machine_exec_projects_child_failure_truth() {
    let temp = workspace();
    let child = failing_child();
    let mut args = vec!["exec", "--"];
    args.extend(child);
    let (output, value) = run_machine(temp.path(), &args);
    // Machine transport itself succeeded; the child result is structural.
    assert!(output.status.success());
    assert!(value["execution_id"].as_str().unwrap().starts_with("exec_"));
    assert_eq!(value["runtime_status"], "COMPLETED");
    assert_eq!(value["exit_code"], 2);
    assert!(value["stdout_bounded"].as_str().unwrap().contains("hello"));
    assert!(value["stderr_bounded"].as_str().unwrap().contains("bad"));
    assert!(
        value["stdout_artifact_uri"]
            .as_str()
            .unwrap()
            .starts_with("artifact://sha256/")
    );
    assert!(
        value["stderr_artifact_uri"]
            .as_str()
            .unwrap()
            .starts_with("artifact://sha256/")
    );
    assert_eq!(value["stdin_disposition"], "closed:null-device");
}

// R2 human: stdout stays on stdout, stderr on stderr, identity on stderr,
// process status propagates.
#[test]
fn r2_human_exec_preserves_streams_and_exit_status() {
    let temp = workspace();
    let child = failing_child();
    let output = omen()
        .arg("exec")
        .arg("--workspace")
        .arg(temp.path())
        .arg("--")
        .args(&child)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.contains("bad"),
        "child stderr must survive: {stderr}"
    );
    assert!(
        stderr.contains("\\O/ exec_"),
        "execution identity must appear on stderr: {stderr}"
    );
    assert!(stderr.contains("exit 2"), "exit truth on stderr: {stderr}");
    assert!(
        stderr.contains("artifact://sha256/"),
        "evidence reference on stderr: {stderr}"
    );
}

// R2 human success stays pipeline-clean and exits zero.
#[test]
fn r2_human_exec_success_is_quiet_and_zero() {
    let temp = workspace();
    let child = echo_child("clean-probe");
    let output = omen()
        .arg("exec")
        .arg("--workspace")
        .arg(temp.path())
        .arg("--")
        .args(&child)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("clean-probe"));
    assert!(output.stderr.is_empty());
}

// R2 timeout: canonical TIMED_OUT spelling, null exit, machine CLI stays 0.
#[test]
fn r2_machine_timeout_is_explicit_timed_out() {
    let temp = workspace();
    let child = sleeping_child();
    let mut args = vec!["exec", "--timeout-ms", "800", "--"];
    args.extend(child);
    let (output, value) = run_machine(temp.path(), &args);
    assert!(output.status.success());
    assert_eq!(value["runtime_status"], "TIMED_OUT");
    assert!(value["exit_code"].is_null());
}

// R3: the exact reviewer workflow — exec URI untouched into inspect/read.
#[test]
fn r3_artifact_uri_round_trips_untouched() {
    let temp = workspace();
    let child = echo_child("round-trip-probe");
    let mut exec_args = vec!["exec".to_string(), "--".to_string()];
    exec_args.extend(child);
    let exec_refs: Vec<&str> = exec_args.iter().map(String::as_str).collect();
    let (_, value) = run_machine(temp.path(), &exec_refs);
    let uri = value["artifact_uri"].as_str().unwrap().to_string();
    assert!(uri.starts_with("artifact://sha256/"));

    for subcommand in ["inspect", "read"] {
        let probed = omen()
            .arg("--machine")
            .arg("artifact")
            .arg(subcommand)
            .arg(&uri)
            .arg("--workspace")
            .arg(temp.path())
            .output()
            .unwrap();
        assert!(
            probed.status.success(),
            "{subcommand} {uri} failed: {}",
            String::from_utf8_lossy(&probed.stderr)
        );
        let doc: Value = serde_json::from_slice(&probed.stdout).unwrap();
        if subcommand == "read" {
            assert!(
                doc["content"]
                    .as_str()
                    .unwrap()
                    .contains("round-trip-probe")
            );
        } else {
            assert_eq!(doc["uri"], uri);
        }
    }

    // Bare digest still works.
    let digest = uri.trim_start_matches("artifact://sha256/").to_string();
    let probed = omen()
        .arg("--machine")
        .arg("artifact")
        .arg("inspect")
        .arg(&digest)
        .arg("--workspace")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(probed.status.success());

    // Malformed references are structured INVALID_URI, not a silent miss.
    for bad in [
        "artifact://md5/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "artifact://sha256/tooshort",
        "artifact://sha256/zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        "not-a-reference",
    ] {
        let probed = omen()
            .arg("--machine")
            .arg("artifact")
            .arg("inspect")
            .arg(bad)
            .arg("--workspace")
            .arg(temp.path())
            .output()
            .unwrap();
        assert!(!probed.status.success());
        let doc: Value = serde_json::from_slice(&probed.stdout).unwrap();
        assert_eq!(doc["error"]["code"], "INVALID_URI", "for {bad}: {doc}");
    }
}

// R5 machine: CLI projects the same UNJOURNALED truth as MCP.
#[test]
fn r5_cli_history_projects_unjournaled_local_execution() {
    let temp = workspace();
    let child = echo_child("marker-probe");
    let mut exec_args = vec!["exec".to_string(), "--".to_string()];
    exec_args.extend(child);
    let exec_refs: Vec<&str> = exec_args.iter().map(String::as_str).collect();
    let _ = run_machine(temp.path(), &exec_refs);
    let (output, value) = run_machine(temp.path(), &["history", "--limit", "5"]);
    assert!(output.status.success());
    assert_eq!(value["history_status"], "UNJOURNALED_LOCAL_EXECUTION");
    assert!(value["history"]["entries"].as_array().unwrap().is_empty());
    assert!(
        value["local_execution"]["execution_id"]
            .as_str()
            .unwrap()
            .starts_with("exec_")
    );
}

// R5 human: empty durable history names the unjournaled marker.
#[test]
fn r5_human_history_names_unjournaled_marker() {
    let temp = workspace();
    let child = echo_child("marker-probe");
    let mut exec_args = vec!["exec".to_string(), "--".to_string()];
    exec_args.extend(child);
    let exec_refs: Vec<&str> = exec_args.iter().map(String::as_str).collect();
    let _ = run_machine(temp.path(), &exec_refs);
    let output = omen()
        .arg("history")
        .arg("--workspace")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(text.contains("No durable execution history recorded."));
    assert!(
        text.contains("unjournaled"),
        "human history must name it: {text}"
    );
}

// R6: described capabilities resolve to concrete invocation surfaces.
#[test]
fn r6_capability_routing_needs_no_guessing() {
    let temp = workspace();
    let (_, execution) = run_machine(temp.path(), &["describe", "execution.run"]);
    assert_eq!(
        execution["definition"]["invocation"]["cli"],
        "exec --machine -- <argv>"
    );
    assert_eq!(
        execution["definition"]["invocation"]["mcp_tool"],
        "omen_execute"
    );

    let (_, definition) = run_machine(temp.path(), &["describe", "semantic.definition"]);
    assert!(definition["definition"]["invocation"]["cli"].is_null());
    assert_eq!(
        definition["definition"]["invocation"]["mcp_tool"],
        "omen_symbol_definition"
    );
    assert_eq!(
        definition["definition"]["invocation"]["interactive"],
        ":def"
    );

    let (_, recipe) = run_machine(temp.path(), &["how", "execute-and-inspect"]);
    for step in recipe["steps"].as_array().unwrap() {
        assert!(
            step.get("invocation").is_some(),
            "recipe step must carry routing: {step}"
        );
    }
}

// R7: doctor identifies the running binary.
#[test]
fn r7_doctor_identifies_running_binary() {
    let temp = workspace();
    let (output, value) = run_machine(temp.path(), &["doctor"]);
    assert!(output.status.success());
    assert_eq!(value["status"], "ok");
    assert_eq!(value["contract_version"], "0.8");
    assert!(!value["git_sha"].as_str().unwrap().is_empty());
    assert!(!value["executable"].as_str().unwrap().is_empty());
    assert!(value.get("path_duplicates").is_some());

    let human = omen()
        .arg("doctor")
        .arg("--workspace")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(human.status.success());
    let text = String::from_utf8_lossy(&human.stdout).to_string();
    assert!(text.contains("Git SHA:"));
    assert!(text.contains("Machine Contract: 0.8"));
}
