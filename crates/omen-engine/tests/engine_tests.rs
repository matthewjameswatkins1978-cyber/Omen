use omen_core::{
    AdapterClassification, Assurance, CoreError, RequiredAssurance, RuntimeStatus, StdioMode,
};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use std::path::PathBuf;

fn gremlin_exe() -> PathBuf {
    let mut path = std::env::current_exe().expect("failed to get current_exe");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    let name = if cfg!(windows) {
        "omen-gremlin.exe"
    } else {
        "omen-gremlin"
    };
    let exe = path.join(name);
    if exe.exists() {
        return exe;
    }

    let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target")
        .join("debug")
        .join(name);
    if fallback.exists() {
        return fallback;
    }

    // If binary not found, build it on-demand
    let _ = std::process::Command::new("cargo")
        .args(["build", "--bin", "omen-gremlin"])
        .status();

    if exe.exists() {
        return exe;
    }
    fallback
}

#[tokio::test]
async fn gremlin_closed_stdin_receives_eof() {
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();
    assert!(
        gremlin.exists(),
        "gremlin binary must exist at {:?}",
        gremlin
    );

    let req = ExecutionRequest {
        argv: vec![gremlin.to_string_lossy().to_string(), "--read-stdin".into()],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms: 10000,
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: vec![],
    };

    let output = supervisor.execute(req).await.unwrap();
    assert_eq!(output.runtime_status, RuntimeStatus::Completed);
    assert_eq!(output.process_exit.code, Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout_all);
    assert!(
        stdout.contains("READ_STDIN_BYTES:0"),
        "Closed stdin must receive EOF immediately, got: {stdout}"
    );
}

#[tokio::test]
async fn gremlin_stdout_and_stderr_captured_separately() {
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();

    let req = ExecutionRequest {
        argv: vec![
            gremlin.to_string_lossy().to_string(),
            "--stdout".into(),
            "hello-stdout".into(),
            "--stderr".into(),
            "hello-stderr".into(),
        ],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms: 10000,
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: vec![],
    };

    let output = supervisor.execute(req).await.unwrap();
    assert_eq!(output.runtime_status, RuntimeStatus::Completed);
    assert!(String::from_utf8_lossy(&output.stdout_all).contains("hello-stdout"));
    assert!(String::from_utf8_lossy(&output.stderr_all).contains("hello-stderr"));
}

#[tokio::test]
async fn gremlin_timeout_terminates_execution() {
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();

    let req = ExecutionRequest {
        argv: vec![
            gremlin.to_string_lossy().to_string(),
            "--sleep-ms".into(),
            "5000".into(),
        ],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms: 200, // Short timeout
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: vec![],
    };

    let output = supervisor.execute(req).await.unwrap();
    assert_eq!(output.runtime_status, RuntimeStatus::TimedOut);
    assert_eq!(
        output.adapter_classification,
        AdapterClassification::Failure
    );
}

#[tokio::test]
async fn gremlin_exit_code_separated_from_runtime_completion() {
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();

    let req = ExecutionRequest {
        argv: vec![
            gremlin.to_string_lossy().to_string(),
            "--exit".into(),
            "42".into(),
        ],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms: 10000,
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: vec![],
    };

    let output = supervisor.execute(req).await.unwrap();
    assert_eq!(output.runtime_status, RuntimeStatus::Completed);
    assert_eq!(output.process_exit.code, Some(42));
    assert_eq!(
        output.adapter_classification,
        AdapterClassification::Failure
    );
}

#[tokio::test]
async fn gremlin_bounded_output() {
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();

    let large_message = "A".repeat(1000);
    let req = ExecutionRequest {
        argv: vec![
            gremlin.to_string_lossy().to_string(),
            "--stdout".into(),
            large_message.clone(),
        ],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms: 10000,
        inline_budget: 100, // Strict bound
        required_assurance: RequiredAssurance::default(),
        secrets: vec![],
    };

    let output = supervisor.execute(req).await.unwrap();
    assert_eq!(output.stdout_bounded.len(), 100);
    assert!(output.stdout_all.len() >= 1000);
}

#[tokio::test]
async fn preflight_rejects_unsupported_assurance() {
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();

    let req = ExecutionRequest {
        argv: vec![gremlin.to_string_lossy().to_string()],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms: 10000,
        inline_budget: 8192,
        required_assurance: RequiredAssurance {
            filesystem: Some(Assurance::Enforced),
            network: None,
            descendants: None,
        },
        secrets: vec![],
    };

    let res = supervisor.execute(req).await;
    match res {
        Err(CoreError::AssuranceNotSatisfied {
            required,
            available,
        }) => {
            assert_eq!(required, "Enforced");
            assert_eq!(available, "Observed");
        }
        Ok(_) => {
            assert_eq!(
                supervisor.backend().capabilities().filesystem,
                omen_core::EnforcementLevel::Enforced
            );
        }
        Err(e) => panic!("Unexpected error: {:?}", e),
    }
}
