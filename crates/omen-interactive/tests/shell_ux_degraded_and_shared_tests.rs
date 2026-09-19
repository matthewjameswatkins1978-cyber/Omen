use tempfile::tempdir;
use tokio::io::duplex;

use omen_client::OmenClient;
use omen_core::InteractiveSessionId;
use omen_daemon::DaemonServer;
use omen_interactive::InteractiveSession;
use omen_ipc::{EventPayload, PlatformStream};
use omen_knowledge::Database;
use reedline::{Completer, Prompt};

#[tokio::test]
async fn test_shell_ux_degraded_mode_when_daemon_offline() {
    let temp = tempdir().unwrap();
    let db_path = temp.path().join("state.sqlite");
    let db = Database::open(&db_path).unwrap();

    let session_id = InteractiveSessionId::generate();
    // Connect without a running daemon -> degraded standalone mode
    let session =
        InteractiveSession::new_with_client(session_id, temp.path().to_path_buf(), Some(db), None)
            .unwrap();

    assert!(
        session.client.is_none(),
        "Client must be None in degraded mode"
    );

    let prompt_rendered = session.prompt.render_prompt_left();
    assert!(
        prompt_rendered.contains("[standalone]"),
        "Prompt indicator must display [standalone] in degraded mode: {prompt_rendered}"
    );
    assert!(
        !prompt_rendered.to_lowercase().contains("error"),
        "No error banners should be shown in degraded mode: {prompt_rendered}"
    );

    // Keystroke latency invariant: completion on hot index is strictly in-memory and instant
    let comp_ctx = session.comp_ctx.clone();
    let completer = std::sync::Arc::new(std::sync::Mutex::new(
        omen_interactive::completion::OmenCompleter::new(comp_ctx),
    ));

    let start = std::time::Instant::now();
    let res = completer.lock().unwrap().complete(":", 1);
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_millis(10),
        "Keystroke completion must execute strictly in-memory under 10ms, took {:?}",
        elapsed
    );
    assert!(
        !res.suggestions().is_empty(),
        "Should provide semantic action suggestions"
    );
}

#[tokio::test]
async fn test_shell_ux_shared_mode_with_connected_daemon() {
    let temp = tempdir().unwrap();
    let db_path = temp.path().join("state.sqlite");
    let db = Database::open(&db_path).unwrap();

    let (client_half, daemon_half) = duplex(1024 * 1024);
    let daemon_stream = PlatformStream::Duplex(daemon_half);
    let client_stream = PlatformStream::Duplex(client_half);

    let daemon = DaemonServer::new(Some("duplex://test".into()));
    let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let instance_id = daemon.instance_id().to_string();
    let registry = daemon.registry();

    tokio::spawn(async move {
        let _ = DaemonServer::handle_connection(daemon_stream, instance_id, registry, shutdown_rx)
            .await;
    });

    let session_id = InteractiveSessionId::generate();
    let client = OmenClient::from_stream(
        client_stream,
        Some("duplex://test".into()),
        Some(session_id.to_string()),
    )
    .await
    .unwrap();

    let (_ws_id, _) = client
        .attach_workspace(temp.path().to_str().unwrap())
        .await
        .unwrap();

    let session = InteractiveSession::new_with_client(
        session_id,
        temp.path().to_path_buf(),
        Some(db),
        Some(client.clone()),
    )
    .unwrap();

    assert!(
        session.client.is_some(),
        "Client must be Some in shared mode"
    );

    let prompt_rendered = session.prompt.render_prompt_left();
    assert!(
        prompt_rendered.contains("[shared]"),
        "Prompt indicator must display [shared] in shared mode: {prompt_rendered}"
    );

    // Simulate daemon publishing a fact event to all subscribers
    let ws_state = daemon.registry().get_or_attach(temp.path()).await.unwrap();
    let _ = ws_state.broadcast_event(EventPayload::FactPublished {
        fact_id: "fact-test-01".into(),
        resource_uri: "file://shared_config.toml".into(),
        validity: "CURRENT".into(),
        assurance: "OBSERVED".into(),
    });

    // Give background subscriber task a short slice to apply event
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify completion context received the fact via shared event
    let ctx = session.comp_ctx.lock().unwrap();
    let found = ctx
        .hot_index
        .active_facts
        .iter()
        .any(|f| f.resource_uri == "file://shared_config.toml");
    assert!(
        found,
        "Hot index must receive published fact from daemon event pump"
    );
}
