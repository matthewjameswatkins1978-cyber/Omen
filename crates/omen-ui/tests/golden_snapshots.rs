use omen_core::{Assurance, ExecutionId, InteractiveSessionId, ResourceUri, ValidityState};
use omen_knowledge::{DependencyRecord, ExecutionRecord, FactProvenance, FactRecord};
use omen_ui::{
    ColorRoles, DiagnosticLevel, DiagnosticRenderer, PromptDensity, PromptRenderer, PromptState,
    SemanticBlock, TerminalCapabilities, Theme,
};
use std::path::PathBuf;

#[test]
fn test_golden_prompt_dumb_terminal() {
    let caps = TerminalCapabilities::dumb();
    let state_clean = PromptState::new(&PathBuf::from("/workspace/omen"), None, 0, false);
    let rendered_clean = PromptRenderer::render(&state_clean, &caps);

    assert_eq!(rendered_clean, "/workspace/omen ok\n\\O/ > ");

    let state_dirty = PromptState::new(
        &PathBuf::from("/workspace/omen"),
        Some("feature/h11".into()),
        3,
        false,
    );
    let rendered_dirty = PromptRenderer::render(&state_dirty, &caps);
    assert_eq!(
        rendered_dirty,
        "/workspace/omen  feature/h11 ! 3 dirty\n\\O/ > "
    );

    let state_fail = PromptState::new(&PathBuf::from("/workspace/omen"), None, 0, true);
    let rendered_fail = PromptRenderer::render(&state_fail, &caps);
    assert_eq!(rendered_fail, "/workspace/omen X\n\\O/ > ");
}

#[test]
fn test_golden_prompt_rich_terminal_osc133() {
    let caps = TerminalCapabilities {
        is_interactive: true,
        has_color: true,
        has_truecolor: true,
        has_unicode: true,
        has_osc7: true,
        has_osc8: true,
        has_osc133: true,
        width: 80,
        height: 24,
    };

    let state_clean = PromptState::new(&PathBuf::from("/repo"), Some("main".into()), 0, false);
    let rendered = PromptRenderer::render(&state_clean, &caps);

    // Verify OSC 133 markers
    assert!(rendered.starts_with("\x1b]133;A\x1b\\"));
    assert!(rendered.ends_with("\x1b]133;B\x1b\\"));
    assert!(rendered.contains("✓"));
    assert!(rendered.contains("/repo"));
    assert!(rendered.contains("main"));
}

#[test]
fn test_compact_prompt_is_stable_and_theme_is_presentation_only() {
    let caps = TerminalCapabilities::dumb();
    let state = PromptState::new(&PathBuf::from("/repo"), Some("main".into()), 0, false)
        .with_density(PromptDensity::Compact)
        .with_theme(Theme::Amber);
    assert_eq!(PromptRenderer::render(&state, &caps), "\\O/ > ");
}

#[test]
fn test_golden_semantic_blocks() {
    let caps = TerminalCapabilities {
        is_interactive: true,
        has_color: true,
        has_truecolor: true,
        has_unicode: true,
        has_osc7: true,
        has_osc8: true,
        has_osc133: true,
        width: 80,
        height: 24,
    };

    let osc7 = SemanticBlock::osc7_cwd(&PathBuf::from("/test/dir"), &caps);
    assert!(osc7.starts_with("\x1b]7;file://localhost/"));
    assert!(osc7.ends_with("\x1b\\"));

    let osc8 = SemanticBlock::osc8_link(
        "artifact://sha256/1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        "output.log",
        &caps,
    );
    assert!(osc8.contains("\x1b]8;;artifact://sha256/"));
    assert!(osc8.contains("output.log"));

    let osc133_exec = SemanticBlock::osc133_command_executed(&caps);
    assert_eq!(osc133_exec, "\x1b]133;C\x1b\\");

    let osc133_exit0 = SemanticBlock::osc133_command_finished(0, &caps);
    assert_eq!(osc133_exit0, "\x1b]133;D;0\x1b\\");

    let osc133_exit1 = SemanticBlock::osc133_command_finished(1, &caps);
    assert_eq!(osc133_exit1, "\x1b]133;D;1\x1b\\");
}

