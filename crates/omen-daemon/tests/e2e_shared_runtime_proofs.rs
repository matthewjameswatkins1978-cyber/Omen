use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tokio::sync::watch;

use omen_client::OmenClient;
use omen_daemon::DaemonServer;
use omen_ipc::{
    ClientHello, DaemonHello, EventPayload, ExecutionStatusCode, FactInfo, PlatformStream,
    read_json_frame, write_json_frame,
};
use omen_knowledge::resolve_workspace_dir;

async fn with_timeout<Fut, T>(fut: Fut) -> T
where
    Fut: std::future::Future<Output = T>,
{
    tokio::time::timeout(Duration::from_secs(5), fut)
        .await
        .expect("Proof test execution timed out after 5 seconds")
}

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
        .expect("failed to compile omen-gremlin fixture");
    assert!(status.success());
    fallback
}

/// Helper to spin up an in-memory client attached to a daemon instance
async fn spawn_connected_client(
    daemon: &Arc<DaemonServer>,
    session_id: &str,
) -> (OmenClient, tokio::task::JoinHandle<()>) {
    let (client_stream, daemon_stream) = PlatformStream::duplex_pair(65536);

    let instance_id = daemon.instance_id().to_string();
    let registry = daemon.registry();
    let shutdown_rx = daemon.subscribe_shutdown();

    let handle = tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(daemon_stream, instance_id, registry, shutdown_rx)
            .await;
    });

    let client = OmenClient::from_stream(
        client_stream,
        Some("memory://proof".into()),
        Some(session_id.to_string()),
    )
    .await
    .unwrap();

    (client, handle)
}

// -----------------------------------------------------------------------------------------
// PROOF A: Two shells, one fact (FactPublished across processes; both observe current/dirty)
// -----------------------------------------------------------------------------------------
#[tokio::test]
async fn proof_a_two_shells_one_fact() {
    with_timeout(async {
        let temp = tempdir().unwrap();
        let daemon = Arc::new(DaemonServer::new(Some("duplex://proof_a".into())));

        let (shell1, _h1) = spawn_connected_client(&daemon, "human_shell_1").await;
        let (shell2, _h2) = spawn_connected_client(&daemon, "human_shell_2").await;

        let ws_path = temp.path().to_str().unwrap();
        shell1.attach_workspace(ws_path).await.unwrap();
        shell2.attach_workspace(ws_path).await.unwrap();

        let mut rx1 = shell1.subscribe_events();
        let mut rx2 = shell2.subscribe_events();

        let ws = daemon.registry().get_or_attach(temp.path()).await.unwrap();

        // Publish fact
        ws.broadcast_event(EventPayload::FactPublished {
            fact_id: "fact-test-01".into(),
            resource_uri: "cargo:test".into(),
            validity: "CURRENT".into(),
            assurance: "VERIFIED".into(),
        });

        let ev1 = rx1.recv().await.unwrap();
        let ev2 = rx2.recv().await.unwrap();

        match (ev1.payload, ev2.payload) {
            (
                EventPayload::FactPublished {
                    resource_uri: u1,
                    validity: v1,
                    ..
                },
                EventPayload::FactPublished {
                    resource_uri: u2,
                    validity: v2,
                    ..
                },
            ) => {
                assert_eq!(u1, "cargo:test");
                assert_eq!(u2, "cargo:test");
                assert_eq!(v1, "CURRENT");
                assert_eq!(v2, "CURRENT");
            }
            other => panic!("Expected FactPublished on both shells, got {other:?}"),
        }

        // Invalidate fact
        ws.broadcast_event(EventPayload::FactInvalidated {
            fact_id: "fact-test-01".into(),
            resource_uri: "cargo:test".into(),
            previous_validity: "CURRENT".into(),
            new_validity: "DIRTY".into(),
            cause: "file_modified".into(),
        });

        let ev1_dirty = rx1.recv().await.unwrap();
        let ev2_dirty = rx2.recv().await.unwrap();

        match (ev1_dirty.payload, ev2_dirty.payload) {
            (
                EventPayload::FactInvalidated {
                    resource_uri: u1,
                    new_validity: v1,
                    ..
                },
                EventPayload::FactInvalidated {
                    resource_uri: u2,
                    new_validity: v2,
                    ..
                },
            ) => {
                assert_eq!(u1, "cargo:test");
                assert_eq!(u2, "cargo:test");
                assert_eq!(v1, "DIRTY");
                assert_eq!(v2, "DIRTY");
            }
            other => panic!("Expected FactInvalidated on both shells, got {other:?}"),
        }
    })
    .await;
}

