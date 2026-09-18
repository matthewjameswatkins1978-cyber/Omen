use omen_client::OmenClient;
use omen_daemon::DaemonServer;
use omen_ipc::{FactInfo, IpcEvent, PlatformStream};
use std::fs;
use tempfile::tempdir;
use tokio::sync::watch;

#[tokio::test]
async fn test_snapshot_consistency_across_clients() {
    let server = DaemonServer::new(Some("test-snapshot".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    // Client 1
    let (c1_stream, s1_stream) = PlatformStream::duplex_pair(4096);
    let inst1 = instance_id.clone();
    let reg1 = registry.clone();
    let rx1 = shutdown_rx.clone();
    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(s1_stream, inst1, reg1, rx1).await;
    });
    let client1 = OmenClient::from_stream(c1_stream, None, None)
        .await
        .unwrap();

    // Client 2
    let (c2_stream, s2_stream) = PlatformStream::duplex_pair(4096);
    let inst2 = instance_id.clone();
    let reg2 = registry.clone();
    let rx2 = shutdown_rx.clone();
    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(s2_stream, inst2, reg2, rx2).await;
    });
    let client2 = OmenClient::from_stream(c2_stream, None, None)
        .await
        .unwrap();

    let tmp = tempdir().unwrap();
    let path = tmp.path().to_str().unwrap();

    client1.attach_workspace(path).await.unwrap();
    client2.attach_workspace(path).await.unwrap();

    // Publish fact into shared workspace
    let ws_state = registry.get_or_attach(tmp.path()).await;
    ws_state
        .put_fact(FactInfo {
            fact_id: "fact://cargo/check".to_string(),
            resource_uri: "fact://build/status".to_string(),
            value: "pass".to_string(),
            validity: "CURRENT".to_string(),
            assurance: "MACHINE_VERIFIED".to_string(),
        })
        .await;

    // Both clients get identical snapshot
    let snap1 = client1.get_snapshot().await.unwrap();
    let snap2 = client2.get_snapshot().await.unwrap();

    assert_eq!(snap1.workspace_id, snap2.workspace_id);
    assert_eq!(snap1.epoch, snap2.epoch);
    assert_eq!(snap1.facts.len(), 1);
    assert_eq!(snap2.facts.len(), 1);
    assert_eq!(snap1.facts[0].value, "pass");
    assert_eq!(snap2.facts[0].value, "pass");
}

#[tokio::test]
async fn test_fs_mutation_invalidates_current_facts() {
    let server = DaemonServer::new(Some("test-invalidation".to_string()));
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
    let file_to_change = tmp.path().join("source.rs");
    fs::write(&file_to_change, "fn main() {}").unwrap();

    let mut event_rx = client.subscribe_events();

    client
        .attach_workspace(tmp.path().to_str().unwrap())
        .await
        .unwrap();

    let ws = server.registry().get_or_attach(tmp.path()).await;

    // Publish fact with CURRENT validity
    ws.put_fact(FactInfo {
        fact_id: "fact://compiler/output".to_string(),
        resource_uri: "fact://build/binary".to_string(),
        value: "ok".to_string(),
        validity: "CURRENT".to_string(),
        assurance: "MACHINE_VERIFIED".to_string(),
    })
    .await;

    // Receive FactPublished
    let _ = event_rx.recv().await.unwrap();

    // Trigger external file mutation directly via helper or filesystem write
    ws.invalidate_all_current_facts("fs:external mutation").await;

    // Client must receive FactInvalidated event
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
        .await
        .expect("Timeout waiting for FactInvalidated")
        .expect("Receive FactInvalidated");

    match event.payload {
        omen_ipc::EventPayload::FactInvalidated {
            fact_id,
            new_validity,
            cause,
            ..
        } => {
            assert_eq!(fact_id, "fact://compiler/output");
            assert_eq!(new_validity, "DIRTY");
            assert_eq!(cause, "fs:external mutation");
        }
        other => panic!("Expected FactInvalidated, got {other:?}"),
    }

    // Query fact now returns DIRTY
    let snap = client.get_snapshot().await.unwrap();
    assert_eq!(snap.dirty_facts_count, 1);
    assert_eq!(snap.facts[0].validity, "DIRTY");
}

#[tokio::test]
async fn test_event_sequence_gap_triggers_resync_required() {
    let (client_stream, mut server_stream) = PlatformStream::duplex_pair(4096);

    // Spawn handshake on server side
    tokio::spawn(async move {
        let _hello: omen_ipc::ClientHello =
            omen_ipc::read_json_frame(&mut server_stream).await.unwrap().unwrap();
        let daemon_hello = omen_ipc::DaemonHello {
            selected_protocol_version: 1,
            daemon_instance_id: "dmn_test".into(),
            product_version: "0.4.0".into(),
            supported_features: vec!["events".into()],
            max_frame_size: 1024 * 1024,
        };
        omen_ipc::write_json_frame(&mut server_stream, &daemon_hello).await.unwrap();

        // Send sequence 1 event
        let ev1 = IpcEvent::new(
            "ws_test",
            1,
            1,
            omen_ipc::EventPayload::FactPublished {
                fact_id: "fact://1".into(),
                resource_uri: "fact://res1".into(),
                validity: "CURRENT".into(),
                assurance: "VERIFIED".into(),
            },
        );
        omen_ipc::write_json_frame(&mut server_stream, &omen_ipc::DaemonMessage::Event(ev1))
            .await
            .unwrap();

        // ARTIFICIAL GAP: skip sequences 2, 3, 4 and send sequence 5!
        let ev5 = IpcEvent::new(
            "ws_test",
            1,
            5,
            omen_ipc::EventPayload::FactInvalidated {
                fact_id: "fact://1".into(),
                resource_uri: "fact://res1".into(),
                previous_validity: "CURRENT".into(),
                new_validity: "DIRTY".into(),
                cause: "gap_test".into(),
            },
        );
        omen_ipc::write_json_frame(&mut server_stream, &omen_ipc::DaemonMessage::Event(ev5))
            .await
            .unwrap();
    });

    let client = OmenClient::from_stream(client_stream, None, None)
        .await
        .unwrap();
    let mut event_rx = client.subscribe_events();

    // 1. First event received (seq 1)
    let ev1 = event_rx.recv().await.unwrap();
    assert_eq!(ev1.sequence, 1);

    // 2. Second received event must be ResyncRequired synthesized by client due to detected gap (seq 1 -> 5)
    let gap_event = event_rx.recv().await.unwrap();
    match gap_event.payload {
        omen_ipc::EventPayload::ResyncRequired { reason } => {
            assert!(reason.contains("Event gap detected"));
        }
        other => panic!("Expected ResyncRequired, got {other:?}"),
    }

    // 3. Then the actual sequence 5 event arrives
    let ev5 = event_rx.recv().await.unwrap();
    assert_eq!(ev5.sequence, 5);
}
