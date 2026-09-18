use omen_client::OmenClient;
use omen_daemon::DaemonServer;
use omen_ipc::{LocalIpcError, PlatformStream};
use tempfile::tempdir;
use tokio::sync::watch;

#[tokio::test]
async fn test_client_multiplexed_concurrent_requests() {
    let (client_stream, server_stream) = PlatformStream::duplex_pair(8192);
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

    // Fire 20 concurrent pings and snapshots
    let mut handles = Vec::new();
    for _ in 0..20 {
        let c = client.clone();
        handles.push(tokio::spawn(async move {
            let p = c.ping().await;
            assert!(p.is_ok());
            let s = c.get_snapshot().await;
            assert!(s.is_ok());
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
}

#[tokio::test]
async fn test_client_disconnect_detection() {
    let (client_stream, server_stream) = PlatformStream::duplex_pair(4096);
    let server = DaemonServer::new(Some("test-endpoint".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(server_stream, instance_id, registry, shutdown_rx)
            .await;
    });

    let client = OmenClient::from_stream(client_stream, None, None)
        .await
        .unwrap();

    assert!(client.is_connected());

    // Trigger server shutdown
    let _ = shutdown_tx.send(true);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Subsequent request should detect disconnect
    let ping_res = client.ping().await;
    assert!(ping_res.is_err());
    assert!(!client.is_connected());
}

#[tokio::test]
async fn test_client_service_not_found_error() {
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

    let stop_res = client.stop_service("nonexistent-service").await;
    match stop_res {
        Err(LocalIpcError::ServiceNotFound(msg)) => {
            assert!(msg.contains("nonexistent-service"));
        }
        other => panic!("Expected ServiceNotFound, got {other:?}"),
    }
}
