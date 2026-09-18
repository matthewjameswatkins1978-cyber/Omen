use omen_ui::{PromptRenderer, PromptState, SemanticBlock, TerminalCapabilities};
use std::path::Path;

#[test]
fn test_osc7_cwd_reporting_and_fallback() {
    let mut caps_enabled = TerminalCapabilities::dumb();
    caps_enabled.has_osc7 = true;

    let caps_disabled = TerminalCapabilities::dumb();

    let path = Path::new("/workspace/project");
    let seq_enabled = SemanticBlock::osc7_cwd(path, &caps_enabled);
    assert!(seq_enabled.starts_with("\x1b]7;file://localhost/"));
    assert!(seq_enabled.ends_with("\x1b\\"));
    assert!(seq_enabled.contains("/workspace/project"));

    let seq_disabled = SemanticBlock::osc7_cwd(path, &caps_disabled);
    assert_eq!(seq_disabled, "");
}

#[test]
fn test_osc8_hyperlinks_and_fallback() {
    let mut caps_enabled = TerminalCapabilities::dumb();
    caps_enabled.has_osc8 = true;

    let caps_disabled = TerminalCapabilities::dumb();

    let url = "artifact://sha256/deadbeef";
    let text = "Build Artifact";

    let link_enabled = SemanticBlock::osc8_link(url, text, &caps_enabled);
    assert_eq!(
        link_enabled,
        format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
    );

    let link_disabled = SemanticBlock::osc8_link(url, text, &caps_disabled);
    assert_eq!(link_disabled, text);
}

#[test]
fn test_osc133_semantic_markers_and_fallback() {
    let mut caps_enabled = TerminalCapabilities::dumb();
    caps_enabled.has_osc133 = true;

    let caps_disabled = TerminalCapabilities::dumb();

    // Enabled
    assert_eq!(
        SemanticBlock::osc133_prompt_start(&caps_enabled),
        "\x1b]133;A\x1b\\"
    );
    assert_eq!(
        SemanticBlock::osc133_command_start(&caps_enabled),
        "\x1b]133;B\x1b\\"
    );
    assert_eq!(
        SemanticBlock::osc133_command_executed(&caps_enabled),
        "\x1b]133;C\x1b\\"
    );
    assert_eq!(
        SemanticBlock::osc133_command_finished(0, &caps_enabled),
        "\x1b]133;D;0\x1b\\"
    );
    assert_eq!(
        SemanticBlock::osc133_command_finished(101, &caps_enabled),
        "\x1b]133;D;101\x1b\\"
    );

    // Disabled
    assert_eq!(SemanticBlock::osc133_prompt_start(&caps_disabled), "");
    assert_eq!(SemanticBlock::osc133_command_start(&caps_disabled), "");
    assert_eq!(SemanticBlock::osc133_command_executed(&caps_disabled), "");
    assert_eq!(
        SemanticBlock::osc133_command_finished(0, &caps_disabled),
        ""
    );
}

#[test]
fn test_dumb_terminal_emits_no_osc_sequences() {
    let caps_dumb = TerminalCapabilities::dumb();
    let state = PromptState::new(Path::new("/test"), Some("main".into()), 0, false);
    let prompt = PromptRenderer::render(&state, &caps_dumb);

    // Prompt contains no escape sequences
    assert!(!prompt.contains("\x1b"));
    assert!(prompt.contains(">"));

    let link = SemanticBlock::link_artifact("artifact://sha256/abc", Some("Label"), &caps_dumb);
    assert_eq!(link, "Label");
    assert!(!link.contains("\x1b"));
}
