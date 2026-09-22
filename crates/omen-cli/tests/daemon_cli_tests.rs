// ENDPOINT_LOCK discipline: the default endpoint is machine-global, so the
// live-endpoint tests below serialize on it for their whole duration, i.e.
// across awaits, by design. Siblings only block on ENDPOINT_LOCK itself,
// so this cannot deadlock.
#![allow(clippy::await_holding_lock)]

use omen_client::OmenClient;
use omen_daemon::DaemonServer;
use omen_ipc::default_endpoint_address;
use std::sync::Mutex;
use tempfile::tempdir;

/// The default endpoint is machine-global: live-endpoint tests in this file
/// serialize so parallel harnesses never fight over one daemon address.
static ENDPOINT_LOCK: Mutex<()> = Mutex::new(());

#[tokio::test]
async fn test_daemon_cli_status_ping_and_stop_lifecycle() {
    let _guard = ENDPOINT_LOCK.lock().expect("endpoint lock poisoned");
    let endpoint = default_endpoint_address();

    // 1. When daemon is not running, ping fails cleanly
    let ping_res = OmenClient::connect_to(&endpoint, None).await;
    // If an existing daemon happens to be running from previous tests, shut it down first
    if let Ok(c) = ping_res {
        let _ = c.shutdown_daemon().await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Now verify offline behavior
    assert!(
        OmenClient::connect_to(&endpoint, None).await.is_err(),
        "Client connect should fail when daemon is offline"
    );

    // 2. Start daemon in background task
    let server = DaemonServer::new(Some(endpoint.clone()));
    let server_task = tokio::spawn(async move {
        let _ = server.run().await;
    });

    // Wait for daemon to become ready
    let mut ready = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(client) = OmenClient::connect_to(&endpoint, None).await
            && client.ping().await.is_ok()
        {
            ready = true;
            break;
        }
    }
    assert!(ready, "Daemon server failed to become ready");

    // 3. Status and Ping succeed
    let client = OmenClient::connect_to(&endpoint, None).await.unwrap();
    let pong_ts = client.ping().await.expect("Ping must return timestamp");
    assert!(pong_ts > 0);

    // 4. Graceful Stop
    let shutdown_res = client.shutdown_daemon().await;
    assert!(shutdown_res.is_ok(), "Shutdown command must succeed");

    // Wait for server task to exit
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), server_task).await;

    // 5. Subsequent connect should fail
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        OmenClient::connect_to(&endpoint, None).await.is_err(),
        "Daemon should no longer accept connections after shutdown"
    );
}

fn gremlin_exe() -> std::path::PathBuf {
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
    let fallback = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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
    let status = std::process::Command::new("cargo")
        .args(["build", "--bin", "omen-gremlin"])
        .status()
        .expect("failed to build omen-gremlin fixture");
    assert!(status.success(), "omen-gremlin build failed");
    fallback
}

/// E2.5: the CLI cancel surface projects the same termination truth as the
/// daemon/IPC/MCP surfaces for one execution identity.
#[tokio::test(flavor = "multi_thread")]
async fn test_cli_cancel_confirms_live_stop_with_same_identity() {
    let _guard = ENDPOINT_LOCK.lock().expect("endpoint lock poisoned");
    // Serialized by ENDPOINT_LOCK: process-global env mutation is sound.
    let state_home = tempdir().unwrap();
    unsafe {
        std::env::set_var("OMEN_STATE_HOME", state_home.path());
    }

    let endpoint = default_endpoint_address();
    if let Ok(c) = OmenClient::connect_to(&endpoint, None).await {
        let _ = c.shutdown_daemon().await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    let server = DaemonServer::new(Some(endpoint.clone()));
    let server_task = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let mut ready = false;
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(client) = OmenClient::connect_to(&endpoint, None).await
            && client.ping().await.is_ok()
        {
            ready = true;
            break;
        }
    }
    assert!(ready, "Daemon server failed to become ready");

    let workspace = tempdir().unwrap();
    let ws_string = workspace.path().to_str().unwrap().to_string();
    let client = OmenClient::connect_to(&endpoint, None).await.unwrap();
    client.attach_workspace(&ws_string).await.unwrap();

    let gremlin = gremlin_exe().to_string_lossy().to_string();
    let ws_for_task = ws_string.clone();
    tokio::spawn(async move {
        let _ = client
            .submit_consequential_execution(
                "req-cli-cancel-01",
                "exec",
                gremlin,
                vec!["--sleep-ms".into(), "30000".into()],
                ws_for_task,
                60000,
            )
            .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    let exec_id = {
        let db = omen_knowledge::Database::open(&omen_knowledge::canonical_workspace_db_path(
            workspace.path(),
        ))
        .unwrap();
        omen_knowledge::WorkspacePersistence::get_request_receipt(&db, "req-cli-cancel-01")
            .unwrap()
            .unwrap()
            .execution_id
            .unwrap()
    };

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_omen"))
        .env("OMEN_STATE_HOME", state_home.path())
        .args(["--workspace", &ws_string, "--machine", "cancel", &exec_id])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "omen cancel must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(record["execution_id"], exec_id);
    assert_eq!(record["outcome"]["outcome"], "TerminationConfirmed");

    let verifier = OmenClient::connect_to(&endpoint, None).await.unwrap();
    verifier.attach_workspace(&ws_string).await.unwrap();
    let status = verifier
        .query_request_status("req-cli-cancel-01")
        .await
        .unwrap();
    assert_eq!(status.status, omen_ipc::ExecutionStatusCode::Cancelled);

    let _ = verifier.shutdown_daemon().await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server_task).await;
    unsafe {
        std::env::remove_var("OMEN_STATE_HOME");
    }
}
