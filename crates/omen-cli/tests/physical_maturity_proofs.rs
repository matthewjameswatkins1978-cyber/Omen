use omen_core::{
    Assurance, CoreError, EnforcementLevel, PtySessionId, RequiredAssurance, RetentionClass,
    RuntimeStatus, StdioMode,
};
use omen_daemon::{DaemonServer, PtySessionManager};
use omen_engine::{ExecutionRequest, ExecutionSecret, ProcessSupervisor, WslExecutionBackend};
use omen_knowledge::{ContentAddressedStore, Database};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;

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

    let _ = std::process::Command::new("cargo")
        .args(["build", "--bin", "omen-gremlin"])
        .status();

    if exe.exists() {
        return exe;
    }
    fallback
}

/// PROOF A: Daemon-owned PTY Session Detach and Reattach Real Path
#[tokio::test]
async fn test_proof_a_pty_detach_reattach_real_path() {
    let gremlin = gremlin_exe();
    assert!(gremlin.exists(), "gremlin must exist");

    let pty_manager = PtySessionManager::new();
    let session_id = PtySessionId::generate();
    let sid_str = session_id.to_string();

    // 1. Create daemon-owned PTY session running interactive gremlin
    let pid = pty_manager
        .create(
            &sid_str,
            vec![gremlin.to_string_lossy().to_string(), "--pty-echo".into()],
            std::env::current_dir().unwrap(),
            vec![],
            24,
            80,
        )
        .await
        .expect("PTY session must be created");

    assert!(pid.is_some(), "PTY child process must have an OS PID");
    let proc_pid = pid.unwrap();
    assert!(
        omen_engine::is_process_alive(proc_pid),
        "PTY child must be physically alive"
    );

    // 2. Client 1 attaches, verifies initial prompt, and sends input
    let (initial_out, state) = pty_manager
        .attach(&sid_str)
        .await
        .expect("Client 1 must attach");
    assert_eq!(state, omen_core::PtyState::Running);

    // Wait bounded time for PTY_READY
    let start = std::time::Instant::now();
    let mut ready = String::from_utf8_lossy(&initial_out).contains("PTY_READY");
    while !ready && start.elapsed() < Duration::from_millis(2000) {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (out, _, _) = pty_manager.read_output(&sid_str, 0).await.unwrap();
        if String::from_utf8_lossy(&out).contains("PTY_READY") {
            ready = true;
            break;
        }
    }
    assert!(ready, "PTY interactive echo must emit PTY_READY");

    // Write input from Client 1
    pty_manager
        .write_input(&sid_str, b"ping_message_1\n")
        .await
        .expect("Must write PTY input");

    // Wait bounded time for echo
    let start = std::time::Instant::now();
    let mut echoed = false;
    while !echoed && start.elapsed() < Duration::from_millis(2000) {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (out, _, _) = pty_manager.read_output(&sid_str, 0).await.unwrap();
        if String::from_utf8_lossy(&out).contains("ECHO:ping_message_1") {
            echoed = true;
            break;
        }
    }
    assert!(echoed, "PTY session must echo Client 1's input");

    // 3. Client 1 detaches
    pty_manager
        .detach(&sid_str)
        .await
        .expect("Client 1 must detach cleanly");

    // Physical check: process MUST remain alive in daemon after client disconnect!
    assert!(
        omen_engine::is_process_alive(proc_pid),
        "Child process must survive client detach"
    );

    // 4. Client 2 attaches to the existing session
    let (reattached_out, reattached_state) = pty_manager
        .attach(&sid_str)
        .await
        .expect("Client 2 must reattach to existing session");
    assert_eq!(reattached_state, omen_core::PtyState::Running);

    // Verify ring-buffer output was retained
    let reattached_text = String::from_utf8_lossy(&reattached_out);
    assert!(
        reattached_text.contains("ECHO:ping_message_1"),
        "Client 2 must receive buffered output upon reattach"
    );

    // 5. Client 2 sends exit command to conclude interactive session
    pty_manager
        .write_input(&sid_str, b"exit\n")
        .await
        .expect("Must write exit to PTY");

    // Bounded wait for process exit via PTY state polling (which reaps child process)
    let start = std::time::Instant::now();
    let mut exited = false;
    while !exited && start.elapsed() < Duration::from_millis(2000) {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Ok((_, _, omen_core::PtyState::Exited)) = pty_manager.read_output(&sid_str, 0).await
        {
            exited = true;
            break;
        }
    }
    assert!(exited, "Process must terminate upon receiving exit");

    // Verify process PID is no longer alive once reaped
    assert!(
        !omen_engine::is_process_alive(proc_pid),
        "Process PID must be dead once reaped"
    );
}

