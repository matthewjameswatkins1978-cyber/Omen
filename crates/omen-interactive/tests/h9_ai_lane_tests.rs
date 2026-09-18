use omen_core::{ExecutionId, InteractiveSessionId};
use omen_interactive::InteractiveSession;
use omen_interactive::ai_lane::AiLaneDispatcher;
use omen_knowledge::{Database, ExecutionHistory, ExecutionRecord};
use tempfile::tempdir;

#[test]
fn test_ai_lane_unconfigured_without_failures() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("ai_clean.db");
    let mut db = Database::open(&db_path).unwrap();

    let session = InteractiveSessionId::new("sess-ai-1").unwrap();
    ExecutionHistory::register_session(&mut db, &session, "human", ".").unwrap();

    let output = AiLaneDispatcher::dispatch("how do I run tests?", &session, Some(&db)).unwrap();
    assert!(!output.configured);
    assert!(output.query.contains("how do I run tests?"));
    assert!(
        output
            .response_text
            .contains("AI reasoning lane is not configured")
    );
    assert!(
        output
            .suggested_commands
            .iter()
            .any(|c| c == ":inspect @last")
    );
    assert!(output.suggested_commands.iter().any(|c| c == ":history"));
}

#[test]
fn test_ai_lane_unconfigured_with_recent_failure() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("ai_fail.db");
    let mut db = Database::open(&db_path).unwrap();

    let session = InteractiveSessionId::new("sess-ai-2").unwrap();
    ExecutionHistory::register_session(&mut db, &session, "human", ".").unwrap();

    // Record a failed execution
    let exec = ExecutionRecord {
        execution_id: ExecutionId::new("exec-fail").unwrap(),
        session_id: session.clone(),
        command: "cargo test auth".into(),
        exit_code: Some(101),
        duration_ms: Some(210),
        stdout_artifact: None,
        stderr_artifact: None,
        envelope_json: None,
        created_at: "2026-09-18T16:00:00Z".into(),
    };
    ExecutionHistory::record_execution(&mut db, &exec, &[], &[], &[]).unwrap();

    let output = AiLaneDispatcher::dispatch("why did my test fail?", &session, Some(&db)).unwrap();
    assert!(!output.configured);
    assert!(
        output
            .suggested_commands
            .iter()
            .any(|c| c == ":show @failed")
    );
    assert!(output.suggested_commands.iter().any(|c| c == ":why @last"));
    assert!(
        output
            .suggested_commands
            .iter()
            .any(|c| c == ":rerun @failed")
    );
}

#[test]
fn test_session_ai_lane_dispatch() {
    let dir = tempdir().unwrap();
    let mut session = InteractiveSession::new(dir.path().to_path_buf(), None).unwrap();

    let exit = session
        .dispatch_input("? what tools are available?")
        .unwrap();
    assert!(exit.is_zero());
}
