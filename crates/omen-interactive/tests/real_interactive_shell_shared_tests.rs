use std::sync::Arc;
use tempfile::tempdir;

use omen_client::OmenClient;
use omen_core::InteractiveSessionId;
use omen_daemon::DaemonServer;
use omen_interactive::resolver::ReferenceResolver;
use omen_interactive::session::InteractiveSession;
use omen_ipc::{EventPayload, PlatformStream};
use omen_knowledge::{Database, canonical_workspace_db_path};
use omen_test_fixtures::{INTEGRATION_TIMEOUT, run_with_test_timeout};

async fn spawn_connected_session(
    daemon: &Arc<DaemonServer>,
    ws_path: &std::path::Path,
    session_id_str: &str,
) -> InteractiveSession {
    let (client_stream, daemon_stream) = PlatformStream::duplex_pair(65536);
    let instance_id = daemon.instance_id().to_string();
    let registry = daemon.registry();
    let shutdown_rx = daemon.subscribe_shutdown();

    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(daemon_stream, instance_id, registry, shutdown_rx)
            .await;
    });

    let client = OmenClient::from_stream(
        client_stream,
        Some("memory://interactive_shell_test".into()),
        Some(session_id_str.to_string()),
    )
    .await
    .unwrap();

    let path_str = ws_path.to_str().unwrap();
    client.attach_workspace(path_str).await.unwrap();

    let db_path = canonical_workspace_db_path(ws_path);
    let db = Database::open(&db_path).unwrap();

    let session_id = InteractiveSessionId::new(session_id_str).unwrap();
    InteractiveSession::new_with_client(session_id, ws_path.to_path_buf(), Some(db), Some(client))
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_real_interactive_shell_observes_remote_fact_dirty() {
    run_with_test_timeout(
        "test_real_interactive_shell_observes_remote_fact_dirty",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("START_DAEMON");
            let temp = tempdir().unwrap();
            let daemon = Arc::new(DaemonServer::new(Some("duplex://real_shell".into())));
            ctx.set_daemon_status("running");

            ctx.phase("ATTACH_CONNECTED_SESSION");
            let mut session =
                spawn_connected_session(&daemon, temp.path(), "sess_human_shell_1").await;
            ctx.set_client_status("connected");
            assert_eq!(session.prompt.dirty_count(), 0);

            ctx.phase("BROADCAST_FACT_INVALIDATED");
            let ws = daemon.registry().get_or_attach(temp.path()).await.unwrap();
            ws.broadcast_event(EventPayload::FactInvalidated {
                fact_id: "fact-remote-01".into(),
                resource_uri: "cargo:build".into(),
                previous_validity: "CURRENT".into(),
                new_validity: "DIRTY".into(),
                cause: "agent mutation".into(),
            });
            ctx.record_event("FactInvalidated(fact-remote-01)");

            ctx.phase("WAIT_FOR_HOT_INDEX_UPDATE");
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;

            ctx.phase("VERIFY_PROMPT_STATE");
            session.update_prompt_state();
            assert_eq!(
                session.prompt.dirty_count(),
                1,
                "Real interactive shell prompt MUST observe dirty fact count from shared index"
            );
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_real_interactive_shell_routes_execution_through_daemon_once() {
    run_with_test_timeout(
        "test_real_interactive_shell_routes_execution_through_daemon_once",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("START_DAEMON");
            let temp = tempdir().unwrap();
            let daemon = Arc::new(DaemonServer::new(Some("duplex://real_shell_exec".into())));
            ctx.set_daemon_status("running");

            ctx.phase("SPAWN_CONNECTED_SESSION");
            let mut session =
                spawn_connected_session(&daemon, temp.path(), "sess_human_exec").await;
            ctx.set_client_status("connected");

            ctx.phase("DISPATCH_EXECUTION_INPUT");
            let exit = session.dispatch_input("cargo --version").unwrap();
            assert_eq!(exit.code, Some(0));

            ctx.phase("VERIFY_DATABASE_HISTORY");
            let db_path = canonical_workspace_db_path(temp.path());
            let db = Database::open(&db_path).unwrap();
            let sid = InteractiveSessionId::new("sess_human_exec").unwrap();

            let last = omen_knowledge::ExecutionHistory::get_last_execution(&db, &sid)
                .unwrap()
                .expect("Execution history must exist");
            assert_eq!(last.command, "cargo --version");
            assert_eq!(last.exit_code, Some(0));
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_real_interactive_shell_last_remains_session_scoped() {
    run_with_test_timeout(
        "test_real_interactive_shell_last_remains_session_scoped",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("START_DAEMON");
            let temp = tempdir().unwrap();
            let daemon = Arc::new(DaemonServer::new(Some("duplex://real_shell_last".into())));
            ctx.set_daemon_status("running");

            ctx.phase("SPAWN_SESSION_1");
            let mut session1 =
                spawn_connected_session(&daemon, temp.path(), "sess_shell_one").await;

            ctx.phase("SPAWN_SESSION_2");
            let mut session2 =
                spawn_connected_session(&daemon, temp.path(), "sess_shell_two").await;
            ctx.set_client_status("two clients connected");

            ctx.phase("DISPATCH_SESSION_1_INPUT");
            session1.dispatch_input("cargo --version").unwrap();

            ctx.phase("DISPATCH_SESSION_2_INPUT");
            session2.dispatch_input("cargo --help").unwrap();

            ctx.phase("VERIFY_SESSION_SCOPED_LAST");
            let db_path = canonical_workspace_db_path(temp.path());
            let db = Database::open(&db_path).unwrap();

            let sid1 = InteractiveSessionId::new("sess_shell_one").unwrap();
            let sid2 = InteractiveSessionId::new("sess_shell_two").unwrap();

            let last1 = ReferenceResolver::resolve("@last", &sid1, &db).unwrap();
            let last2 = ReferenceResolver::resolve("@last", &sid2, &db).unwrap();

            assert_eq!(last1, "cargo --version");
            assert_eq!(last2, "cargo --help");
            assert_ne!(
                last1, last2,
                "@last MUST remain strictly session-scoped even within the shared runtime"
            );
        },
    )
    .await;
}
