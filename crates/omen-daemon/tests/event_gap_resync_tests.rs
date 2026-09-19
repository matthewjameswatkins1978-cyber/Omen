use std::sync::Arc;
use tempfile::tempdir;

use omen_client::OmenClient;
use omen_daemon::DaemonServer;
use omen_ipc::{EventPayload, IpcEvent, PlatformStream};
use omen_test_fixtures::{INTEGRATION_TIMEOUT, run_with_test_timeout, wait_for_event};

#[tokio::test(flavor = "multi_thread")]
async fn test_real_sequence_gap_triggers_resync() {
    run_with_test_timeout(
        "test_real_sequence_gap_triggers_resync",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_DAEMON");
            let temp = tempdir().unwrap();
            let daemon = Arc::new(DaemonServer::new(Some("duplex://gap_test".into())));
            ctx.set_daemon_status("running");

            ctx.phase("CONNECT_CLIENT");
            let (client_stream, daemon_stream) = PlatformStream::duplex_pair(65536);
            let instance_id = daemon.instance_id().to_string();
            let registry = daemon.registry();
            let shutdown_rx = daemon.subscribe_shutdown();

            tokio::spawn(async move {
                let _ = DaemonServer::handle_connection(
                    daemon_stream,
                    instance_id,
                    registry,
                    shutdown_rx,
                )
                .await;
            });

            let client = OmenClient::from_stream(
                client_stream,
                Some("memory://gap_test".into()),
                Some("session_gap".into()),
            )
            .await
            .unwrap();

            let ws_path = temp.path().to_str().unwrap();
            client.attach_workspace(ws_path).await.unwrap();
            ctx.set_client_status("connected");

            let mut event_rx = client.subscribe_events();
            let ws = daemon.registry().get_or_attach(temp.path()).await.unwrap();

            ctx.phase("SEND_SEQUENCE_1");
            // 1. Send seq 1
            let ev1 = IpcEvent::new(
                ws.workspace_id(),
                ws.epoch(),
                1,
                EventPayload::FactPublished {
                    fact_id: "fact-seq-1".into(),
                    resource_uri: "res://1".into(),
                    validity: "CURRENT".into(),
                    assurance: "VERIFIED".into(),
                },
            );
            let _ = ws.broadcast_raw_event(ev1);

            ctx.phase("WAIT_FOR_SEQUENCE_1");
            let received_ev1 = wait_for_event(
                &mut event_rx,
                INTEGRATION_TIMEOUT,
                "sequence 1 event",
                |e| e.sequence == 1,
            )
            .await
            .unwrap();
            assert_eq!(received_ev1.sequence, 1);
            ctx.record_event("Sequence1Received");

            ctx.phase("INJECT_SEQUENCE_GAP_AND_SEND_3");
            // 2. Skip seq 2 and send seq 3 directly to simulate a network/pipe dropped sequence
            let ev3 = IpcEvent::new(
                ws.workspace_id(),
                ws.epoch(),
                3,
                EventPayload::FactPublished {
                    fact_id: "fact-seq-3".into(),
                    resource_uri: "res://3".into(),
                    validity: "CURRENT".into(),
                    assurance: "VERIFIED".into(),
                },
            );
            let _ = ws.broadcast_raw_event(ev3);

            ctx.phase("WAIT_FOR_RESYNC_REQUIRED");
            // The client's background reader MUST detect sequence gap (3 > 1 + 1)
            // and inject ResyncRequired before delivering seq 3
            let resync_ev = wait_for_event(
                &mut event_rx,
                INTEGRATION_TIMEOUT,
                "ResyncRequired event",
                |e| matches!(e.payload, EventPayload::ResyncRequired { .. }),
            )
            .await
            .unwrap();

            match resync_ev.payload {
                EventPayload::ResyncRequired { reason } => {
                    assert!(
                        reason.contains("Event gap detected: expected 2, received 3"),
                        "Expected gap detection reason, got: {reason}"
                    );
                }
                other => panic!("Expected ResyncRequired event, got {other:?}"),
            }
            ctx.record_event("ResyncRequiredReceived");

            ctx.phase("WAIT_FOR_SEQUENCE_3");
            let received_ev3 = wait_for_event(
                &mut event_rx,
                INTEGRATION_TIMEOUT,
                "sequence 3 event",
                |e| e.sequence == 3,
            )
            .await
            .unwrap();
            assert_eq!(received_ev3.sequence, 3);
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_daemon_epoch_change_forces_resnapshot() {
    run_with_test_timeout(
        "test_daemon_epoch_change_forces_resnapshot",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_DAEMON");
            let temp = tempdir().unwrap();
            let daemon = Arc::new(DaemonServer::new(Some("duplex://epoch_test".into())));
            ctx.set_daemon_status("running");

            ctx.phase("CONNECT_CLIENT");
            let (client_stream, daemon_stream) = PlatformStream::duplex_pair(65536);
            let instance_id = daemon.instance_id().to_string();
            let registry = daemon.registry();
            let shutdown_rx = daemon.subscribe_shutdown();

            tokio::spawn(async move {
                let _ = DaemonServer::handle_connection(
                    daemon_stream,
                    instance_id,
                    registry,
                    shutdown_rx,
                )
                .await;
            });

            let client = OmenClient::from_stream(
                client_stream,
                Some("memory://epoch_test".into()),
                Some("session_epoch".into()),
            )
            .await
            .unwrap();

            let ws_path = temp.path().to_str().unwrap();
            client.attach_workspace(ws_path).await.unwrap();
            ctx.set_client_status("connected");

            let mut event_rx = client.subscribe_events();
            let ws = daemon.registry().get_or_attach(temp.path()).await.unwrap();

            ctx.phase("SEND_EPOCH_1_EVENT");
            // 1. Send event with epoch 1
            let ev1 = IpcEvent::new(
                ws.workspace_id(),
                1,
                1,
                EventPayload::FactPublished {
                    fact_id: "fact-epoch-1".into(),
                    resource_uri: "res://epoch".into(),
                    validity: "CURRENT".into(),
                    assurance: "VERIFIED".into(),
                },
            );
            let _ = ws.broadcast_raw_event(ev1);

            let _ = wait_for_event(&mut event_rx, INTEGRATION_TIMEOUT, "epoch 1 event", |e| {
                e.epoch == 1
            })
            .await
            .unwrap();

            ctx.phase("SEND_EPOCH_2_EVENT");
            // 2. Send event with epoch 2 (simulating daemon restarted with new epoch)
            let ev2 = IpcEvent::new(
                ws.workspace_id(),
                2,
                1,
                EventPayload::FactPublished {
                    fact_id: "fact-epoch-2".into(),
                    resource_uri: "res://epoch".into(),
                    validity: "CURRENT".into(),
                    assurance: "VERIFIED".into(),
                },
            );
            let _ = ws.broadcast_raw_event(ev2);

            ctx.phase("WAIT_FOR_RESYNC_REQUIRED");
            // The client MUST detect epoch mismatch and inject ResyncRequired
            let resync_ev = wait_for_event(
                &mut event_rx,
                INTEGRATION_TIMEOUT,
                "epoch change ResyncRequired event",
                |e| matches!(e.payload, EventPayload::ResyncRequired { .. }),
            )
            .await
            .unwrap();

            match resync_ev.payload {
                EventPayload::ResyncRequired { reason } => {
                    assert!(
                        reason.contains("Epoch changed; daemon restarted"),
                        "Expected epoch change reason, got: {reason}"
                    );
                }
                other => panic!("Expected ResyncRequired event, got {other:?}"),
            }
        },
    )
    .await;
}
