use omen_interactive::{ChildHandoff, InteractiveSession};
use omen_ui::{PromptRenderer, PromptState, TerminalCapabilities};
use tempfile::tempdir;

#[test]
fn test_terminal_capabilities_dumb_and_detection() {
    let dumb = TerminalCapabilities::dumb();
    assert!(!dumb.is_interactive);
    assert!(!dumb.has_color);
    assert!(!dumb.has_osc7);

    let detected = TerminalCapabilities::detect();
    assert!(detected.width > 0);
    assert!(detected.height > 0);
}

#[test]
fn test_prompt_rendering_levels() {
    let temp = tempdir().unwrap();
    let caps_color = TerminalCapabilities {
        is_interactive: true,
        has_color: true,
        has_truecolor: false,
        has_unicode: true,
        has_osc7: false,
        has_osc8: false,
        has_osc133: false,
        width: 80,
        height: 24,
    };

    // 1. Clean prompt
    let clean_state = PromptState::new(temp.path(), Some("main".into()), 0, false);
    let rendered_clean = PromptRenderer::render(&clean_state, &caps_color);
    assert!(rendered_clean.contains("main"));
    assert!(rendered_clean.contains("✓"));
    assert!(rendered_clean.contains(">"));

    // 2. Dirty facts prompt
    let dirty_state = PromptState::new(temp.path(), Some("main".into()), 3, false);
    let rendered_dirty = PromptRenderer::render(&dirty_state, &caps_color);
    assert!(rendered_dirty.contains("3 dirty"));

    // 3. Failed command prompt
    let fail_state = PromptState::new(temp.path(), Some("main".into()), 0, true);
    let rendered_fail = PromptRenderer::render(&fail_state, &caps_color);
    assert!(rendered_fail.contains("✕"));

    // 4. Dumb/Plain terminal fallback
    let caps_dumb = TerminalCapabilities::dumb();
    let rendered_dumb = PromptRenderer::render(&clean_state, &caps_dumb);
    assert!(rendered_dumb.contains("ok"));
    assert!(!rendered_dumb.contains("\x1b[")); // No ANSI escapes in plain/dumb mode
}

#[test]
fn test_child_interactive_detection() {
    assert!(ChildHandoff::is_interactive_command("vim"));
    assert!(ChildHandoff::is_interactive_command("/usr/bin/nano"));
    assert!(ChildHandoff::is_interactive_command("less"));
    assert!(!ChildHandoff::is_interactive_command("cargo"));
    assert!(!ChildHandoff::is_interactive_command("git status"));
}

#[test]
fn test_interactive_session_creation_and_dispatch() {
    let temp = tempdir().unwrap();
    let session = InteractiveSession::new(temp.path().to_path_buf(), None);
    assert!(session.is_ok());
    let mut session = session.unwrap();

    assert_eq!(session.cwd, temp.path());
    assert!(session.session_id.as_str().starts_with("sess-"));

    // Test non-interactive command execution dispatch
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();

    let exit = session.dispatch_input("cargo --version");
    assert!(exit.is_ok());
    let exit = exit.unwrap();
    assert!(exit.is_zero());
    assert!(session.last_exit.is_some());
}