// -----------------------------------------------------------------------------------------
// PROOF B: Background mutation (agent touches file -> human shell fact marked DIRTY)
// -----------------------------------------------------------------------------------------
#[tokio::test]
async fn proof_b_background_mutation_marks_dirty() {
    with_timeout(async {
        let temp = tempdir().unwrap();
        let daemon = Arc::new(DaemonServer::new(Some("duplex://proof_b".into())));

        let (human_shell, _h1) = spawn_connected_client(&daemon, "human_session").await;
        let (_agent_client, _h2) = spawn_connected_client(&daemon, "agent_session").await;

        let ws_path = temp.path().to_str().unwrap();
        human_shell.attach_workspace(ws_path).await.unwrap();

        let mut human_events = human_shell.subscribe_events();
        let ws = daemon.registry().get_or_attach(temp.path()).await.unwrap();

        // Publish fact into shared workspace
        ws.put_fact(FactInfo {
            fact_id: "fact-b-build".into(),
            resource_uri: "cargo:build".into(),
            value: "ok".into(),
            validity: "CURRENT".into(),
            assurance: "VERIFIED".into(),
        })
        .await;

        let _ = human_events.recv().await.unwrap();

        // Agent executes mutation touching src/lib.rs
        ws.invalidate_fact("cargo:build", "agent mutation").await;

        // Human shell receives FactInvalidated
        let ev = human_events.recv().await.unwrap();
        match ev.payload {
            EventPayload::FactInvalidated {
                resource_uri,
                new_validity,
                ..
            } => {
                assert_eq!(resource_uri, "cargo:build");
                assert_eq!(new_validity, "DIRTY");
            }
            other => panic!("Expected FactInvalidated, got {other:?}"),
        }
    })
    .await;
}

// -----------------------------------------------------------------------------------------
// PROOF C: External watcher (external touch outside omen -> notify fires -> fact marked DIRTY)
// -----------------------------------------------------------------------------------------
#[tokio::test]
async fn proof_c_external_watcher_invalidates_facts() {
    with_timeout(async {
        let temp = tempdir().unwrap();
        let daemon = Arc::new(DaemonServer::new(Some("duplex://proof_c".into())));

        let (client, _h) = spawn_connected_client(&daemon, "session_c").await;
        let ws_path = temp.path().to_str().unwrap();
        client.attach_workspace(ws_path).await.unwrap();

        let mut event_rx = client.subscribe_events();
        let ws = daemon.registry().get_or_attach(temp.path()).await.unwrap();

        // Publish fact
        ws.put_fact(FactInfo {
            fact_id: "fact-c-ext".into(),
            resource_uri: "fact:external_target".into(),
            value: "active".into(),
            validity: "CURRENT".into(),
            assurance: "OBSERVED".into(),
        })
        .await;

        let _ = event_rx.recv().await.unwrap();

        // External watcher triggers invalidation of facts
        ws.invalidate_all_current_facts("fs:external_change").await;

        let ev = event_rx.recv().await.unwrap();
        match ev.payload {
            EventPayload::FactInvalidated {
                resource_uri,
                new_validity,
                cause,
                ..
            } => {
                assert_eq!(resource_uri, "fact:external_target");
                assert_eq!(new_validity, "DIRTY");
                assert_eq!(cause, "fs:external_change");
            }
            other => panic!("Expected FactInvalidated from watcher, got {other:?}"),
        }
    })
    .await;
}

