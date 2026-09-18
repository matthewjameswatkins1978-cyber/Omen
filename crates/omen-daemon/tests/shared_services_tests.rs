use omen_client::OmenClient;
use omen_daemon::DaemonServer;
use omen_ipc::{EventPayload, LocalIpcError, PlatformStream};
use omen_knowledge::{
    Database, ServiceRecord, WorkspacePersistence, deterministic_workspace_id,
    resolve_workspace_dir,
};
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

    let status = std::process::Command::new("cargo")
        .args(["build", "--bin", "omen-gremlin"])
        .status()
        .expect("failed to build omen-gremlin fixture");
    assert!(status.success(), "omen-gremlin build failed");
    fallback
}

#[tokio::test]
async fn test_shared_services_supervision_and_two_clients() {
    let server = DaemonServer::new(Some("services-test-endpoint".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let tmp = tempdir().unwrap();
    let path = tmp.path().to_str().unwrap();
    let gremlin = gremlin_exe();

    // 1. Connect Client 1 (Human)
    let (c1_stream, s1_stream) = PlatformStream::duplex_pair(4096);
    let reg1 = registry.clone();
    let inst1 = instance_id.clone();
    let rx1 = shutdown_rx.clone();
    tokio::spawn(async move {
        DaemonServer::handle_connection(s1_stream, inst1, reg1, rx1)
            .await
            .unwrap();
    });

    let client1 = OmenClient::from_stream(c1_stream, None, Some("sess_human".to_string()))
        .await
        .unwrap();
    let mut client1_events = client1.subscribe_events();
    client1.attach_workspace(path).await.unwrap();

    // 2. Connect Client 2 (Agent)
    let (c2_stream, s2_stream) = PlatformStream::duplex_pair(4096);
    let reg2 = registry.clone();
    let inst2 = instance_id.clone();
    let rx2 = shutdown_rx.clone();
    tokio::spawn(async move {
        DaemonServer::handle_connection(s2_stream, inst2, reg2, rx2)
            .await
            .unwrap();
    });

    let client2 = OmenClient::from_stream(c2_stream, None, Some("sess_agent".to_string()))
        .await
        .unwrap();
    let mut client2_events = client2.subscribe_events();
    client2.attach_workspace(path).await.unwrap();

    // 3. Client 1 starts long-running service "api-server"
    let gremlin_str = gremlin.to_string_lossy().to_string();
    let svc1 = client1
        .start_service(
            "api-server",
            &gremlin_str,
            vec![
                "--sleep-ms".into(),
                "5000".into(),
                "--stdout".into(),
                "api-server-ready".into(),
            ],
        )
        .await
        .expect("Client 1 should start service");

    assert_eq!(svc1.name, "api-server");
    assert_eq!(svc1.state, "running");
    let original_pid = svc1.pid.expect("PID should be allocated");

    // Both clients receive ServiceStateChanged event
    let ev1 = client1_events.recv().await.unwrap();
    if let EventPayload::ServiceStateChanged { name, state, pid } = ev1.payload {
        assert_eq!(name, "api-server");
        assert_eq!(state, "running");
        assert_eq!(pid, Some(original_pid));
    } else {
        panic!("Expected ServiceStateChanged event for client 1, got {ev1:?}");
    }

    let ev2 = client2_events.recv().await.unwrap();
    if let EventPayload::ServiceStateChanged { name, state, pid } = ev2.payload {
        assert_eq!(name, "api-server");
        assert_eq!(state, "running");
        assert_eq!(pid, Some(original_pid));
    } else {
        panic!("Expected ServiceStateChanged event for client 2, got {ev2:?}");
    }

    // 4. Client 2 sees the running service in list_services
    let services = client2.list_services().await.unwrap();
    assert_eq!(services.len(), 1);
    assert_eq!(services[0].name, "api-server");
    assert_eq!(services[0].state, "running");
    assert_eq!(services[0].pid, Some(original_pid));

    // Wait a brief moment for output line to be captured in ring buffer
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Client 2 reads logs from "api-server"
    let (logs, _) = client2.service_logs("api-server", 10).await.unwrap();
    assert!(
        logs.iter().any(|l| l.contains("api-server-ready")),
        "Logs should contain child stdout: {logs:?}"
    );

    // Starting service again with the same name returns ServiceAlreadyRunning
    let duplicate_err = client1
        .start_service("api-server", &gremlin_str, vec![])
        .await
        .unwrap_err();
    assert!(
        matches!(duplicate_err, LocalIpcError::ServiceAlreadyRunning(_)),
        "Expected ServiceAlreadyRunning, got: {duplicate_err:?}"
    );

    // 5. Client 2 restarts "api-server"
    let restarted = client2.restart_service("api-server").await.unwrap();
    assert_eq!(restarted.name, "api-server");
    assert_eq!(restarted.state, "running");
    let new_pid = restarted.pid.expect("New PID should be allocated");
    assert_ne!(original_pid, new_pid, "Restart should allocate a new PID");

    // 6. Client 1 stops "api-server"
    let stopped_name = client1.stop_service("api-server").await.unwrap();
    assert_eq!(stopped_name, "api-server");

    // Client 2 checks service state
    let svcs_after = client2.list_services().await.unwrap();
    assert_eq!(svcs_after[0].state, "stopped");
}

#[tokio::test]
async fn test_service_crash_reconciliation_upon_daemon_start() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().to_str().unwrap();
    let ws_id = deterministic_workspace_id(tmp.path());
    let state_dir = resolve_workspace_dir(tmp.path());
    std::fs::create_dir_all(&state_dir).unwrap();

    // 1. Manually seed a stale "running" service with a dead PID (e.g. 999999) in SQLite
    let db_path = state_dir.join("knowledge.db");
    let db = Database::open(&db_path).unwrap();
    WorkspacePersistence::upsert_service(
        &db,
        &ServiceRecord {
            workspace_id: ws_id.clone(),
            name: "ghost-service".into(),
            command: "ghost-bin --serve".into(),
            pid: Some(999999),
            state: "running".into(),
            started_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        },
    )
    .unwrap();
    drop(db);

    // 2. Start a fresh daemon server and attach client
    let server = DaemonServer::new(Some("reconcile-test-endpoint".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let (c_stream, s_stream) = PlatformStream::duplex_pair(4096);
    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(s_stream, instance_id, registry, shutdown_rx).await;
    });

    let client = OmenClient::from_stream(c_stream, None, Some("sess_reconcile".to_string()))
        .await
        .unwrap();
    client.attach_workspace(path).await.unwrap();

    // 3. Query list_services: the stale "running" ghost service must be reconciled to "crashed"
    let services = client.list_services().await.unwrap();
    assert_eq!(services.len(), 1);
    let ghost = &services[0];
    assert_eq!(ghost.name, "ghost-service");
    assert_eq!(
        ghost.state, "crashed",
        "Dead PID recorded as running must be reconciled to 'crashed' upon startup"
    );
    assert_eq!(
        ghost.pid, None,
        "Crashed service PID should be cleared to None"
    );
}
