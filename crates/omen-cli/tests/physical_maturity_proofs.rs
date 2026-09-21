use omen_core::{
    Assurance, CoreError, EnforcementLevel, PtySessionId, RequiredAssurance, RetentionClass,
    RuntimeStatus, StdioMode,
};
use omen_daemon::{DaemonServer, PtySessionManager};
use omen_engine::{
    ExecutionBackend, ExecutionRequest, ExecutionSecret, ProcessSupervisor, WslExecutionBackend,
};
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
    let (_initial_out, state) = pty_manager
        .attach(&sid_str)
        .await
        .expect("Client 1 must attach");
    assert_eq!(state, omen_core::PtyState::Running);

    // Wait bounded time for PTY_READY and child-observed terminal semantics
    let start = std::time::Instant::now();
    let mut ready = false;
    let mut is_term = false;
    let mut initial_size = false;
    let mut initial_size_str = String::new();
    while (!ready || !is_term || !initial_size) && start.elapsed() < Duration::from_millis(5000) {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (out, _, _) = pty_manager.read_output(&sid_str, 0).await.unwrap();
        let text = String::from_utf8_lossy(&out);
        if text.contains("PTY_READY") {
            ready = true;
        }
        if text.contains("IS_TERMINAL:true") {
            is_term = true;
        }
        if let Some(pos) = text.find("INITIAL_SIZE:") {
            initial_size_str = text[pos..].lines().next().unwrap_or("").to_string();
        }
        if text.contains("INITIAL_SIZE:80x24") {
            initial_size = true;
        }
    }
    assert!(ready, "PTY interactive echo must emit PTY_READY");
    assert!(
        is_term,
        "Child process must observe attached terminal (isatty)"
    );
    assert!(
        initial_size,
        "Child process must observe initial terminal size 80x24, got: {initial_size_str}"
    );

    // Write input from Client 1
    pty_manager
        .write_input(&sid_str, b"ping_message_1\n")
        .await
        .expect("Must write PTY input");

    // Wait bounded time for echo
    let start = std::time::Instant::now();
    let mut echoed = false;
    while !echoed && start.elapsed() < Duration::from_millis(5000) {
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

    // 5. Test real OS PTY resize and verify child observes new dimensions (100x30)
    pty_manager
        .resize(&sid_str, 30, 100)
        .await
        .expect("PTY resize must succeed");
    // Allow terminal redraw from resize to settle in ring buffer
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send get_size request to query the child's live terminal dimensions
    pty_manager
        .write_input(&sid_str, b"get_size\n")
        .await
        .expect("Must write get_size command to PTY");

    let start = std::time::Instant::now();
    let mut resized = false;
    while !resized && start.elapsed() < Duration::from_millis(5000) {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (out, _, _) = pty_manager.read_output(&sid_str, 0).await.unwrap();
        if String::from_utf8_lossy(&out).contains("CURRENT_SIZE:100x30") {
            resized = true;
            break;
        }
    }
    assert!(
        resized,
        "Child process must observe resized dimensions 100x30"
    );

    // 6. Test real ring-buffer offset semantics
    let (read_0, total_offset_1, _) = pty_manager.read_output(&sid_str, 0).await.unwrap();
    assert!(total_offset_1 > 0);
    assert!(String::from_utf8_lossy(&read_0).contains("ECHO:ping_message_1"));

    // Write second message to test reading with non-zero offset
    pty_manager
        .write_input(&sid_str, b"offset_ping_2\n")
        .await
        .expect("Must write PTY input");

    let start = std::time::Instant::now();
    let mut offset_read = Vec::new();
    let mut total_offset_2 = total_offset_1;
    while start.elapsed() < Duration::from_millis(5000) {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (out, tot, _) = pty_manager
            .read_output(&sid_str, total_offset_1)
            .await
            .unwrap();
        if String::from_utf8_lossy(&out).contains("ECHO:offset_ping_2") {
            offset_read = out;
            total_offset_2 = tot;
            break;
        }
    }
    assert!(
        String::from_utf8_lossy(&offset_read).contains("ECHO:offset_ping_2"),
        "Offset read must contain new echoed input"
    );
    assert!(
        total_offset_2 > total_offset_1,
        "Total written offset must monotonically increase"
    );
    assert!(
        !String::from_utf8_lossy(&offset_read).contains("PTY_READY"),
        "Offset read starting from total_offset_1 must NOT replay earlier output"
    );

    // 7. Client 2 sends exit command to conclude interactive session
    pty_manager
        .write_input(&sid_str, b"exit\n")
        .await
        .expect("Must write exit to PTY");

    // Bounded wait for process exit via PTY state polling (which reaps child process)
    let start = std::time::Instant::now();
    let mut exited = false;
    while !exited && start.elapsed() < Duration::from_millis(5000) {
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

    // Spawn a hostile tree with depth 2: parent -> child -> grandchild
    let req = ExecutionRequest {
        argv: vec![
            gremlin.to_string_lossy().to_string(),
            "--spawn-tree".into(),
            "2".into(),
            "--sleep-ms".into(),
            "60000".into(),
        ],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms: 1000, // Strict timeout: parent tree will be terminated
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: vec![],
    };

    let output = supervisor
        .execute(req)
        .await
        .expect("Execution must run to timeout");

    assert_eq!(output.runtime_status, RuntimeStatus::TimedOut);

    // Extract child and grandchild PIDs from stdout
    let stdout = String::from_utf8_lossy(&output.stdout_all);
    let mut spawned_pids: Vec<u32> = Vec::new();
    for line in stdout.lines() {
        if let Some(p) = line
            .strip_prefix("TREE_SPAWNED:")
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            spawned_pids.push(p);
        }
    }

    assert!(
        spawned_pids.len() >= 2,
        "Must spawn at least child and grandchild (depth 2), found: {spawned_pids:?}"
    );

    for &pid in &spawned_pids {
        // Bounded verification: every descendant in the tree must be terminated
        let start = std::time::Instant::now();
        let mut is_dead = false;
        while start.elapsed() < Duration::from_millis(2000) {
            if !omen_engine::is_process_alive(pid) {
                is_dead = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            is_dead,
            "Descendant process PID {pid} must be terminated when root times out"
        );
    }
}

/// PROOF B2: A descendant created after the supervisor starts is still owned
/// by the same containment boundary.
#[cfg(windows)]
#[tokio::test]
async fn test_proof_b2_delayed_process_tree_kill_real_path() {
    let gremlin = gremlin_exe();
    let supervisor = ProcessSupervisor::new();
    let req = ExecutionRequest {
        argv: vec![
            gremlin.to_string_lossy().to_string(),
            "--spawn-tree".into(),
            "1".into(),
            "--spawn-tree-delay-ms".into(),
            "100".into(),
            "--sleep-ms".into(),
            "60000".into(),
        ],
        cwd: std::env::current_dir().unwrap(),
        env: vec![],
        stdin_mode: StdioMode::Closed,
        stdin_payload: None,
        timeout_ms: 1000,
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: vec![],
    };

    let output = supervisor
        .execute(req)
        .await
        .expect("delayed tree must run to timeout");
    assert_eq!(output.runtime_status, RuntimeStatus::TimedOut);

    let spawned_pid = String::from_utf8_lossy(&output.stdout_all)
        .lines()
        .find_map(|line| {
            line.strip_prefix("TREE_SPAWNED:")
                .and_then(|pid| pid.parse::<u32>().ok())
        })
        .expect("delayed child PID must be reported");

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(2000) {
        if !omen_engine::is_process_alive(spawned_pid) {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("delayed descendant process PID {spawned_pid} survived timeout");
}

/// PROOF B3: Containment is isolated when two executions overlap.
#[cfg(windows)]
#[tokio::test]
async fn test_proof_b3_concurrent_process_isolation_real_path() {
    let gremlin = gremlin_exe();
    let supervisor = ProcessSupervisor::new();
    let timed_out = ExecutionRequest {
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
        timeout_ms: 1000,
        inline_budget: 8192,
        required_assurance: RequiredAssurance::default(),
        secrets: vec![],
    };
    let completes = ExecutionRequest {
        argv: vec![
            gremlin.to_string_lossy().to_string(),
            "--stdout".into(),
            "survivor".into(),
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

    let (timed_out_result, completed_result) =
        tokio::join!(supervisor.execute(timed_out), supervisor.execute(completes));
    assert_eq!(
        timed_out_result
            .expect("timed-out execution must return")
            .runtime_status,
        RuntimeStatus::TimedOut
    );
    let completed = completed_result.expect("concurrent execution must return");
    assert_eq!(completed.runtime_status, RuntimeStatus::Completed);
    assert!(String::from_utf8_lossy(&completed.stdout_all).contains("survivor"));
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
    assert_eq!(
        output.enforcement.symlink_escape,
        EnforcementLevel::Observed,
        "Symlink escape without sandbox boundary must report OBSERVED"
    );

    #[cfg(windows)]
    assert_eq!(
        output.enforcement.descendant_processes,
        EnforcementLevel::Enforced,
        "Windows Job Objects must be reported as ENFORCED"
    );

    #[cfg(target_os = "linux")]
    assert_eq!(
        output.enforcement.descendant_processes,
        EnforcementLevel::BestEffort,
        "Linux process groups without cgroups v2 must report BEST_EFFORT"
    );
}

/// PROOF E: Real Non-Native Execution Backend (WSL on Windows) Real Path
#[tokio::test]
async fn test_proof_e_non_native_wsl_backend_real_path() {
    if !WslExecutionBackend::is_available() {
        if std::env::var("CI").is_ok() {
            println!("Skipping WSL real-path test: WSL2 is not available on hosted CI runner");
            return;
        } else {
            panic!("WSL backend must be available on local development host");
        }
    }

    let wsl_backend = Arc::new(WslExecutionBackend::new());
    let caps = wsl_backend.capabilities();
    assert_eq!(caps.descendants, EnforcementLevel::Mediated);
    assert_eq!(caps.filesystem, EnforcementLevel::Mediated);
    assert_eq!(caps.network, EnforcementLevel::Mediated);
    assert_eq!(caps.symlink_escape, EnforcementLevel::Mediated);

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
            "--echo-stdin".into(),
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

    // Assert that Debug implementation does not leak secret in cleartext
    for (idx, secret) in req.secrets.iter().enumerate() {
        let secret_debug = format!("{:?}", secret);
        assert!(
            !secret_debug.contains(&canary_env) && !secret_debug.contains(&canary_stdin),
            "Secret[{idx}] Debug implementation leaked raw secret value: {secret_debug}"
        );
        assert!(
            secret_debug.contains("[REDACTED]"),
            "Secret[{idx}] Debug implementation missing [REDACTED]: {secret_debug}"
        );
    }

    let output = supervisor
        .execute(req)
        .await
        .expect("Execution with secrets must succeed");

    let stdout = String::from_utf8_lossy(&output.stdout_all);

    // 1. Redaction verification: plaintext canaries must NOT appear in output
    assert!(
        !stdout.contains(&canary_env),
        "Raw env canary secret must be redacted from stdout! Got: {stdout}"
    );
    assert!(
        !stdout.contains(&canary_stdin),
        "Raw stdin canary secret must be redacted from stdout! Got: {stdout}"
    );
    assert!(
        stdout.contains("[REDACTED:AUTH_TOKEN]"),
        "Redaction tag for AUTH_TOKEN must appear in place of secret! Got: {stdout}"
    );
    assert!(
        stdout.contains("[REDACTED:STDIN_TOKEN]"),
        "Redaction tag for STDIN_TOKEN must appear in place of secret! Got: {stdout}"
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
        "CAS artifact must not leak plaintext env canary secret"
    );
    assert!(
        !cas_text.contains(&canary_stdin),
        "CAS artifact must not leak plaintext stdin canary secret"
    );
    assert!(cas_text.contains("[REDACTED:AUTH_TOKEN]"));
    assert!(cas_text.contains("[REDACTED:STDIN_TOKEN]"));
}

/// PROOF H: Hostile Terminal Escape Sanitization vs Verbatim Raw Evidence
#[tokio::test]
async fn test_proof_hostile_terminal_sanitization_vs_raw_evidence() {
    let supervisor = ProcessSupervisor::new();
    let gremlin = gremlin_exe();
    let temp_dir = tempdir().unwrap();
    let mut db = Database::open(&temp_dir.path().join("state.sqlite")).unwrap();
    let cas = ContentAddressedStore::new(temp_dir.path().join("cas"));

    let req = ExecutionRequest {
        argv: vec![
            gremlin.to_string_lossy().to_string(),
            "--hostile-terminal-escapes".into(),
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

    let output = supervisor
        .execute(req)
        .await
        .expect("Execution with hostile escapes must succeed");

    // 1. Raw execution output preserves verbatim untampered byte stream for forensics/CAS
    let raw_stdout = &output.stdout_all;
    assert!(
        raw_stdout.windows(2).any(|w| w == b"\x1b["),
        "Raw stdout must retain untampered ANSI CSI escape sequences"
    );
    assert!(
        raw_stdout.windows(2).any(|w| w == b"\x1b]"),
        "Raw stdout must retain untampered ANSI OSC escape sequences"
    );
    assert!(
        raw_stdout.contains(&0x07),
        "Raw stdout must retain untampered BEL control character"
    );
    assert!(
        String::from_utf8_lossy(raw_stdout).contains("HostileWindowTitle"),
        "Raw stdout must retain full hostile OSC payload"
    );

    // 2. CAS storage of raw output preserves exact unreduced forensic evidence
    let artifact = cas
        .store(
            &mut db,
            &output.stdout_all,
            "application/octet-stream",
            "omen://execution/hostile_escape_raw",
            RetentionClass::Referenced,
        )
        .expect("CAS store must succeed");

    let cas_content = cas
        .read_slice(&mut db, &artifact.digest, 0, artifact.size)
        .expect("Must read back from CAS");
    assert_eq!(
        cas_content, output.stdout_all,
        "CAS stored content must match raw untampered bytes verbatim"
    );

    // 3. Sanitized view (for daemon display, agent context, and logs) strips all hostile escapes
    let sanitized = output.stdout_sanitized();
    assert!(
        !sanitized.contains('\x1b'),
        "Sanitized output must contain zero ESC bytes"
    );
    assert!(
        !sanitized.contains('\x07'),
        "Sanitized output must contain zero BEL characters"
    );
    assert!(
        !sanitized.contains("HostileWindowTitle"),
        "Sanitized output must not leak OSC window title payload"
    );
    assert!(
        !sanitized.contains("HackedTitle"),
        "Sanitized output must not leak hacked OSC title payload"
    );
    assert!(
        sanitized.contains("LEGITIMATE_DATA"),
        "Sanitized output must preserve safe legitimate payload: got '{sanitized}'"
    );
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