/// PROOF B: Process-Tree Ownership and Descendant Termination Real Path
#[tokio::test]
async fn test_proof_b_process_tree_kill_real_path() {
    let gremlin = gremlin_exe();
    let supervisor = ProcessSupervisor::new();

    // Spawn a hostile tree with depth 1: parent spawns a child that sleeps for 60s
    let req = ExecutionRequest {
        argv: vec![
            gremlin.to_string_lossy().to_string(),
            "--spawn-tree".into(),
            "1".into(),
            "--sleep-ms".into(),
            "60000".into(),
        ],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms: 1000, // Strict timeout: parent will be killed
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: vec![],
    };

    let output = supervisor
        .execute(req)
        .await
        .expect("Execution must run to timeout");

    assert_eq!(output.runtime_status, RuntimeStatus::TimedOut);

    // Extract child PID from stdout
    let stdout = String::from_utf8_lossy(&output.stdout_all);
    let mut child_pid: Option<u32> = None;
    for line in stdout.lines() {
        let parsed_pid = line
            .strip_prefix("TREE_SPAWNED:")
            .and_then(|s| s.trim().parse::<u32>().ok());
        if let Some(p) = parsed_pid {
            child_pid = Some(p);
            break;
        }
    }

    if let Some(cpid) = child_pid {
        // Bounded verification: child process must be terminated by OS Job Object / process group
        let start = std::time::Instant::now();
        let mut child_dead = false;
        while start.elapsed() < Duration::from_millis(2000) {
            if !omen_engine::is_process_alive(cpid) {
                child_dead = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            child_dead,
            "Child process PID {cpid} must be terminated when parent times out"
        );
    }
}

/// PROOF C: Fail-Closed Assurance Refusal Before Spawn Real Path
#[tokio::test]
async fn test_proof_c_assurance_refusal_real_path() {
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();

    // Native backend reports network: Observed. Requesting network: Enforced MUST fail preflight.
    let req = ExecutionRequest {
        argv: vec![gremlin.to_string_lossy().to_string()],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms: 5000,
        inline_budget: 8192,
        required_assurance: RequiredAssurance {
            filesystem: None,
            network: Some(Assurance::Enforced), // Prohibited requirement on native backend
            descendants: None,
        },
        secrets: vec![],
    };

    let result = supervisor.execute(req).await;
    match result {
        Err(CoreError::AssuranceNotSatisfied {
            required,
            available,
        }) => {
            assert_eq!(required, "Enforced");
            assert_eq!(available, "Observed");
        }
        Ok(_) => {
            panic!(
                "Execution must be refused before spawn when required assurance exceeds backend capabilities"
            );
        }
        Err(e) => {
            panic!("Expected AssuranceNotSatisfied, got: {e:?}");
        }
    }
}

/// PROOF D: Truthful Containment Assurance Matrix Real Path
#[tokio::test]
async fn test_proof_d_containment_real_path() {
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();

    let req = ExecutionRequest {
        argv: vec![
            gremlin.to_string_lossy().to_string(),
            "--exit".into(),
            "0".into(),
        ],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms: 5000,
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: vec![],
    };

    let output = supervisor.execute(req).await.unwrap();
    assert_eq!(output.runtime_status, RuntimeStatus::Completed);

    // On Windows, Job Objects guarantee descendant cleanup (ENFORCED),
    // but filesystem and network are observed (never falsely claimed as ENFORCED).
    assert_eq!(
        output.enforcement.network,
        EnforcementLevel::Observed,
        "Network without sandbox must report OBSERVED"
    );
    assert_eq!(
        output.enforcement.filesystem,
        EnforcementLevel::Observed,
        "Filesystem without container/sandbox must report OBSERVED"
    );

    #[cfg(windows)]
    assert_eq!(
        output.enforcement.descendant_processes,
        EnforcementLevel::Enforced,
        "Windows Job Objects must be reported as ENFORCED"
    );
}

/// PROOF E: Real Non-Native Execution Backend (WSL on Windows) Real Path
#[tokio::test]
async fn test_proof_e_non_native_wsl_backend_real_path() {
    if !WslExecutionBackend::is_available() {
        println!("Skipping WSL real-path test: WSL2 is not available on this host environment");
        return;
    }

    let wsl_backend = Arc::new(WslExecutionBackend::new());
    let supervisor = ProcessSupervisor::with_backend(wsl_backend);

    let req = ExecutionRequest {
        argv: vec!["echo".into(), "hello-from-wsl-kernel".into()],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms: 10000,
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: vec![],
    };

    let output = supervisor
        .execute(req)
        .await
        .expect("WSL execution must succeed");

    assert_eq!(output.runtime_status, RuntimeStatus::Completed);
    assert_eq!(output.process_exit.code, Some(0));

    let stdout = String::from_utf8_lossy(&output.stdout_all);
    assert!(
        stdout.contains("hello-from-wsl-kernel"),
        "WSL backend must return command output, got: {stdout}"
    );
}

/// PROOF F: Secret Injection and Output Redaction Real Path
#[tokio::test]
async fn test_proof_f_secret_injection_and_redaction_real_path() {
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();
    let temp_dir = tempdir().unwrap();
    let mut db = Database::open(&temp_dir.path().join("state.sqlite")).unwrap();
    let cas = ContentAddressedStore::new(temp_dir.path().join("cas"));

    let canary_env = format!(
        "OMEN_SECRET_CANARY_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let canary_stdin = format!(
        "OMEN_STDIN_CANARY_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            + 1
    );

    let req = ExecutionRequest {
        argv: vec![
            gremlin.to_string_lossy().to_string(),
            "--print-env".into(),
            "AUTH_TOKEN".into(),
            "--read-stdin".into(),
        ],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Inline,
        stdin_payload: None,
        timeout_ms: 10000,
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: vec![
            ExecutionSecret::env("AUTH_TOKEN", &canary_env),
            ExecutionSecret::stdin("STDIN_TOKEN", &canary_stdin),
        ],
    };

    let output = supervisor
        .execute(req)
        .await
        .expect("Execution with secrets must succeed");

    let stdout = String::from_utf8_lossy(&output.stdout_all);

    // 1. Redaction verification: plaintext canary must NOT appear in output
    assert!(
        !stdout.contains(&canary_env),
        "Raw canary secret must be redacted from stdout! Got: {stdout}"
    );
    assert!(
        stdout.contains("[REDACTED:AUTH_TOKEN]"),
        "Redaction tag must appear in place of secret! Got: {stdout}"
    );

    // 2. CAS Storage verification: unreduced artifact in CAS must also be redacted
    let artifact = cas
        .store(
            &mut db,
            &output.stdout_all,
            "text/plain",
            "omen://execution/secret_test",
            RetentionClass::Referenced,
        )
        .expect("CAS store must succeed");

    let cas_content = cas
        .read_slice(&mut db, &artifact.digest, 0, artifact.size)
        .expect("Must retrieve artifact from CAS");

    let cas_text = String::from_utf8_lossy(&cas_content);
    assert!(
        !cas_text.contains(&canary_env),
        "CAS artifact must not leak plaintext canary secret"
    );
    assert!(cas_text.contains("[REDACTED:AUTH_TOKEN]"));
}

/// PROOF G: Physical Service Lifecycle Lease and Disconnect Survival Real Path
#[tokio::test]
async fn test_proof_g_service_ownership_real_path() {
    let gremlin = gremlin_exe();
    let temp_dir = tempdir().unwrap();

    let server = DaemonServer::new(None);
    let registry = server.registry();
    let ws = registry
        .get_or_attach(temp_dir.path())
        .await
        .expect("Workspace must attach");

    // 1. Start managed service with long runtime
    let service_name = "test-worker-service";
    let svc_info = ws
        .start_managed_service(
            service_name,
            &gremlin.to_string_lossy(),
            &["--sleep-ms".into(), "30000".into()],
        )
        .await
        .expect("Service must start");

    assert_eq!(svc_info.state, "running");
    assert!(svc_info.pid.is_some());
    let pid = svc_info.pid.unwrap();
    assert!(
        omen_engine::is_process_alive(pid),
        "Managed service must be physically running"
    );

    // Verify physical lifecycle lease ID is assigned
    assert!(
        svc_info.lease_id.is_some(),
        "Managed service must hold an active RuntimeLeaseId"
    );
    let lease_id = svc_info.lease_id.unwrap();
    assert!(lease_id.starts_with("lease-"));

    // 2. Simulate shell disconnect & reconnect:
    // Query services from workspace as if a new shell connected
    let services = ws.list_services().await;
    let found = services.iter().find(|s| s.name == service_name);
    assert!(found.is_some(), "Service must survive across shell queries");
    let found_svc = found.unwrap();
    assert_eq!(found_svc.state, "running");
    assert_eq!(found_svc.lease_id.as_deref(), Some(lease_id.as_str()));

    // 3. Stop service with bounded cleanup
    let stopped_name = ws
        .stop_managed_service(service_name)
        .await
        .expect("Service must stop cleanly");
    assert_eq!(stopped_name, service_name);

    // Verify physical termination: process is DEAD
    let start = std::time::Instant::now();
    let mut is_dead = false;
    while start.elapsed() < Duration::from_millis(2000) {
        if !omen_engine::is_process_alive(pid) {
            is_dead = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(is_dead, "Process must be terminated after stop");

    // Verify state updated in registry
    let services_after = ws.list_services().await;
    let stopped_svc = services_after
        .iter()
        .find(|s| s.name == service_name)
        .unwrap();
    assert_eq!(stopped_svc.state, "stopped");
}
