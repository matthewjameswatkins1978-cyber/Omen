use std::sync::Arc;
use tempfile::tempdir;

use omen_daemon::workspace::WorkspaceState;
use omen_ipc::LocalIpcError;
use omen_knowledge::{
    Database, ServiceRecord, WorkspacePersistence, canonical_workspace_db_path,
    deterministic_workspace_id,
};
use omen_test_fixtures::{INTEGRATION_TIMEOUT, run_with_test_timeout};

#[tokio::test(flavor = "multi_thread")]
async fn test_service_restart_reconciliation_distinguishes_observed() {
    run_with_test_timeout(
        "test_service_restart_reconciliation_distinguishes_observed",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();
            let ws_id = deterministic_workspace_id(&ws_path);

            ctx.phase("SEED_SURVIVING_SERVICE_IN_SQLITE");
            // 1. Seed a service in SQLite with state = "running" and an alive PID (our own process)
            let my_pid = std::process::id();
            let db_path = canonical_workspace_db_path(&ws_path);
            let db = Database::open(&db_path).unwrap();
            WorkspacePersistence::upsert_service(
                &db,
                &ServiceRecord {
                    workspace_id: ws_id.clone(),
                    name: "surviving-service".into(),
                    command: "echo test".into(),
                    pid: Some(my_pid),
                    state: "running".into(),
                    started_at: chrono::Utc::now().to_rfc3339(),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                },
            )
            .unwrap();
            drop(db);

            ctx.phase("RESTART_DAEMON_WORKSPACE");
            // 2. Start new daemon WorkspaceState (simulating daemon restart while OS process survives)
            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 1).unwrap());
            ctx.set_daemon_status("restarted");

            ctx.phase("VERIFY_OBSERVED_STATE");
            // Verify daemon marks it "observed", NOT "running"
            let snapshot = ws.get_snapshot().await;
            let svc = snapshot
                .services
                .iter()
                .find(|s| s.name == "surviving-service")
                .expect("Service must be present");

            assert_eq!(
                svc.state, "observed",
                "Alive PID without child handle must be observed, never claimed as running owned"
            );
            assert_eq!(svc.pid, Some(my_pid));
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_service_restart_reconciliation_never_claims_false_stop() {
    run_with_test_timeout(
        "test_service_restart_reconciliation_never_claims_false_stop",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path().to_path_buf();
            let ws_id = deterministic_workspace_id(&ws_path);

            ctx.phase("SEED_SURVIVING_SERVICE_IN_SQLITE");
            // 1. Seed service in SQLite with alive PID
            let my_pid = std::process::id();
            let db_path = canonical_workspace_db_path(&ws_path);
            let db = Database::open(&db_path).unwrap();
            WorkspacePersistence::upsert_service(
                &db,
                &ServiceRecord {
                    workspace_id: ws_id.clone(),
                    name: "unowned-daemon-proc".into(),
                    command: "test".into(),
                    pid: Some(my_pid),
                    state: "running".into(),
                    started_at: chrono::Utc::now().to_rfc3339(),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                },
            )
            .unwrap();
            drop(db);

            ctx.phase("RESTART_DAEMON_WORKSPACE");
            // 2. Start daemon WorkspaceState
            let ws = Arc::new(WorkspaceState::new(ws_path.clone(), 1).unwrap());
            ctx.set_daemon_status("restarted");

            ctx.phase("ATTEMPT_STOP_OBSERVED");
            // 3. Attempt to stop the observed service
            let res = ws.stop_managed_service("unowned-daemon-proc").await;
            match res {
                Err(LocalIpcError::LocalPeerDenied(msg)) => {
                    assert!(
                        msg.contains("lacks process ownership"),
                        "Expected ownership refusal error, got: {msg}"
                    );
                }
                other => panic!(
                    "Expected LocalPeerDenied refusal to stop unowned alive process, got: {other:?}"
                ),
            }

            ctx.phase("VERIFY_DATABASE_STATE");
            // 4. Verify SQLite state is NOT set to stopped while OS process lives
            let db = Database::open(&db_path).unwrap();
            let record = WorkspacePersistence::get_service(&db, &ws_id, "unowned-daemon-proc")
                .unwrap()
                .unwrap();
            assert_ne!(
                record.state, "stopped",
                "Omen must NEVER falsely claim stopped in DB while OS process survives"
            );
            assert_eq!(record.state, "observed");
        },
    )
    .await;
}
