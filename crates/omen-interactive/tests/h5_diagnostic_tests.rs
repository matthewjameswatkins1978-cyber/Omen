use omen_core::{Assurance, ExecutionId, InteractiveSessionId, ResourceUri, ValidityState};
use omen_interactive::InteractiveSession;
use omen_knowledge::{
    Database, DependencyRecord, ExecutionHistory, ExecutionRecord, FactProvenance, FactRecord,
    FactRegistry, PublishFactRequest,
};
use omen_ui::{ColorRoles, DiagnosticLevel, DiagnosticRenderer};
use tempfile::tempdir;

#[test]
fn test_render_execution_progressive_disclosure() {
    let roles = ColorRoles::plain();
    let record = ExecutionRecord {
        execution_id: ExecutionId::new("exec-diag-1").unwrap(),
        session_id: InteractiveSessionId::new("sess-1").unwrap(),
        command: "cargo test --all".into(),
        exit_code: Some(101),
        duration_ms: Some(340),
        stdout_artifact: Some("artifact://sha256:abcd".into()),
        stderr_artifact: Some("artifact://sha256:error1".into()),
        envelope_json: None,
        created_at: "2026-09-18T14:00:00Z".into(),
    };

    // Level 0: Minimal
    let l0 = DiagnosticRenderer::render_execution(&record, DiagnosticLevel::Level0, &roles);
    assert!(l0.contains("cargo test --all"));
    assert!(l0.contains("exit 101"));
    assert!(l0.contains("340ms"));
    assert_eq!(l0.lines().count(), 1);

    // Level 1: Compact
    let l1 = DiagnosticRenderer::render_execution(&record, DiagnosticLevel::Level1, &roles);
    assert!(l1.contains("cargo test --all"));
    assert!(l1.contains("exit code 101"));
    assert!(l1.contains("artifact://sha256:error1"));

    // Level 2: Detailed
    let l2 = DiagnosticRenderer::render_execution(&record, DiagnosticLevel::Level2, &roles);
    assert!(l2.contains("exec-diag-1"));
    assert!(l2.contains("Session:   sess-1"));
    assert!(l2.contains("Stdout CAS: artifact://sha256:abcd"));

    // Level 3: Machine JSON
    let l3 = DiagnosticRenderer::render_execution(&record, DiagnosticLevel::Level3, &roles);
    assert!(l3.contains("\"command\": \"cargo test --all\""));
    assert!(l3.contains("\"exit_code\": 101"));
}

#[test]
fn test_render_fact_provenance_current_vs_dirty() {
    let roles = ColorRoles::plain();
    let prov_current = FactProvenance {
        fact: FactRecord {
            fact_id: omen_core::FactId::new("fact-1").unwrap(),
            resource_uri: ResourceUri::parse("fact://git/branch").unwrap(),
            value: "main".into(),
            validity: ValidityState::Current,
            assurance: Assurance::Observed,
            producer: "git".into(),
            witness: None,
            superseded_by: None,
            created_at: "2026-09-18T14:00:00Z".into(),
        },
        dependencies: vec![DependencyRecord {
            generation_name: "git:head".into(),
            recorded_generation: 1,
            current_generation: 1,
            is_dirty: false,
        }],
        artifacts: vec![],
        history: vec![],
    };

    let rendered_cur =
        DiagnosticRenderer::render_fact_provenance(&prov_current, DiagnosticLevel::Level2, &roles);
    assert!(rendered_cur.contains("CURRENT"));
    assert!(rendered_cur.contains("git:head [OK]"));

    let prov_dirty = FactProvenance {
        fact: FactRecord {
            fact_id: omen_core::FactId::new("fact-2").unwrap(),
            resource_uri: ResourceUri::parse("fact://git/branch").unwrap(),
            value: "main".into(),
            validity: ValidityState::Dirty,
            assurance: Assurance::Observed,
            producer: "git".into(),
            witness: None,
            superseded_by: None,
            created_at: "2026-09-18T14:00:00Z".into(),
        },
        dependencies: vec![DependencyRecord {
            generation_name: "git:head".into(),
            recorded_generation: 1,
            current_generation: 2,
            is_dirty: true,
        }],
        artifacts: vec![],
        history: vec![],
    };

    let rendered_dirty =
        DiagnosticRenderer::render_fact_provenance(&prov_dirty, DiagnosticLevel::Level2, &roles);
    assert!(rendered_dirty.contains("DIRTY"));
    assert!(rendered_dirty.contains("git:head [DIRTY]"));
}

#[test]
fn test_semantic_actions_inspect_show_why() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("diag_actions.db");
    let mut db = Database::open(&db_path).unwrap();

    // Register a fact
    let res = ResourceUri::parse("fact://workspace/build").unwrap();
    FactRegistry::publish_fact(
        &mut db,
        PublishFactRequest {
            resource: &res,
            value: "success",
            assurance: Assurance::Deterministic,
            producer: "cargo",
            witness: None,
            dependencies: &[],
            artifacts: &[],
        },
    )
    .unwrap();

    let mut session = InteractiveSession::new(dir.path().to_path_buf(), Some(db)).unwrap();

    // Record an execution
    let exec = ExecutionRecord {
        execution_id: ExecutionId::new("exec-fail-1").unwrap(),
        session_id: session.session_id.clone(),
        command: "cargo test --failed".into(),
        exit_code: Some(1),
        duration_ms: Some(150),
        stdout_artifact: None,
        stderr_artifact: None,
        envelope_json: None,
        created_at: "2026-09-18T15:00:00Z".into(),
    };
    ExecutionHistory::record_execution(session.db.as_mut().unwrap(), &exec, &[], &[], &[]).unwrap();

    // Dispatch :inspect @last
    let exit1 = session.dispatch_input(":inspect @last").unwrap();
    assert!(exit1.is_zero());

    // Dispatch :show @failed
    let exit2 = session.dispatch_input(":show @failed").unwrap();
    assert!(exit2.is_zero());

    // Dispatch :why @fact://workspace/build
    let exit3 = session
        .dispatch_input(":why @fact://workspace/build")
        .unwrap();
    assert!(exit3.is_zero());
}