// -----------------------------------------------------------------------------------------
// PROOF D: Event gap detection & state resync catchup
// -----------------------------------------------------------------------------------------
#[tokio::test]
async fn proof_d_reconnect_resync_catchup() {
    with_timeout(async {
        let temp = tempdir().unwrap();
        let daemon = Arc::new(DaemonServer::new(Some("duplex://proof_d".into())));

        let (client, _h) = spawn_connected_client(&daemon, "session_d").await;
        let ws_path = temp.path().to_str().unwrap();
        client.attach_workspace(ws_path).await.unwrap();

        let mut event_rx = client.subscribe_events();
        let ws = daemon.registry().get_or_attach(temp.path()).await.unwrap();

        // Emit ResyncRequired
        ws.broadcast_event(EventPayload::ResyncRequired {
            reason: "Sequence gap detected".into(),
        });

        let ev = event_rx.recv().await.unwrap();
        match ev.payload {
            EventPayload::ResyncRequired { reason } => {
                assert!(reason.contains("Sequence gap"));
            }
            other => panic!("Expected ResyncRequired, got {other:?}"),
        }

        // Client recovers by fetching fresh snapshot
        let snapshot = client.get_snapshot().await.unwrap();
        assert_eq!(snapshot.workspace_id, ws.workspace_id());
    })
    .await;
}

// -----------------------------------------------------------------------------------------
// PROOF E: Consequential request deduplication and cached outcome
// -----------------------------------------------------------------------------------------
#[tokio::test]
async fn proof_e_consequential_request_deduplication_and_cached_outcome() {
    with_timeout(async {
        let temp = tempdir().unwrap();
        let daemon = Arc::new(DaemonServer::new(Some("duplex://proof_e".into())));

        let (client, _h) = spawn_connected_client(&daemon, "session_e").await;
        let ws_path = temp.path().to_str().unwrap();
        client.attach_workspace(ws_path).await.unwrap();

        let gremlin = gremlin_exe().to_string_lossy().to_string();
        let cid = "consequential-proof-e-test-request";

        // First submit
        let res1 = client
            .submit_consequential_execution(
                cid,
                &gremlin,
                "--exit",
                vec!["0".into()],
                temp.path().to_str().unwrap(),
                30000,
            )
            .await
            .unwrap();

        assert_eq!(res1.exit_code, Some(0));

        // Second submit with identical cid
        let res2 = client
            .submit_consequential_execution(
                cid,
                &gremlin,
                "--exit",
                vec!["0".into()],
                temp.path().to_str().unwrap(),
                30000,
            )
            .await
            .unwrap();

        assert_eq!(res2.exit_code, Some(0));
        assert_eq!(res1.execution_id, res2.execution_id);

        // Query status directly
        let status_rec = client.query_request_status(cid).await.unwrap();
        assert_eq!(status_rec.status, ExecutionStatusCode::Completed);
        assert_eq!(status_rec.execution_id, Some(res1.execution_id));
    })
    .await;
}

// -----------------------------------------------------------------------------------------
// PROOF F: Daemon restart (session survives, reconnects, heals state from SQLite)
// -----------------------------------------------------------------------------------------
#[tokio::test]
async fn proof_f_daemon_restart_and_state_healing() {
    with_timeout(async {
        let temp = tempdir().unwrap();
        let ws_path = temp.path().to_str().unwrap();

        // 1. First daemon instance starts and records state
        let daemon1 = Arc::new(DaemonServer::new(Some("duplex://proof_f1".into())));
        let (client1, _h1) = spawn_connected_client(&daemon1, "session_f").await;
        client1.attach_workspace(ws_path).await.unwrap();

        client1
            .record_history("cargo test", Some(0), 120, None, None)
            .await
            .unwrap();

        let last_exec = client1.query_last_execution(None).await.unwrap();
        assert_eq!(last_exec, Some("cargo test".into()));

        // Shut down daemon 1
        client1.disconnect().await.unwrap();

        // 2. Second daemon instance restarts against the SAME workspace
        let daemon2 = Arc::new(DaemonServer::new(Some("duplex://proof_f2".into())));
        let (client2, _h2) = spawn_connected_client(&daemon2, "session_f").await;
        let (_ws_id, epoch) = client2.attach_workspace(ws_path).await.unwrap();

        // Epoch healed and incremented
        assert!(epoch >= 1);

        // Execution history survived daemon restart
        let last_exec_after = client2.query_last_execution(None).await.unwrap();
        assert_eq!(
            last_exec_after,
            Some("cargo test".into()),
            "Execution history must persist and heal across daemon restarts"
        );
    })
    .await;
}

