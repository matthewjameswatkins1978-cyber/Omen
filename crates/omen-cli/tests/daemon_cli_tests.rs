use omen_client::OmenClient;
use omen_daemon::DaemonServer;
use omen_ipc::default_endpoint_address;

#[tokio::test]
async fn test_daemon_cli_status_ping_and_stop_lifecycle() {
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
