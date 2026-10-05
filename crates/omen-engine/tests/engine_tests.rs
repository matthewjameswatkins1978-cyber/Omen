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

fn pipeline_stage(gremlin: &std::path::Path, args: &[&str], timeout_ms: u64) -> ExecutionRequest {
    let mut argv = vec![gremlin.to_string_lossy().to_string()];
    argv.extend(args.iter().map(|arg| (*arg).to_string()));
    ExecutionRequest {
        argv,
        cwd: std::env::current_dir().unwrap(),
        env: Vec::new(),
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms,
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: Vec::new(),
    }
}

#[tokio::test]
async fn native_pipeline_streams_exact_bytes_between_stages() {
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();
    let producer = pipeline_stage(&gremlin, &["--stdout", "pipeline-bytes"], 10000);
    let consumer = pipeline_stage(&gremlin, &["--echo-stdin"], 10000);

    let output = supervisor
        .execute_pipeline(vec![producer, consumer])
        .await
        .unwrap();

    assert_eq!(output.execution.runtime_status, RuntimeStatus::Completed);
    assert_eq!(output.execution.process_exit.code, Some(0));
    assert_eq!(output.stage_exits.len(), 2);
    assert!(output.stage_exits.iter().all(|exit| exit.code == Some(0)));
    assert_eq!(
        output.execution.stdout_all,
        b"READ_STDIN_BYTES:15\nSTDIN_ECHO:pipeline-bytes\n"
    );
    assert_eq!(output.stderr_by_stage, vec![Vec::<u8>::new(), Vec::new()]);
}

#[tokio::test]
async fn native_pipeline_streams_more_than_pipe_capacity_without_deadlock() {
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();
    let producer = pipeline_stage(&gremlin, &["--stdout-bytes", "1048576"], 10000);
    let consumer = pipeline_stage(&gremlin, &["--stdin-report"], 10000);

    let output = supervisor
        .execute_pipeline(vec![producer, consumer])
        .await
        .unwrap();

    assert_eq!(output.execution.runtime_status, RuntimeStatus::Completed);
    assert_eq!(output.execution.process_exit.code, Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.execution.stdout_all).unwrap();
    assert_eq!(report["bytes_read"], 1_048_576);
    assert_eq!(report["stdin_eof"], true);
}

#[tokio::test]
async fn native_pipeline_deadline_terminates_every_stage() {
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();
    let producer = pipeline_stage(&gremlin, &["--sleep-ms", "5000"], 200);
    let consumer = pipeline_stage(&gremlin, &["--echo-stdin"], 200);

    let output = supervisor
        .execute_pipeline(vec![producer, consumer])
        .await
        .unwrap();

    assert_eq!(output.execution.runtime_status, RuntimeStatus::TimedOut);
    assert_eq!(output.stage_exits.len(), 2);
    assert!(output.execution.duration_ms < 3000);
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

#[tokio::test]
async fn cancel_mid_flight_kills_and_confirms_termination() {
    use tokio::sync::watch;
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();

    let req = ExecutionRequest {
        argv: vec![
            gremlin.to_string_lossy().to_string(),
            "--sleep-ms".into(),
            "30000".into(),
        ],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms: 60000,
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: vec![],
    };

    let (tx, rx) = watch::channel(false);
    let handle = tokio::spawn(async move { supervisor.execute_cancelable(req, rx).await });
    // Let the child actually start before requesting the stop.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    tx.send(true).expect("cancel flag must fire");
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), handle)
        .await
        .expect("cancel wait must be bounded")
        .expect("worker task must not panic")
        .expect("cancelable execute must return output");
    assert_eq!(output.runtime_status, RuntimeStatus::Cancelled);
    assert_eq!(output.process_exit.signal.as_deref(), Some("CANCELLED"));
    assert_eq!(
        output.adapter_classification,
        AdapterClassification::Failure
    );
}

#[tokio::test]
async fn unfired_cancel_leaves_normal_completion_untouched() {
    use tokio::sync::watch;
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();

    let req = ExecutionRequest {
        argv: vec![
            gremlin.to_string_lossy().to_string(),
            "--stdout".into(),
            "cancel-quiet".into(),
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

    let (_tx, rx) = watch::channel(false);
    let output = supervisor.execute_cancelable(req, rx).await.unwrap();
    assert_eq!(output.runtime_status, RuntimeStatus::Completed);
    assert!(String::from_utf8_lossy(&output.stdout_all).contains("cancel-quiet"));
}

#[tokio::test]
async fn prefired_cancel_never_spawns() {
    use omen_core::BackendId;
    use omen_core::EnforcementLevel;
    use omen_engine::backend::ExecutionWaitFuture;
    use omen_engine::{
        BackendAvailability, BackendCapabilities, BackendDescriptor, BackendKind, ExecutionBackend,
        ExecutionHandle, PtyExecutionHandle, PtyExecutionRequest,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::watch;

    struct NoSpawnHandle;
    impl ExecutionHandle for NoSpawnHandle {
        fn pid(&self) -> Option<u32> {
            None
        }
        fn terminate_tree(&mut self) -> Result<(), CoreError> {
            Ok(())
        }
        fn wait_bounded(self: Box<Self>, _timeout: Duration) -> ExecutionWaitFuture {
            panic!("NoSpawn backend must never wait: nothing was spawned");
        }
        fn wait_cancelable(
            self: Box<Self>,
            _timeout: Duration,
            _cancel: watch::Receiver<bool>,
        ) -> ExecutionWaitFuture {
            panic!("NoSpawn backend must never wait: nothing was spawned");
        }
    }

    struct NoSpawnBackend {
        spawns: Arc<AtomicUsize>,
    }
    impl ExecutionBackend for NoSpawnBackend {
        fn id(&self) -> BackendId {
            BackendId::native()
        }
        fn descriptor(&self) -> BackendDescriptor {
            BackendDescriptor {
                id: self.id(),
                name: "NoSpawn".into(),
                kind: BackendKind::NativeHost,
                availability: BackendAvailability::Available,
                capabilities: BackendCapabilities {
                    filesystem: EnforcementLevel::Observed,
                    network: EnforcementLevel::Observed,
                    descendants: EnforcementLevel::Observed,
                    symlink_escape: EnforcementLevel::Observed,
                    pty: false,
                },
            }
        }
        fn spawn(
            &self,
            _req: &omen_engine::supervisor::ExecutionRequest,
        ) -> Result<Box<dyn ExecutionHandle>, CoreError> {
            self.spawns.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(NoSpawnHandle))
        }
        fn spawn_pty(
            &self,
            _req: &PtyExecutionRequest,
        ) -> Result<Box<dyn PtyExecutionHandle>, CoreError> {
            Err(CoreError::ExecutionFailed("NoSpawn has no PTY".into()))
        }
    }

    let spawns = Arc::new(AtomicUsize::new(0));
    let supervisor = ProcessSupervisor::with_backend(Arc::new(NoSpawnBackend {
        spawns: spawns.clone(),
    }));

    let req = ExecutionRequest {
        argv: vec!["anything".into()],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms: 10000,
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: vec![],
    };

    let (tx, rx) = watch::channel(false);
    tx.send(true).expect("cancel flag must fire");
    let output = supervisor.execute_cancelable(req, rx).await.unwrap();
    assert_eq!(output.runtime_status, RuntimeStatus::Cancelled);
    assert_eq!(
        output.process_exit.signal.as_deref(),
        Some("CANCELLED_BEFORE_DISPATCH")
    );
    assert_eq!(
        spawns.load(Ordering::SeqCst),
        0,
        "pre-fired cancel must never spawn a physical process"
    );
}
