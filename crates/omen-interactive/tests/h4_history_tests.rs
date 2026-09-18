use omen_core::{ExecutionId, InteractiveSessionId, ResourceUri};
use omen_interactive::InteractiveSession;
use omen_interactive::resolver::ReferenceResolver;
use omen_knowledge::{Database, ExecutionHistory, ExecutionRecord};
use tempfile::tempdir;

#[test]
fn test_execution_history_recording_and_scoping() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("history_test.db");
    let mut db = Database::open(&db_path).unwrap();

    let session_human = InteractiveSessionId::new("sess-human-1").unwrap();
    let session_agent = InteractiveSessionId::new("sess-agent-bg").unwrap();

    ExecutionHistory::register_session(&mut db, &session_human, "human", ".").unwrap();
    ExecutionHistory::register_session(&mut db, &session_agent, "agent-bg", ".").unwrap();

    // Human runs cargo test (success)
    let exec1 = ExecutionRecord {
        execution_id: ExecutionId::new("exec-1").unwrap(),
        session_id: session_human.clone(),
        command: "cargo test".into(),
        exit_code: Some(0),
        duration_ms: Some(120),
        stdout_artifact: Some("artifact://sha256:abcd".into()),
        stderr_artifact: None,
        envelope_json: None,
        created_at: "2026-09-18T10:00:00Z".into(),
    };
    ExecutionHistory::record_execution(&mut db, &exec1, &[], &[], &[]).unwrap();

    // Human runs cargo test auth (fails)
    let exec2 = ExecutionRecord {
        execution_id: ExecutionId::new("exec-2").unwrap(),
        session_id: session_human.clone(),
        command: "cargo test auth".into(),
        exit_code: Some(101),
        duration_ms: Some(250),
        stdout_artifact: Some("artifact://sha256:1111".into()),
        stderr_artifact: Some("artifact://sha256:2222".into()),
        envelope_json: None,
        created_at: "2026-09-18T10:01:00Z".into(),
    };
    ExecutionHistory::record_execution(&mut db, &exec2, &[], &[], &[]).unwrap();

    // Background agent runs git status later
    let exec_bg = ExecutionRecord {
        execution_id: ExecutionId::new("exec-bg").unwrap(),
        session_id: session_agent.clone(),
        command: "git status".into(),
        exit_code: Some(0),
        duration_ms: Some(40),
        stdout_artifact: None,
        stderr_artifact: None,
        envelope_json: None,
        created_at: "2026-09-18T10:02:00Z".into(),
    };
    ExecutionHistory::record_execution(&mut db, &exec_bg, &[], &[], &[]).unwrap();

    // Assert human session @last is isolated from agent session @last
    let human_last = ReferenceResolver::resolve("@last", &session_human, &db).unwrap();
    assert_eq!(human_last, "cargo test auth");

    let agent_last = ReferenceResolver::resolve("@last", &session_agent, &db).unwrap();
    assert_eq!(agent_last, "git status");

    // Human @failed resolves to the failing cargo test command
    let human_failed = ReferenceResolver::resolve("@failed", &session_human, &db).unwrap();
    assert_eq!(human_failed, "cargo test auth");

    // Agent has no failed execution
    let agent_failed = ReferenceResolver::resolve("@failed", &session_agent, &db);
    assert!(agent_failed.is_err());
}

#[test]
fn test_typed_reference_resolution() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("resolve_test.db");
    let mut db = Database::open(&db_path).unwrap();

    let session = InteractiveSessionId::new("sess-1").unwrap();
    ExecutionHistory::register_session(&mut db, &session, "human", ".").unwrap();

    let res_mod = (
        ResourceUri::parse("workspace://src/main.rs").unwrap(),
        "modified".into(),
    );
    let art_hash = "a".repeat(64);
    let art_uri_str = format!("artifact://sha256/{art_hash}");
    let art1 = ResourceUri::parse(&art_uri_str).unwrap();

    let exec = ExecutionRecord {
        execution_id: ExecutionId::new("exec-10").unwrap(),
        session_id: session.clone(),
        command: "cargo check".into(),
        exit_code: Some(0),
        duration_ms: Some(15),
        stdout_artifact: Some(art_uri_str.clone()),
        stderr_artifact: None,
        envelope_json: None,
        created_at: "2026-09-18T12:00:00Z".into(),
    };
    ExecutionHistory::record_execution(&mut db, &exec, &[res_mod], &[], &[art1]).unwrap();

    // Resolving @last.artifact
    let art_ref = ReferenceResolver::resolve("@last.artifact", &session, &db).unwrap();
    assert_eq!(art_ref, art_uri_str);

    // Resolving @last.changed
    let changed = ReferenceResolver::resolve("@last.changed", &session, &db).unwrap();
    assert_eq!(changed, "workspace://src/main.rs");

    // Resolving @fact.fs:workspace
    let fact_ref = ReferenceResolver::resolve("@fact.fs:workspace", &session, &db).unwrap();
    assert_eq!(fact_ref, "fact://fs:workspace");

    // Resolving argv replacement
    let argv = vec!["cat".into(), "@last.artifact".into()];
    let resolved = ReferenceResolver::resolve_argv(&argv, &session, Some(&db));
    assert_eq!(resolved, vec!["cat", &art_uri_str]);
}

#[test]
fn test_semantic_history_and_rerun_actions() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("actions_test.db");
    let db = Database::open(&db_path).unwrap();

    let mut session = InteractiveSession::new(dir.path().to_path_buf(), Some(db)).unwrap();

    // Record an execution
    let exec = ExecutionRecord {
        execution_id: ExecutionId::new("exec-test-1").unwrap(),
        session_id: session.session_id.clone(),
        command: "cargo build --release".into(),
        exit_code: Some(0),
        duration_ms: Some(400),
        stdout_artifact: None,
        stderr_artifact: None,
        envelope_json: None,
        created_at: "2026-09-18T13:00:00Z".into(),
    };
    ExecutionHistory::record_execution(session.db.as_mut().unwrap(), &exec, &[], &[], &[]).unwrap();

    // Dispatch :history
    let hist_exit = session.dispatch_input(":history").unwrap();
    assert!(hist_exit.is_zero());

    // Dispatch :rerun @last
    let rerun_exit = session.dispatch_input(":rerun @last").unwrap();
    assert!(rerun_exit.is_zero());
}
