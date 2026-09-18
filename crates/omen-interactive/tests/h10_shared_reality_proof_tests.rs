use omen_core::{Assurance, ExecutionId, InteractiveSessionId, ResourceUri, ValidityState};
use omen_interactive::InteractiveSession;
use omen_interactive::resolver::ReferenceResolver;
use omen_knowledge::{
    Database, ExecutionHistory, ExecutionRecord, FactRegistry, PublishFactRequest,
};
use omen_ui::{ColorRoles, DiagnosticLevel, DiagnosticRenderer};
use reedline::Prompt;
use tempfile::tempdir;

#[test]
fn test_human_agent_shared_reality_and_session_isolation() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("shared_reality.db");
    let mut db = Database::open(&db_path).unwrap();

    let human_sid = InteractiveSessionId::new("sess-human-proof").unwrap();
    let agent_sid = InteractiveSessionId::new("sess-agent-worker").unwrap();

    // 1. Register human and agent sessions
    ExecutionHistory::register_session(&mut db, &human_sid, "human", ".").unwrap();
    ExecutionHistory::register_session(&mut db, &agent_sid, "agent-worker", ".").unwrap();

    // 2. Human initializes interactive session
    let mut human_session = InteractiveSession::new_with_session_id(
        human_sid.clone(),
        dir.path().to_path_buf(),
        Some(db),
    )
    .unwrap();

    // 3. Human executes `cargo test` and records clean passing fact
    let exec_human = ExecutionRecord {
        execution_id: ExecutionId::new("exec-human-01").unwrap(),
        session_id: human_sid.clone(),
        command: "cargo test".into(),
        exit_code: Some(0),
        duration_ms: Some(180),
        stdout_artifact: None,
        stderr_artifact: None,
        envelope_json: None,
        created_at: "2026-09-18T14:00:00Z".into(),
    };
    ExecutionHistory::record_execution(
        human_session.db.as_mut().unwrap(),
        &exec_human,
        &[],
        &[],
        &[],
    )
    .unwrap();

    FactRegistry::set_generation(human_session.db.as_mut().unwrap(), "fs:workspace", 1).unwrap();

    let fact_uri = ResourceUri::parse("fact://test:suite").unwrap();
    let initial_fact = PublishFactRequest {
        resource: &fact_uri,
        value: "all 42 tests passed",
        assurance: Assurance::Verified,
        producer: "cargo-test",
        witness: Some("git-rev-1"),
        dependencies: &[("fs:workspace".into(), 1)],
        artifacts: &[],
    };
    FactRegistry::publish_fact(human_session.db.as_mut().unwrap(), initial_fact).unwrap();

    // Verify initial clean state in prompt and @last
    human_session.update_prompt_state();
    let p_clean = human_session.prompt.render_prompt_left();
    assert!(
        p_clean.contains('✓') || p_clean.contains("ok"),
        "Prompt should be clean before agent changes: {p_clean}"
    );
    assert!(!p_clean.contains("dirty"));

    let resolved_last =
        ReferenceResolver::resolve("@last", &human_sid, human_session.db.as_ref().unwrap())
            .unwrap();
    assert_eq!(resolved_last, "cargo test");

    // 4. Background agent performs a mutation in session `sess-agent-worker`
    // Agent records its execution
    let exec_agent = ExecutionRecord {
        execution_id: ExecutionId::new("exec-agent-01").unwrap(),
        session_id: agent_sid.clone(),
        command: "threadmoth mutate --request req.json".into(),
        exit_code: Some(0),
        duration_ms: Some(95),
        stdout_artifact: None,
        stderr_artifact: None,
        envelope_json: None,
        created_at: "2026-09-18T14:02:00Z".into(),
    };
    ExecutionHistory::record_execution(
        human_session.db.as_mut().unwrap(),
        &exec_agent,
        &[],
        &[],
        &[],
    )
    .unwrap();

    // Agent workspace mutation increments generation of `fs:workspace` -> 2
    let new_gen =
        FactRegistry::increment_generation(human_session.db.as_mut().unwrap(), "fs:workspace")
            .unwrap();
    assert_eq!(new_gen, 2);

    // 5. Verify Subordinate Session Isolation:
    // Human `@last` is STRICTLY preserved as `cargo test`
    let human_last_after =
        ReferenceResolver::resolve("@last", &human_sid, human_session.db.as_ref().unwrap())
            .unwrap();
    assert_eq!(
        human_last_after, "cargo test",
        "Human session history must NOT be polluted by agent execution"
    );

    // Agent `@last` reflects agent's own execution
    let agent_last =
        ReferenceResolver::resolve("@last", &agent_sid, human_session.db.as_ref().unwrap())
            .unwrap();
    assert_eq!(
        agent_last, "threadmoth mutate --request req.json",
        "Agent session history must be scoped to agent session"
    );

    // 6. Verify Human Shared-Reality Awareness:
    // The human prompt automatically discovers the DIRTY fact on update
    human_session.update_prompt_state();
    let p_dirty = human_session.prompt.render_prompt_left();
    assert!(
        p_dirty.contains("1 dirty"),
        "Human prompt must indicate 1 dirty fact: {p_dirty}"
    );

    // 7. Human inspects provenance via :why
    let why = FactRegistry::why_fact(human_session.db.as_ref().unwrap(), &fact_uri).unwrap();
    assert_eq!(why.fact.validity, ValidityState::Dirty);
    assert!(why.dependencies.iter().any(|d| d.is_dirty));

    let roles = ColorRoles::plain();
    let diag = DiagnosticRenderer::render_fact_provenance(&why, DiagnosticLevel::Level2, &roles);
    assert!(diag.contains("DIRTY"));
    assert!(diag.contains("fs:workspace"));

    // Also verify human dispatching :why action succeeds
    let why_exit = human_session
        .dispatch_input(":why @fact://test:suite")
        .unwrap();
    assert!(why_exit.is_zero());

    // 8. Human re-runs `cargo test` and republishes refreshed fact
    let revalidated_fact = PublishFactRequest {
        resource: &fact_uri,
        value: "all 42 tests passed",
        assurance: Assurance::Verified,
        producer: "cargo-test",
        witness: Some("git-rev-2"),
        dependencies: &[("fs:workspace".into(), 2)],
        artifacts: &[],
    };
    FactRegistry::publish_fact(human_session.db.as_mut().unwrap(), revalidated_fact).unwrap();

    // Human prompt transitions back to clean
    human_session.update_prompt_state();
    let p_reclean = human_session.prompt.render_prompt_left();
    assert!(!p_reclean.contains("dirty"));
    assert!(p_reclean.contains('✓') || p_reclean.contains("ok"));
}