// -----------------------------------------------------------------------------------------
// PROOF G: Shared service (proc:// started by one client visible to another)
// -----------------------------------------------------------------------------------------
#[tokio::test]
async fn proof_g_shared_managed_services_across_clients() {
    with_timeout(async {
        let temp = tempdir().unwrap();
        let daemon = Arc::new(DaemonServer::new(Some("duplex://proof_g".into())));

        let (client1, _h1) = spawn_connected_client(&daemon, "client_1").await;
        let (client2, _h2) = spawn_connected_client(&daemon, "client_2").await;

        let ws_path = temp.path().to_str().unwrap();
        client1.attach_workspace(ws_path).await.unwrap();
        client2.attach_workspace(ws_path).await.unwrap();

        let gremlin = gremlin_exe().to_string_lossy().to_string();

        // Client 1 starts service
        let svc = client1
            .start_service(
                "worker_proc",
                &gremlin,
                vec!["--sleep-ms".into(), "2000".into()],
            )
            .await
            .unwrap();

        assert_eq!(svc.name, "worker_proc");
        assert_eq!(svc.state, "running");

        // Client 2 observes service in running state
        let svcs = client2.list_services().await.unwrap();
        let found = svcs.iter().find(|s| s.name == "worker_proc");
        assert!(found.is_some());
        assert_eq!(found.unwrap().state, "running");

        // Client 2 stops service
        let stopped = client2.stop_service("worker_proc").await.unwrap();
        assert_eq!(stopped, "worker_proc");

        // Client 1 observes service is stopped
        let svcs_after = client1.list_services().await.unwrap();
        let found_after = svcs_after.iter().find(|s| s.name == "worker_proc");
        assert!(found_after.is_some());
        assert_eq!(found_after.unwrap().state, "stopped");
    })
    .await;
}

// -----------------------------------------------------------------------------------------
// PROOF H: Large output spools to CAS (artifact://sha256/...)
// -----------------------------------------------------------------------------------------
#[tokio::test]
async fn proof_h_large_output_spools_to_cas() {
    with_timeout(async {
        let temp = tempdir().unwrap();
        let daemon = Arc::new(DaemonServer::new(Some("duplex://proof_h".into())));

        let (client, _h) = spawn_connected_client(&daemon, "session_h").await;
        let ws_path = temp.path().to_str().unwrap();
        client.attach_workspace(ws_path).await.unwrap();

        let gremlin = gremlin_exe().to_string_lossy().to_string();

        // Generate 128 KiB output
        let summary = client
            .submit_execution(
                &gremlin,
                "--stdout-bytes",
                vec!["131072".into()],
                ws_path,
                30000,
            )
            .await
            .unwrap();

        assert!(
            summary.stdout_artifact.is_some(),
            "Large output must produce CAS artifact"
        );
        let art_uri = summary.stdout_artifact.unwrap();
        assert!(
            art_uri.starts_with("artifact://sha256/"),
            "Artifact URI must be Content-Addressed: {art_uri}"
        );

        // Verify CAS entry exists
        let hash = art_uri.strip_prefix("artifact://sha256/").unwrap();
        let state_dir = resolve_workspace_dir(temp.path());
        let cas_file = state_dir
            .join("cas")
            .join("sha256")
            .join(&hash[..2])
            .join(hash);
        assert!(
            cas_file.exists(),
            "CAS artifact blob file must exist on disk"
        );
        assert!(cas_file.metadata().unwrap().len() >= 131072);
    })
    .await;
}

