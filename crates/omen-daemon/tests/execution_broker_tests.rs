use omen_client::OmenClient;
use omen_daemon::DaemonServer;
use omen_ipc::{ExecutionStatusCode, PlatformStream};
use omen_knowledge::cas::ContentAddressedStore;
use omen_knowledge::resolve_workspace_dir;
use std::path::PathBuf;
use tempfile::tempdir;
use tokio::sync::watch;

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

    // Build on demand if not present
    let status = std::process::Command::new("cargo")
        .args(["build", "--bin", "omen-gremlin"])
        .status()
        .expect("failed to build omen-gremlin fixture");
    assert!(status.success(), "omen-gremlin build failed");
    fallback
}

#[tokio::test]
async fn test_execution_broker_dispatch_and_deduplication() {
    let server = DaemonServer::new(Some("broker-test-endpoint".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let tmp = tempdir().unwrap();
    let path = tmp.path().to_str().unwrap();
    let gremlin = gremlin_exe();

    // 1. Connect Client
    let (c_stream, s_stream) = PlatformStream::duplex_pair(4096);
    let reg = registry.clone();
    let inst = instance_id.clone();
    let rx = shutdown_rx.clone();
    tokio::spawn(async move {
        DaemonServer::handle_connection(s_stream, inst, reg, rx)
            .await
            .unwrap();
    });

    let client = OmenClient::from_stream(c_stream, None, Some("sess_tester".to_string()))
        .await
        .expect("Client should connect");
    client.attach_workspace(path).await.unwrap();

    // 2. Submit initial execution with deduplication key
    let dedup_key = "req_consequential_alpha";
    let summary1 = client
        .submit_consequential_execution(
            dedup_key,
            "exec",
            gremlin.to_string_lossy(),
            vec!["--stdout".into(), "hello-broker".into()],
            path,
            10000,
        )
        .await
        .expect("Execution should succeed");

    assert_eq!(summary1.exit_code, Some(0));
    assert!(summary1.stdout_preview.contains("hello-broker"));
    assert!(summary1.execution_id.starts_with("exec_"));

    // 3. Deduplication: submit again with the same consequential_request_id
    let summary2 = client
        .submit_consequential_execution(
            dedup_key,
            "exec",
            gremlin.to_string_lossy(),
            vec!["--stdout".into(), "hello-broker".into()],
            path,
            10000,
        )
        .await
        .expect("Duplicate execution request should return cached result");

    assert_eq!(
        summary1.execution_id, summary2.execution_id,
        "Deduplicated request must return identical execution_id"
    );
    assert_eq!(summary1.exit_code, summary2.exit_code);

    // 4. Query request status for dedup key
    let status_rec = client
        .query_request_status(dedup_key)
        .await
        .expect("Status query should succeed");
    assert_eq!(status_rec.consequential_request_id, dedup_key);
    assert_eq!(status_rec.status, ExecutionStatusCode::Completed);
    assert_eq!(status_rec.execution_id, Some(summary1.execution_id));
}

#[tokio::test]
async fn test_execution_broker_disconnect_and_reconnect_status() {
    let server = DaemonServer::new(Some("broker-disconnect-test".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let tmp = tempdir().unwrap();
    let path = tmp.path().to_str().unwrap();
    let gremlin = gremlin_exe();

    // 1. Client 1 connects and initiates a command that takes 500ms
    let (c1_stream, s1_stream) = PlatformStream::duplex_pair(4096);
    let reg1 = registry.clone();
    let inst1 = instance_id.clone();
    let rx1 = shutdown_rx.clone();
    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(s1_stream, inst1, reg1, rx1).await;
    });

    let client1 = OmenClient::from_stream(c1_stream, None, Some("sess_client_1".to_string()))
        .await
        .unwrap();
    client1.attach_workspace(path).await.unwrap();

    let dedup_key = "req_consequential_disconnect";
    let gremlin_path = gremlin.to_string_lossy().to_string();
    let ws_path = path.to_string();

    // Submit execution and disconnect shortly after starting
    let client1_clone = client1;
    let dedup_clone = dedup_key.to_string();
    tokio::spawn(async move {
        let _ = client1_clone
            .submit_consequential_execution(
                dedup_clone,
                "exec",
                gremlin_path,
                vec![
                    "--sleep-ms".into(),
                    "500".into(),
                    "--stdout".into(),
                    "detached-output".into(),
                ],
                ws_path,
                10000,
            )
            .await;
    });

    // Wait a brief moment for task to be spawned, then drop client1 (simulating disconnect)
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Wait for the background execution to finish on the daemon
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;

    // 2. Client 2 connects to the same workspace
    let (c2_stream, s2_stream) = PlatformStream::duplex_pair(4096);
    let reg2 = registry.clone();
    let inst2 = instance_id.clone();
    let rx2 = shutdown_rx.clone();
    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(s2_stream, inst2, reg2, rx2).await;
    });

    let client2 = OmenClient::from_stream(c2_stream, None, Some("sess_client_2".to_string()))
        .await
        .unwrap();
    client2.attach_workspace(path).await.unwrap();

    // Query status of the execution initiated by client 1
    let status = client2.query_request_status(dedup_key).await.unwrap();
    assert_eq!(status.status, ExecutionStatusCode::Completed);
    assert!(status.execution_id.is_some());

    // Re-submitting returns the cached result without re-executing
    let summary = client2
        .submit_consequential_execution(
            dedup_key,
            "exec",
            gremlin.to_string_lossy(),
            vec![],
            path,
            10000,
        )
        .await
        .unwrap();
    assert_eq!(summary.execution_id, status.execution_id.unwrap());
    assert_eq!(summary.exit_code, Some(0));
}

#[tokio::test]
async fn test_execution_broker_large_output_spools_to_cas() {
    let server = DaemonServer::new(Some("broker-cas-test".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let tmp = tempdir().unwrap();
    let path = tmp.path().to_str().unwrap();
    let gremlin = gremlin_exe();

    let (c_stream, s_stream) = PlatformStream::duplex_pair(4096);
    let reg = registry.clone();
    let inst = instance_id.clone();
    let rx = shutdown_rx.clone();
    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(s_stream, inst, reg, rx).await;
    });

    let client = OmenClient::from_stream(c_stream, None, Some("sess_cas".to_string()))
        .await
        .unwrap();
    client.attach_workspace(path).await.unwrap();

    // Request 16384 bytes of stdout (> 8192 inline budget)
    let summary = client
        .submit_consequential_execution(
            "req_consequential_large_output",
            "exec",
            gremlin.to_string_lossy(),
            vec!["--stdout-bytes".into(), "16384".into()],
            path,
            10000,
        )
        .await
        .expect("Execution should succeed");

    assert_eq!(summary.exit_code, Some(0));
    assert!(
        summary.stdout_artifact.is_some(),
        "Large output (>8192 bytes) must spool to CAS artifact"
    );

    let artifact_uri = summary.stdout_artifact.unwrap();
    assert!(
        artifact_uri.starts_with("artifact://sha256/"),
        "Artifact URI must be artifact://sha256/..., got: {artifact_uri}"
    );

    // Verify artifact is inspectable in CAS
    let state_dir = resolve_workspace_dir(tmp.path());
    let cas = ContentAddressedStore::new(state_dir.join("cas"));
    let digest = artifact_uri.trim_start_matches("artifact://sha256/");
    let db = omen_knowledge::Database::open(&state_dir.join("knowledge.db")).unwrap();
    let meta = cas
        .inspect(&db, digest)
        .expect("Artifact must exist in CAS");
    assert_eq!(meta.size, 16384);
}
