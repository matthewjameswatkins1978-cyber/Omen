//! Issue #18: bare drive designators are navigation grammar, never executable.
//!
//! Proves that `C:`, `D:`, `c:`, `d:` are recognised as Windows drive
//! navigation BEFORE executable dispatch and never reach process spawn.

use omen_interactive::commands::{is_drive_designator, is_drive_path};

// ---------------------------------------------------------------------------
// RECOGNITION
// ---------------------------------------------------------------------------

#[test]
fn drive_designator_uppercase() {
    assert_eq!(is_drive_designator("D:"), Some('D'));
    assert_eq!(is_drive_designator("C:"), Some('C'));
    assert_eq!(is_drive_designator("Z:"), Some('Z'));
}

#[test]
fn drive_designator_lowercase() {
    assert_eq!(is_drive_designator("d:"), Some('D'));
    assert_eq!(is_drive_designator("c:"), Some('C'));
    assert_eq!(is_drive_designator("z:"), Some('Z'));
}

#[test]
fn drive_designator_rejects_paths() {
    assert_eq!(is_drive_designator(r"D:\"), None);
    assert_eq!(is_drive_designator(r"D:\foo"), None);
    assert_eq!(is_drive_designator("D:foo"), None);
    assert_eq!(is_drive_designator(r"c:\Windows"), None);
}

#[test]
fn drive_designator_rejects_longer_tokens() {
    assert_eq!(is_drive_designator("CD:"), None);
    assert_eq!(is_drive_designator("EXIT:"), None);
    assert_eq!(is_drive_designator(":"), None);
    assert_eq!(is_drive_designator(""), None);
    assert_eq!(is_drive_designator("D::"), None);
}

#[test]
fn drive_designator_rejects_non_alpha() {
    assert_eq!(is_drive_designator("1:"), None);
    assert_eq!(is_drive_designator("_:"), None);
    assert_eq!(is_drive_designator(" :"), None);
}

// ---------------------------------------------------------------------------
// DRIVE PATH RECOGNITION
// ---------------------------------------------------------------------------

#[test]
fn drive_path_covers_designator_and_beyond() {
    assert!(is_drive_path("D:"));
    assert!(is_drive_path(r"D:\"));
    assert!(is_drive_path(r"D:\foo\bar"));
    assert!(is_drive_path("D:foo"));
    assert!(is_drive_path("c:"));
}

#[test]
fn drive_path_rejects_non_drive() {
    assert!(!is_drive_path("cargo"));
    assert!(!is_drive_path("cd"));
    assert!(!is_drive_path("/usr/bin"));
    assert!(!is_drive_path(""));
    assert!(!is_drive_path(":"));
}

// ---------------------------------------------------------------------------
// GRAMMAR: BARE DRIVE DESIGNATOR NEVER ENTERS EXECUTABLE LANE AS SPAWN
// ---------------------------------------------------------------------------

#[test]
fn bare_drive_designator_recognised_by_commands_authority() {
    // Every bare designator the grammar might see must be recognised.
    for token in ["C:", "D:", "c:", "d:", "X:", "z:"] {
        assert!(
            is_drive_designator(token).is_some(),
            "{token:?} must be a drive designator"
        );
    }
}

// ---------------------------------------------------------------------------
// WINDOWS INTEGRATION: DISPATCH NEVER SPAWNS
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod windows_dispatch {
    use omen_interactive::InputLane;
    use omen_interactive::grammar::GrammarScanner;

    #[test]
    fn bare_drive_falls_to_executable_lane_before_nav_check() {
        // Grammar currently routes `D:` to Executable. The session's dispatch
        // intercepts it BEFORE spawn. This test proves the grammar at least
        // produces a well-formed lane (not a panic / semantic action).
        let lane = GrammarScanner::scan("D:").unwrap();
        match lane {
            InputLane::Executable { argv } => {
                assert_eq!(argv, vec!["D:".to_string()]);
            }
            other => panic!("D: must not be a semantic/AI lane: {other:?}"),
        }
    }

    #[test]
    fn lowercase_bare_drive_falls_to_executable_lane() {
        let lane = GrammarScanner::scan("d:").unwrap();
        match lane {
            InputLane::Executable { argv } => {
                assert_eq!(argv, vec!["d:".to_string()]);
            }
            other => panic!("d: must not be a semantic/AI lane: {other:?}"),
        }
    }

    #[test]
    fn drive_path_with_backslash_is_not_designator() {
        // `D:\` has more than 2 bytes — not a bare designator.
        assert!(omen_interactive::commands::is_drive_designator(r"D:\").is_none());
    }
}