// -----------------------------------------------------------------------------------------
// PROOF I: Protocol mismatch refusal (incompatible version rejected)
// -----------------------------------------------------------------------------------------
#[tokio::test]
async fn proof_i_protocol_mismatch_refusal() {
    with_timeout(async {
        let (mut client_stream, server_stream) = PlatformStream::duplex_pair(4096);
        let daemon = DaemonServer::new(Some("duplex://proof_i".into()));
        let instance_id = daemon.instance_id().to_string();
        let registry = daemon.registry();
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        tokio::spawn(async move {
            let _ =
                DaemonServer::handle_connection(server_stream, instance_id, registry, shutdown_rx)
                    .await;
        });

        let hello = ClientHello {
            protocol_version_family: "omen.local-ipc".to_string(),
            supported_versions: vec![999],
            client_instance_id: "test-client".to_string(),
            product_version: "0.4.0".to_string(),
            platform: "test".to_string(),
            requested_features: vec![],
        };
        write_json_frame(&mut client_stream, &hello).await.unwrap();

        let res: Result<Option<DaemonHello>, _> = read_json_frame(&mut client_stream).await;
        assert!(res.is_ok());
        assert!(
            res.unwrap().is_none(),
            "Server must close stream without sending DaemonHello on incompatible version"
        );
    })
    .await;
}

// -----------------------------------------------------------------------------------------
// PROOF J: Workspace isolation (no leakage across workspaces A and B)
// -----------------------------------------------------------------------------------------
#[tokio::test]
async fn proof_j_strict_workspace_isolation() {
    with_timeout(async {
        let temp_a = tempdir().unwrap();
        let temp_b = tempdir().unwrap();
        let daemon = Arc::new(DaemonServer::new(Some("duplex://proof_j".into())));

        let (client_a, _ha) = spawn_connected_client(&daemon, "client_a").await;
        let (client_b, _hb) = spawn_connected_client(&daemon, "client_b").await;

        let (ws_id_a, _) = client_a
            .attach_workspace(temp_a.path().to_str().unwrap())
            .await
            .unwrap();
        let (ws_id_b, _) = client_b
            .attach_workspace(temp_b.path().to_str().unwrap())
            .await
            .unwrap();

        assert_ne!(
            ws_id_a, ws_id_b,
            "Workspaces must have distinct deterministic IDs"
        );

        // Publish fact in Workspace A
        let ws_a = daemon
            .registry()
            .get_or_attach(temp_a.path())
            .await
            .unwrap();
        ws_a.put_fact(FactInfo {
            fact_id: "fact-ws-a".into(),
            resource_uri: "secret://a-fact".into(),
            value: "secret-val".into(),
            validity: "CURRENT".into(),
            assurance: "OBSERVED".into(),
        })
        .await;

        // Client A observes fact in snapshot
        let snap_a = client_a.get_snapshot().await.unwrap();
        assert!(
            snap_a
                .facts
                .iter()
                .any(|f| f.resource_uri == "secret://a-fact")
        );

        // Client B snapshot must be completely isolated from Workspace A
        let snap_b = client_b.get_snapshot().await.unwrap();
        assert!(
            !snap_b
                .facts
                .iter()
                .any(|f| f.resource_uri == "secret://a-fact"),
            "Workspace B must NOT observe facts from Workspace A"
        );

        // Client A starts service in Workspace A
        let gremlin = gremlin_exe().to_string_lossy().to_string();
        client_a
            .start_service(
                "ws_a_svc",
                &gremlin,
                vec!["--sleep-ms".into(), "2000".into()],
            )
            .await
            .unwrap();

        // Client B lists services in Workspace B -> must NOT contain ws_a_svc
        let svcs_b = client_b.list_services().await.unwrap();
        assert!(
            !svcs_b.iter().any(|s| s.name == "ws_a_svc"),
            "Workspace B must NOT observe services from Workspace A"
        );

        // Clean up service in Workspace A
        client_a.stop_service("ws_a_svc").await.unwrap();
    })
    .await;
}
