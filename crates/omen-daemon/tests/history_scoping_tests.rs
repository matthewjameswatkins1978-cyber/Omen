use omen_client::OmenClient;
use omen_core::InteractiveSessionId;
use omen_daemon::DaemonServer;
use omen_interactive::resolver::ReferenceResolver;
use omen_ipc::{EventPayload, PlatformStream};
use omen_knowledge::Database;
use tempfile::tempdir;
use tokio::sync::watch;

#[tokio::test]
async fn test_shared_subordinate_history_and_session_scoped_last() {
    let server = DaemonServer::new(Some("history-test-endpoint".to_string()));
    let registry = server.registry();
    let instance_id = server.instance_id().to_string();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let tmp = tempdir().unwrap();
    let path = tmp.path().to_str().unwrap();

    // 1. Connect Human Client (session_id: sess_human)
    let (c1_stream, s1_stream) = PlatformStream::duplex_pair(4096);
    let reg1 = registry.clone();
    let inst1 = instance_id.clone();
    let rx1 = shutdown_rx.clone();
    tokio::spawn(async move {
        DaemonServer::handle_connection(s1_stream, inst1, reg1, rx1)
            .await
            .unwrap();
    });

    let client_human = OmenClient::from_stream(c1_stream, None, Some("sess_human".to_string()))
        .await
        .expect("Human client should connect");
    let mut human_events = client_human.subscribe_events();
    let (ws_id_human, _) = client_human.attach_workspace(path).await.unwrap();

    // 2. Connect Agent Client (session_id: sess_agent)
    let (c2_stream, s2_stream) = PlatformStream::duplex_pair(4096);
    let reg2 = registry.clone();
    let inst2 = instance_id.clone();
    let rx2 = shutdown_rx.clone();
    tokio::spawn(async move {
        DaemonServer::handle_connection(s2_stream, inst2, reg2, rx2)
            .await
            .unwrap();
    });

    let client_agent = OmenClient::from_stream(c2_stream, None, Some("sess_agent".to_string()))
        .await
        .expect("Agent client should connect");
    let mut agent_events = client_agent.subscribe_events();
    let (ws_id_agent, _) = client_agent.attach_workspace(path).await.unwrap();

    assert_eq!(ws_id_human, ws_id_agent);

    // Initial state: no executions recorded yet
    assert_eq!(client_human.query_last_execution(None).await.unwrap(), None);
    assert_eq!(client_agent.query_last_execution(None).await.unwrap(), None);

    // 3. Human executes `cargo test`
    client_human
        .record_history(
            "cargo test",
            Some(0),
            320,
            Some("artifact://stdout/cargo-test".to_string()),
            None,
        )
        .await
        .expect("Human record history should succeed");

    // Both clients receive ExecutionRecorded event
    let ev1 = human_events.recv().await.unwrap();
    if let EventPayload::ExecutionRecorded {
        session_id,
        command,
        exit_code,
        ..
    } = ev1.payload
    {
        assert_eq!(session_id, "sess_human");
        assert_eq!(command, "cargo test");
        assert_eq!(exit_code, Some(0));
    } else {
        panic!("Expected ExecutionRecorded event for human, got {ev1:?}");
    }

    let ev2 = agent_events.recv().await.unwrap();
    if let EventPayload::ExecutionRecorded {
        session_id,
        command,
        exit_code,
        ..
    } = ev2.payload
    {
        assert_eq!(session_id, "sess_human");
        assert_eq!(command, "cargo test");
        assert_eq!(exit_code, Some(0));
    } else {
        panic!("Expected ExecutionRecorded event broadcast to agent, got {ev2:?}");
    }

    // Human `@last` is now `cargo test`, Agent `@last` remains None
    assert_eq!(
        client_human.query_last_execution(None).await.unwrap(),
        Some("cargo test".to_string())
    );
    assert_eq!(client_agent.query_last_execution(None).await.unwrap(), None);

    // 4. Background agent executes `npm run build`
    client_agent
        .record_history(
            "npm run build",
            Some(0),
            1200,
            Some("artifact://stdout/npm-build".to_string()),
            None,
        )
        .await
        .expect("Agent record history should succeed");

    // Both clients receive Agent's ExecutionRecorded event
    let ev3 = human_events.recv().await.unwrap();
    if let EventPayload::ExecutionRecorded {
        session_id,
        command,
        exit_code,
        ..
    } = ev3.payload
    {
        assert_eq!(session_id, "sess_agent");
        assert_eq!(command, "npm run build");
        assert_eq!(exit_code, Some(0));
    } else {
        panic!("Expected ExecutionRecorded event for agent, got {ev3:?}");
    }

    let ev4 = agent_events.recv().await.unwrap();
    if let EventPayload::ExecutionRecorded {
        session_id,
        command,
        exit_code,
        ..
    } = ev4.payload
    {
        assert_eq!(session_id, "sess_agent");
        assert_eq!(command, "npm run build");
        assert_eq!(exit_code, Some(0));
    } else {
        panic!("Expected ExecutionRecorded event broadcast to agent, got {ev4:?}");
    }

    // 5. Verify Subordinate Isolation over IPC:
    // Human `@last` remains strictly `cargo test`
    assert_eq!(
        client_human.query_last_execution(None).await.unwrap(),
        Some("cargo test".to_string()),
        "Human @last must NOT be polluted by background agent execution"
    );
    // Agent `@last` reflects agent's own execution `npm run build`
    assert_eq!(
        client_agent.query_last_execution(None).await.unwrap(),
        Some("npm run build".to_string()),
        "Agent @last must strictly reflect agent's own execution"
    );

    // 6. Verify Subordinate Isolation in Database via ReferenceResolver
    let db_path = omen_knowledge::canonical_workspace_db_path(tmp.path());
    let db = Database::open(&db_path).expect("Should open workspace database");

    let human_sid = InteractiveSessionId::new("sess_human").unwrap();
    let agent_sid = InteractiveSessionId::new("sess_agent").unwrap();

    let resolved_human_last =
        ReferenceResolver::resolve("@last", &human_sid, &db).expect("Human @last resolution");
    assert_eq!(resolved_human_last, "cargo test");

    let resolved_human_art = ReferenceResolver::resolve("@last.artifact", &human_sid, &db)
        .expect("Human @last.artifact resolution");
    assert_eq!(resolved_human_art, "artifact://stdout/cargo-test");

    let resolved_agent_last =
        ReferenceResolver::resolve("@last", &agent_sid, &db).expect("Agent @last resolution");
    assert_eq!(resolved_agent_last, "npm run build");

    let resolved_agent_art = ReferenceResolver::resolve("@last.artifact", &agent_sid, &db)
        .expect("Agent @last.artifact resolution");
    assert_eq!(resolved_agent_art, "artifact://stdout/npm-build");

    // 7. Human executes a subsequent command `cargo clippy`
    client_human
        .record_history(
            "cargo clippy",
            Some(0),
            400,
            Some("artifact://stdout/cargo-clippy".to_string()),
            None,
        )
        .await
        .unwrap();

    let resolved_human_last_updated =
        ReferenceResolver::resolve("@last", &human_sid, &db).expect("Updated human @last");
    assert_eq!(resolved_human_last_updated, "cargo clippy");

    // Agent `@last` is still preserved as `npm run build`
    let resolved_agent_last_preserved =
        ReferenceResolver::resolve("@last", &agent_sid, &db).expect("Preserved agent @last");
    assert_eq!(resolved_agent_last_preserved, "npm run build");
}