#[test]
fn test_golden_diagnostics_levels() {
    let roles = ColorRoles::plain();
    let record = ExecutionRecord {
        execution_id: ExecutionId::new("exec-golden-1").unwrap(),
        session_id: InteractiveSessionId::new("sess-1").unwrap(),
        command: "cargo check --workspace".into(),
        exit_code: Some(0),
        duration_ms: Some(215),
        stdout_artifact: Some("artifact://sha256/stdout-digest".into()),
        stderr_artifact: None,
        envelope_json: None,
        created_at: "2026-09-18T16:00:00Z".into(),
    };

    // Level 0
    let l0 = DiagnosticRenderer::render_execution(&record, DiagnosticLevel::Level0, &roles);
    assert_eq!(l0, "✓ cargo check --workspace (exit 0, 215ms)");

    // Level 1
    let l1 = DiagnosticRenderer::render_execution(&record, DiagnosticLevel::Level1, &roles);
    assert!(l1.contains("Command: cargo check --workspace"));
    assert!(l1.contains("Outcome: SUCCESS"));

    // Level 2
    let l2 = DiagnosticRenderer::render_execution(&record, DiagnosticLevel::Level2, &roles);
    assert!(l2.contains("Execution Diagnostic: exec-golden-1"));
    assert!(l2.contains("Session:   sess-1"));
    assert!(l2.contains("Stdout CAS: artifact://sha256/stdout-digest"));

    // Level 3
    let l3 = DiagnosticRenderer::render_execution(&record, DiagnosticLevel::Level3, &roles);
    assert!(l3.contains("\"execution_id\": \"exec-golden-1\""));
    assert!(l3.contains("\"exit_code\": 0"));
    assert!(l3.contains("\"duration_ms\": 215"));
}

#[test]
fn test_human_error_is_concise_and_keeps_machine_code() {
    let roles = ColorRoles::plain();
    let error = omen_core::CoreError::ExecutionFailedCode {
        code: omen_core::ErrorCode::PlanChanged,
        message: "project changed after planning".into(),
    };
    let rendered = DiagnosticRenderer::render_human_error(&error, &roles);
    assert!(rendered.contains("\\O/ PLAN_CHANGED"));
    assert!(rendered.contains("project changed after planning"));
    assert!(rendered.contains("Plan again before executing"));
}

#[test]
fn test_golden_fact_provenance_clean_vs_dirty() {
    let roles = ColorRoles::plain();

    let prov_clean = FactProvenance {
        fact: FactRecord {
            fact_id: omen_core::FactId::new("fact-gold-1").unwrap(),
            resource_uri: ResourceUri::parse("fact://workspace/status").unwrap(),
            value: "healthy".into(),
            validity: ValidityState::Current,
            assurance: Assurance::Verified,
            producer: "cargo-check".into(),
            witness: Some("git-rev-abc".into()),
            superseded_by: None,
            created_at: "2026-09-18T16:00:00Z".into(),
        },
        dependencies: vec![DependencyRecord {
            generation_name: "fs:workspace".into(),
            recorded_generation: 1,
            current_generation: 1,
            is_dirty: false,
        }],
        artifacts: vec![],
        history: vec![],
    };

    let rendered_clean =
        DiagnosticRenderer::render_fact_provenance(&prov_clean, DiagnosticLevel::Level2, &roles);
    assert!(rendered_clean.contains("Validity:   CURRENT"));
    assert!(rendered_clean.contains("Assurance:  Verified"));
    assert!(rendered_clean.contains("fs:workspace [OK]: recorded=1, current=1"));
    assert!(!rendered_clean.contains("[DIRTY]"));

    let prov_dirty = FactProvenance {
        fact: FactRecord {
            fact_id: omen_core::FactId::new("fact-gold-2").unwrap(),
            resource_uri: ResourceUri::parse("fact://workspace/status").unwrap(),
            value: "healthy".into(),
            validity: ValidityState::Dirty,
            assurance: Assurance::Verified,
            producer: "cargo-check".into(),
            witness: Some("git-rev-abc".into()),
            superseded_by: None,
            created_at: "2026-09-18T16:00:00Z".into(),
        },
        dependencies: vec![DependencyRecord {
            generation_name: "fs:workspace".into(),
            recorded_generation: 1,
            current_generation: 2,
            is_dirty: true,
        }],
        artifacts: vec![],
        history: vec![],
    };

    let rendered_dirty =
        DiagnosticRenderer::render_fact_provenance(&prov_dirty, DiagnosticLevel::Level2, &roles);
    assert!(rendered_dirty.contains("Validity:   DIRTY"));
    assert!(rendered_dirty.contains("fs:workspace [DIRTY]: recorded=1, current=2"));
}
