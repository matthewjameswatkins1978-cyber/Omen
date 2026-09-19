use std::fs;
use tempfile::tempdir;

use omen_core::{Assurance, InteractiveSessionId, ResourceUri};
use omen_daemon::workspace::WorkspaceState;
use omen_ipc::LocalIpcError;
use omen_knowledge::{
    CANONICAL_DB_FILE_NAME, Database, ExecutionHistory, ExecutionRecord, FactRegistry,
    PublishFactRequest, canonical_workspace_db_path, resolve_workspace_dir,
};
use omen_test_fixtures::{INTEGRATION_TIMEOUT, UNIT_TIMEOUT, run_with_test_timeout};

#[tokio::test(flavor = "multi_thread")]
async fn test_real_03_database_migrates_into_shared_runtime() {
    run_with_test_timeout(
        "test_real_03_database_migrates_into_shared_runtime",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path();

            // 1. Simulate an existing 0.3 workspace by creating the canonical database directly
            let db_path = canonical_workspace_db_path(ws_path);
            assert!(db_path.ends_with(CANONICAL_DB_FILE_NAME));

            ctx.phase("SEED_03_DATA");
            let mut db = Database::open(&db_path).expect("Should open canonical database");

            let res_uri = ResourceUri::parse("workspace://cargo-toml").unwrap();
            FactRegistry::publish_fact(
                &mut db,
                PublishFactRequest {
                    resource: &res_uri,
                    value: "omen 0.3.0",
                    assurance: Assurance::Verified,
                    producer: "cargo",
                    witness: None,
                    dependencies: &[],
                    artifacts: &[],
                },
            )
            .expect("Publish fact in 0.3 database");

            let sess_id = InteractiveSessionId::new("sess_03_legacy").unwrap();
            let exec_rec = ExecutionRecord {
                execution_id: omen_core::ExecutionId::new("exec-03-init").unwrap(),
                session_id: sess_id.clone(),
                command: "cargo check".into(),
                exit_code: Some(0),
                duration_ms: Some(150),
                stdout_artifact: None,
                stderr_artifact: None,
                envelope_json: None,
                created_at: chrono::Utc::now().to_rfc3339(),
            };
            ExecutionHistory::record_execution(&mut db, &exec_rec, &[], &[], &[])
                .expect("Record history in 0.3 database");

            drop(db);

            ctx.phase("ATTACH_WORKSPACE_DAEMON");
            // 2. Open the workspace via omen-daemon WorkspaceState
            let ws_state = WorkspaceState::new(ws_path.to_path_buf(), 1)
                .expect("Daemon attaches to 0.3 workspace");

            ctx.phase("VERIFY_FACT_SNAPSHOT");
            // Verify the daemon state read the existing 0.3 facts
            let snapshot = ws_state.get_snapshot().await;
            let fact = snapshot
                .facts
                .iter()
                .find(|f| f.resource_uri == "workspace://cargo-toml")
                .expect("0.3 fact must be visible in daemon snapshot");
            assert_eq!(fact.value, "omen 0.3.0");
            assert_eq!(fact.validity, "CURRENT");

            ctx.phase("VERIFY_HISTORY");
            // Verify daemon reads the existing execution history
            let last = ws_state
                .get_last_execution("sess_03_legacy")
                .await
                .unwrap()
                .expect("0.3 history must be queryable");
            assert_eq!(last.command, "cargo check");
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_daemon_and_cli_use_same_database() {
    run_with_test_timeout(
        "test_daemon_and_cli_use_same_database",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path();

            let state_dir = resolve_workspace_dir(ws_path);
            let canonical_path = canonical_workspace_db_path(ws_path);

            assert_eq!(state_dir.join(CANONICAL_DB_FILE_NAME), canonical_path);

            ctx.phase("INIT_DAEMON_WORKSPACE");
            let ws_state =
                WorkspaceState::new(ws_path.to_path_buf(), 1).expect("Create workspace state");

            ctx.phase("RECORD_HISTORY_FROM_DAEMON");
            ws_state
                .record_history("sess_daemon", "cargo build", Some(0), 200, None, None)
                .await
                .expect("Record history from daemon");

            ctx.phase("VERIFY_CLI_DATABASE_READ");
            // CLI opens canonical DB directly
            let db = Database::open(&canonical_path).expect("Open database from CLI");
            let sess = InteractiveSessionId::new("sess_daemon").unwrap();
            let rec = ExecutionHistory::get_last_execution(&db, &sess)
                .expect("Query from CLI")
                .expect("Record must exist in CLI database");

            assert_eq!(rec.command, "cargo build");
            assert_eq!(rec.exit_code, Some(0));

            // Ensure no spurious duplicate databases like "knowledge.db" were created
            assert!(!state_dir.join("knowledge.db").exists());
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_persistent_db_open_failure_does_not_fall_back_silently() {
    run_with_test_timeout(
        "test_persistent_db_open_failure_does_not_fall_back_silently",
        UNIT_TIMEOUT,
        |ctx| async move {
            ctx.phase("CREATE_CONFLICTING_DIR");
            let tmp = tempdir().unwrap();
            let ws_path = tmp.path();

            // Create a directory where the state.sqlite file should be, causing SQLite open to fail
            let db_path = canonical_workspace_db_path(ws_path);
            fs::create_dir_all(&db_path)
                .expect("Create directory at db file path to force failure");

            ctx.phase("VERIFY_FAIL_CLOSED");
            // WorkspaceState::new MUST fail closed with DaemonDegraded, not silently fall back to in-memory
            let res = WorkspaceState::new(ws_path.to_path_buf(), 1);
            match res {
                Err(LocalIpcError::DaemonDegraded(msg)) => {
                    assert!(
                        msg.contains("Failed to open canonical database"),
                        "Expected canonical database error message, got: {msg}"
                    );
                }
                Ok(_) => {
                    panic!(
                        "WorkspaceState::new must NOT fall back to in-memory database on failure"
                    )
                }
                Err(other) => panic!("Expected DaemonDegraded error, got: {other:?}"),
            }
        },
    )
    .await;
}
