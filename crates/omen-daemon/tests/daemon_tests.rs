use omen_client::OmenClient;
use omen_daemon::DaemonServer;
use omen_ipc::{FactInfo, LocalIpcError, PlatformStream};
use tempfile::tempdir;
use tokio::sync::watch;

#[tokio::test]
async fn test_daemon_client_handshake_and_ping() {
    let (client_stream, server_stream) = PlatformStream::duplex_pair(4096);
    let server = DaemonServer::new(Some("test-endpoint".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    // Spawn server connection handler
    tokio::spawn(async move {
        DaemonServer::handle_connection(server_stream, instance_id, registry, shutdown_rx)
            .await
            .unwrap();
    });

    // Connect client
    let client = OmenClient::from_stream(client_stream, None, None)
        .await
        .expect("Client should connect and handshake");

    assert!(client.is_connected());

    // Ping
    let pong_ts = client.ping().await.expect("Ping should succeed");
    assert!(pong_ts > 0);
}

#[tokio::test]
async fn test_daemon_workspace_attach_and_snapshot() {
    let (client_stream, server_stream) = PlatformStream::duplex_pair(4096);
    let server = DaemonServer::new(Some("test-endpoint".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(server_stream, instance_id, registry, shutdown_rx)
            .await;
    });

    let client = OmenClient::from_stream(client_stream, None, None)
        .await
        .unwrap();

    let tmp = tempdir().unwrap();
    let path = tmp.path().to_str().unwrap();

    let (ws_id, epoch) = client.attach_workspace(path).await.unwrap();
    assert!(ws_id.starts_with("ws_"));
    assert_eq!(epoch, server.epoch());
    assert_eq!(client.active_workspace_id().await, Some(ws_id.clone()));

    // Get snapshot
    let snapshot = client.get_snapshot().await.unwrap();
    assert_eq!(snapshot.workspace_id, ws_id);
    assert_eq!(snapshot.epoch, epoch);
    assert!(snapshot.facts.is_empty());
    assert!(snapshot.tools.contains(&"cargo".to_string()));
}

#[tokio::test]
async fn test_daemon_event_broadcasting_to_client() {
    let (client_stream, server_stream) = PlatformStream::duplex_pair(4096);
    let server = DaemonServer::new(Some("test-endpoint".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(server_stream, instance_id, registry, shutdown_rx)
            .await;
    });

    let client = OmenClient::from_stream(client_stream, None, None)
        .await
        .unwrap();

    let mut event_rx = client.subscribe_events();

    let tmp = tempdir().unwrap();
    let (ws_id, _) = client
        .attach_workspace(tmp.path().to_str().unwrap())
        .await
        .unwrap();

    // Now put a fact directly in the server workspace state to trigger an event broadcast
    let ws_state = server.registry().get_by_id(&ws_id).await.unwrap();
    ws_state
        .put_fact(FactInfo {
            fact_id: "fact://tests/suite".to_string(),
            resource_uri: "fact://tests/all".to_string(),
            value: "passed".to_string(),
            validity: "CURRENT".to_string(),
            assurance: "MACHINE_VERIFIED".to_string(),
        })
        .await;

    // Receive broadcasted FactPublished event
    let event1 = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
        .await
        .expect("Event timeout")
        .expect("Receive event");

    match event1.payload {
        omen_ipc::EventPayload::FactPublished {
            fact_id,
            resource_uri,
            validity,
            ..
        } => {
            assert_eq!(fact_id, "fact://tests/suite");
            assert_eq!(resource_uri, "fact://tests/all");
            assert_eq!(validity, "CURRENT");
        }
        other => panic!("Expected FactPublished, got {other:?}"),
    }

    // Now invalidate the fact
    ws_state
        .invalidate_fact("fact://tests/all", "fs mutation")
        .await;

    // Receive broadcasted FactInvalidated event
    let event2 = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
        .await
        .expect("Event timeout")
        .expect("Receive event");

    match event2.payload {
        omen_ipc::EventPayload::FactInvalidated {
            resource_uri,
            new_validity,
            cause,
            ..
        } => {
            assert_eq!(resource_uri, "fact://tests/all");
            assert_eq!(new_validity, "DIRTY");
            assert_eq!(cause, "fs mutation");
        }
        other => panic!("Expected FactInvalidated, got {other:?}"),
    }
}

#[tokio::test]
async fn test_daemon_services_lifecycle() {
    let (client_stream, server_stream) = PlatformStream::duplex_pair(4096);
    let server = DaemonServer::new(Some("test-endpoint".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(server_stream, instance_id, registry, shutdown_rx)
            .await;
    });

    let client = OmenClient::from_stream(client_stream, None, None)
        .await
        .unwrap();

    let tmp = tempdir().unwrap();
    client
        .attach_workspace(tmp.path().to_str().unwrap())
        .await
        .unwrap();

    let mut gremlin_path = std::env::current_exe().expect("current exe");
    gremlin_path.pop();
    if gremlin_path.ends_with("deps") {
        gremlin_path.pop();
    }
    let gremlin_name = if cfg!(windows) {
        "omen-gremlin.exe"
    } else {
        "omen-gremlin"
    };
    let gremlin = gremlin_path
        .join(gremlin_name)
        .to_string_lossy()
        .to_string();

    // Start service
    let svc = client
        .start_service("web", &gremlin, vec!["--sleep-ms".into(), "10000".into()])
        .await
        .unwrap();
    assert_eq!(svc.name, "web");
    assert_eq!(svc.state, "running");

    // Starting already running service must fail with ServiceAlreadyRunning
    let duplicate_res = client
        .start_service("web", &gremlin, vec!["--sleep-ms".into(), "10000".into()])
        .await;
    match duplicate_res {
        Err(LocalIpcError::ServiceAlreadyRunning(msg)) => {
            assert!(msg.contains("web"));
        }
        other => panic!("Expected ServiceAlreadyRunning error, got {other:?}"),
    }

    // List services
    let services = client.list_services().await.unwrap();
    assert_eq!(services.len(), 1);
    assert_eq!(services[0].name, "web");

    // Stop service
    let stopped = client.stop_service("web").await.unwrap();
    assert_eq!(stopped, "web");

    // Restart service
    let restarted = client.restart_service("web").await.unwrap();
    assert_eq!(restarted.name, "web");
    assert_eq!(restarted.state, "running");
}

#[tokio::test]
async fn test_handshake_unsupported_version_rejection() {
    let (mut client_stream, server_stream) = PlatformStream::duplex_pair(4096);
    let server = DaemonServer::new(Some("test-endpoint".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(server_stream, instance_id, registry, shutdown_rx)
            .await;
    });

    // Send client hello with unsupported version (999)
    let hello = omen_ipc::ClientHello {
        protocol_version_family: "omen.local-ipc".to_string(),
        supported_versions: vec![999],
        client_instance_id: "test-client".to_string(),
        product_version: "0.4.0".to_string(),
        platform: "test".to_string(),
        requested_features: vec![],
    };
    omen_ipc::write_json_frame(&mut client_stream, &hello)
        .await
        .unwrap();

    // Server should close the stream without sending DaemonHello
    let res: Result<Option<omen_ipc::DaemonHello>, _> =
        omen_ipc::read_json_frame(&mut client_stream).await;
    assert!(res.is_ok());
    assert!(res.unwrap().is_none());
}
