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

fn gremlin_exe() -> std::path::PathBuf {
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
    assert!(
        exe.exists(),
        "omen-gremlin fixture must be built: {}",
        exe.display()
    );
    exe
}

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
async fn test_background_shell_refuses_standalone_and_boolean_chain_jobs() {
    let temp = tempdir().unwrap();
    let db_path = canonical_workspace_db_path(temp.path());
    let db = Database::open(&db_path).unwrap();
    let session_id = InteractiveSessionId::new("sess_shell_background_standalone").unwrap();
    let mut session =
        InteractiveSession::new_with_client(session_id, temp.path().to_path_buf(), Some(db), None)
            .unwrap();

    for input in ["echo first && echo second &", "echo standalone &"] {
        let error = session
            .dispatch_input(input)
            .expect_err("unsupported background requests must fail closed");
        assert!(
            matches!(
                error,
                omen_core::CoreError::ExecutionFailedCode {
                    code: omen_core::ErrorCode::Unsupported,
                    ..
                }
            ),
            "expected explicit unsupported result for {input:?}, got {error:?}"
        );
        assert!(
            session.tracked_background_jobs().is_empty(),
            "a refused background request must not be recorded as a job"
        );
    }
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
async fn test_real_interactive_shell_background_broker_jobs_and_stop() {
    run_with_test_timeout(
        "test_real_interactive_shell_background_broker_jobs_and_stop",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("START_DAEMON_AND_SESSION");
            let temp = tempdir().unwrap();
            let daemon = Arc::new(DaemonServer::new(Some(
                "duplex://real_shell_background".into(),
            )));
            let mut session =
                spawn_connected_session(&daemon, temp.path(), "sess_shell_background").await;

            ctx.phase("SUBMIT_BACKGROUND_SHELL_COMMAND");
            let gremlin = gremlin_exe().to_string_lossy().replace('\\', "/");
            let command = format!("\"{gremlin}\" --sleep-ms 30000 &");
            let accepted = session.dispatch_input(&command).unwrap();
            assert_eq!(accepted.code, Some(0));

            ctx.phase("VERIFY_BROKER_STATUS_AND_LIST_JOBS");
            let job = session
                .tracked_background_jobs()
                .first()
                .expect("interactive session must track accepted background work");
            let request_id = job.request_id.clone();
            let execution_id = job.execution_id.clone();
            assert!(execution_id.starts_with("exec_"));
            let client = session.client.as_ref().unwrap().clone();
            let running = client.query_request_status(&request_id).await.unwrap();
            assert_eq!(running.status, omen_ipc::ExecutionStatusCode::Running);
            assert_eq!(running.execution_id.as_deref(), Some(execution_id.as_str()));
            assert_eq!(session.dispatch_input(":jobs").unwrap().code, Some(0));

            ctx.phase("STOP_VIA_CANONICAL_EXECUTION_CANCELLATION");
            let stopped = session
                .dispatch_input(&format!(":stop {execution_id}"))
                .unwrap();
            assert_eq!(stopped.code, Some(0));
            let terminal = client.query_request_status(&request_id).await.unwrap();
            assert_eq!(terminal.status, omen_ipc::ExecutionStatusCode::Cancelled);
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
