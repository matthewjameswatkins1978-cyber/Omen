use omen_client::OmenClient;
use omen_daemon::DaemonServer;
use omen_ipc::{ExecutionStatusCode, FactInfo, PlatformStream};
use tempfile::tempdir;
use tokio::sync::watch;

#[tokio::test]
async fn test_multi_workspace_isolation() {
    let server = DaemonServer::new(Some("test-isolation".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    // Workspace A client
    let (client_a_stream, server_a_stream) = PlatformStream::duplex_pair(4096);
    let inst_a = instance_id.clone();
    let reg_a = registry.clone();
    let rx_a = shutdown_rx.clone();
    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(server_a_stream, inst_a, reg_a, rx_a).await;
    });
    let client_a = OmenClient::from_stream(client_a_stream, None, None)
        .await
        .unwrap();

    // Workspace B client
    let (client_b_stream, server_b_stream) = PlatformStream::duplex_pair(4096);
    let inst_b = instance_id.clone();
    let reg_b = registry.clone();
    let rx_b = shutdown_rx.clone();
    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(server_b_stream, inst_b, reg_b, rx_b).await;
    });
    let client_b = OmenClient::from_stream(client_b_stream, None, None)
        .await
        .unwrap();

    let tmp_a = tempdir().unwrap();
    let tmp_b = tempdir().unwrap();

    let (ws_a_id, _) = client_a
        .attach_workspace(tmp_a.path().to_str().unwrap())
        .await
        .unwrap();
    let (ws_b_id, _) = client_b
        .attach_workspace(tmp_b.path().to_str().unwrap())
        .await
        .unwrap();

    // Verify workspace IDs are distinct
    assert_ne!(ws_a_id, ws_b_id);

    let mut events_b = client_b.subscribe_events();

    // 1. Publish fact into Workspace A
    let ws_a_state = registry.get_by_id(&ws_a_id).await.unwrap();
    ws_a_state
        .put_fact(FactInfo {
            fact_id: "fact://workspace_a/secret".to_string(),
            resource_uri: "fact://a/data".to_string(),
            value: "secret_value".to_string(),
            validity: "CURRENT".to_string(),
            assurance: "MACHINE_VERIFIED".to_string(),
        })
        .await;

    // Workspace B must NOT receive any event from Workspace A
    let recv_timeout =
        tokio::time::timeout(std::time::Duration::from_millis(150), events_b.recv()).await;
    assert!(
        recv_timeout.is_err(),
        "Workspace B leaked an event from Workspace A!"
    );

    // Workspace B must NOT find Workspace A's fact
    let fact_query_b = client_b.query_fact("fact://a/data").await.unwrap();
    assert!(
        fact_query_b.is_none(),
        "Workspace B accessed Workspace A's fact!"
    );

    // 2. Start service in Workspace A
    client_a
        .start_service("srv_a", "cargo", vec!["build".into()])
        .await
        .unwrap();

    // Workspace B must NOT see Workspace A's service
    let services_b = client_b.list_services().await.unwrap();
    assert!(
        services_b.is_empty(),
        "Workspace B leaked Workspace A's services!"
    );
}

#[tokio::test]
async fn test_request_receipt_tracking_and_status() {
    let server = DaemonServer::new(Some("test-receipts".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let (client_stream, server_stream) = PlatformStream::duplex_pair(4096);
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

    // Query non-existent request ID
    let not_seen = client
        .query_request_status("req_nonexistent")
        .await
        .unwrap();
    assert_eq!(not_seen.status, ExecutionStatusCode::NotSeen);
    assert!(not_seen.execution_id.is_none());

    // Submit execution with deterministic request ID via low-level request
    let test_req_id = "req_consequential_001".to_string();
    let resp = client
        .send_request_with_id(
            &test_req_id,
            omen_ipc::RequestPayload::SubmitExecution {
                tool: "cargo".into(),
                operation: "--version".into(),
                args: vec![],
                cwd: tmp.path().to_str().unwrap().into(),
                timeout_ms: 5000,
                consequential_request_id: Some(test_req_id.clone()),
            },
        )
        .await
        .unwrap();

    let exec_id = match resp {
        omen_ipc::ResponsePayload::ExecutionFinished(summary) => {
            assert!(summary.execution_id.starts_with("exec_"));
            assert_eq!(summary.exit_code, Some(0));
            summary.execution_id
        }
        other => panic!("Expected ExecutionFinished, got {other:?}"),
    };

    // Query status using the consequential request ID
    let status_rec = client.query_request_status(&test_req_id).await.unwrap();
    assert_eq!(status_rec.status, ExecutionStatusCode::Completed);
    assert_eq!(status_rec.execution_id, Some(exec_id));
}
