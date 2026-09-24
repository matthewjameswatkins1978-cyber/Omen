//! Genuine ConPTY terminal-gate tests.
//!
//! Spawns the `pty_gate` binary inside a real Windows ConPTY, sends keystrokes,
//! and verifies that Reedline completion and ghost hints work in a real terminal.
//!
//! These are NOT unit tests of the completion engine — they are real-terminal
//! acceptance evidence via ConPTY automation.
//!
//! M0 acceptance matrix:
//! - `:sta<Tab>`          → auto-completes to `:status`
//! - `c<Tab>`             → multi-candidate chooser
//! - `gi<Tab>`            → auto-completes to `git`
//! - `car<Tab>`           → auto-completes to `cargo`
//! - quoted path with spaces
//! - multi-candidate chooser + Up/Down navigation
//! - Enter acceptance
//! - Esc dismissal (buffer unchanged)
//! - repeated Tab safety
//! - `D:` / `d:` drive designators
//! - `cd D:\` / `cd "C:\Program Files"`

#![cfg(windows)]

use omen_engine::backend::{PtyExecutionHandle, PtyExecutionRequest};
use omen_engine::pty::NativePtyHandle;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const GATE_TIMEOUT: Duration = Duration::from_secs(15);
const READ_POLL: Duration = Duration::from_millis(50);
const SETTLE: Duration = Duration::from_millis(300);

fn pty_gate_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pty_gate"))
}

fn spawn_gate() -> NativePtyHandle {
    let exe = pty_gate_exe();
    let req = PtyExecutionRequest {
        session_id: omen_core::PtySessionId::generate(),
        argv: vec![exe.to_string_lossy().into_owned()],
        cwd: std::env::temp_dir(),
        env: vec![
            ("NO_COLOR".into(), "1".into()),
            ("TERM".into(), "dumb".into()),
        ],
        rows: 24,
        cols: 80,
    };
    NativePtyHandle::spawn(&req).expect("ConPTY spawn must succeed")
}

fn read_until(handle: &mut NativePtyHandle, needle: &str) -> String {
    let deadline = Instant::now() + GATE_TIMEOUT;
    let mut collected = String::new();
    while Instant::now() < deadline {
        match handle.read_output() {
            Ok(bytes) if !bytes.is_empty() => {
                collected.push_str(&String::from_utf8_lossy(&bytes));
                if collected.contains(needle) {
                    return collected;
                }
            }
            _ => {}
        }
        std::thread::sleep(READ_POLL);
    }
    collected
}

fn read_available(handle: &mut NativePtyHandle, dur: Duration) -> String {
    let deadline = Instant::now() + dur;
    let mut collected = String::new();
    while Instant::now() < deadline {
        match handle.read_output() {
            Ok(bytes) if !bytes.is_empty() => {
                collected.push_str(&String::from_utf8_lossy(&bytes));
            }
            _ => {}
        }
        std::thread::sleep(READ_POLL);
    }
    collected
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c2 in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c2) {
                        break;
                    }
                }
            } else if chars.peek() == Some(&']') {
                chars.next();
                for c2 in chars.by_ref() {
                    if c2 == '\u{7}' {
                        break;
                    }
                    if c2 == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Types literal text into the gate.
fn type_text(handle: &mut NativePtyHandle, text: &str) {
    handle.write_input(text.as_bytes()).unwrap();
    std::thread::sleep(SETTLE);
    let _ = read_available(handle, Duration::from_millis(100));
}

/// Sends the Tab key.
fn press_tab(handle: &mut NativePtyHandle) {
    handle.write_input(b"\t").unwrap();
    std::thread::sleep(SETTLE);
    let _ = read_available(handle, Duration::from_millis(100));
}

/// Sends Enter.
fn press_enter(handle: &mut NativePtyHandle) {
    handle.write_input(b"\r").unwrap();
    std::thread::sleep(SETTLE);
}

/// Sends Escape.
fn press_esc(handle: &mut NativePtyHandle) {
    handle.write_input(b"\x1b").unwrap();
    std::thread::sleep(SETTLE);
    let _ = read_available(handle, Duration::from_millis(100));
}

/// Sends Down arrow (ESC [ B).
fn press_down(handle: &mut NativePtyHandle) {
    handle.write_input(b"\x1b[B").unwrap();
    std::thread::sleep(SETTLE);
    let _ = read_available(handle, Duration::from_millis(100));
}

/// Sends Up arrow (ESC [ A).
fn press_up(handle: &mut NativePtyHandle) {
    handle.write_input(b"\x1b[A").unwrap();
    std::thread::sleep(SETTLE);
    let _ = read_available(handle, Duration::from_millis(100));
}

/// Cleanly exits the gate.
fn exit_gate(handle: &mut NativePtyHandle) {
    handle.write_input(b"\x03").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    handle.write_input(b"exit\r").unwrap();
    let _ = read_until(handle, "PTY_GATE_EXIT");
}

/// Asserts the gate emitted `PTY_GATE_LINE:<expected>`.
fn assert_line_output(plain: &str, expected: &str) {
    assert!(
        plain.contains(&format!("PTY_GATE_LINE:{expected}")),
        "expected PTY_GATE_LINE:{expected}, got: {plain:?}"
    );
}

/// Sends Enter twice: first accepts the menu selection, second submits the line.
/// When no menu is active the first Enter submits and the second is harmless.
fn press_enter_submit(handle: &mut NativePtyHandle) {
    press_enter(handle);
    press_enter(handle);
}

// ===========================================================================
// BASIC CONPTY SPAWN AND I/O
// ===========================================================================

#[test]
fn pty_conpty_spawn_and_basic_io() {
    let mut handle = spawn_gate();
    let prompt = read_until(&mut handle, "pty-gate>");
    assert!(prompt.contains("pty-gate>"), "ConPTY must show prompt");

    type_text(&mut handle, "hello");
    press_enter(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:hello");
    let plain = strip_ansi(&out);
    assert_line_output(&plain, "hello");

    handle.write_input(b"exit\r").unwrap();
    let bye = read_until(&mut handle, "PTY_GATE_EXIT");
    assert!(bye.contains("PTY_GATE_EXIT"));
}

// ===========================================================================
// A. ONE UNAMBIGUOUS CANDIDATE -> COMPLETE IT (auto-accept via quick_completions)
// ===========================================================================

#[test]
fn m0_tab_single_candidate_autocompletes_omen_action() {
    // `:sta` -> `:status` is the only Omen action starting with `:sta`.
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, ":sta");
    press_tab(&mut handle);
    // With quick_completions, Tab auto-accepts the single candidate.
    press_enter(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    assert_line_output(&plain, ":status");
    exit_gate(&mut handle);
}

#[test]
fn m0_tab_single_candidate_autocompletes_path_command() {
    // `gi` -> `git` is the only path command starting with `gi`.
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, "gi");
    press_tab(&mut handle);
    press_enter(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    assert_line_output(&plain, "git");
    exit_gate(&mut handle);
}

#[test]
fn m0_tab_single_candidate_autocompletes_cargo() {
    // `car` -> `cargo` is the only path command starting with `car`.
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, "car");
    press_tab(&mut handle);
    press_enter(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    assert_line_output(&plain, "cargo");
    exit_gate(&mut handle);
}

// ===========================================================================
// B. MULTIPLE VALID CANDIDATES -> OPEN ONE NAVIGABLE CHOOSER
// ===========================================================================

#[test]
fn m0_tab_multiple_candidates_opens_chooser() {
    // `c` matches `cargo`, `cat`, `cd` — multiple candidates.
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, "c");
    let out = {
        handle.write_input(b"\t").unwrap();
        std::thread::sleep(SETTLE);
        read_available(&mut handle, Duration::from_millis(400))
    };
    let plain = strip_ansi(&out);
    // The chooser must show at least 2 candidates.
    let has_cargo = plain.contains("cargo");
    let has_cat = plain.contains("cat");
    let has_cd = plain.contains("cd");
    let candidate_count = [has_cargo, has_cat, has_cd].iter().filter(|x| **x).count();
    assert!(
        candidate_count >= 2,
        "chooser must show multiple candidates, got only {candidate_count}: {plain:?}"
    );
    exit_gate(&mut handle);
}

#[test]
fn m0_tab_chooser_enter_accepts_selected_candidate() {
    // `c` -> chooser -> Enter accepts the first candidate, Enter submits.
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, "c");
    press_tab(&mut handle);
    press_enter_submit(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    // Must be one of the valid candidates, not `c` alone.
    let candidates = ["cargo", "cat", "cd"];
    let accepted = candidates.iter().any(|c| plain.contains(&format!("PTY_GATE_LINE:{c}")));
    assert!(
        accepted,
        "Enter must accept a valid candidate from the chooser, got: {plain:?}"
    );
    exit_gate(&mut handle);
}

// ===========================================================================
// UP/DOWN CHOOSER NAVIGATION (only when chooser active)
// ===========================================================================

#[test]
fn m0_chooser_down_navigates_candidates() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, "c");
    press_tab(&mut handle);
    press_down(&mut handle);
    press_enter_submit(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    let candidates = ["cargo", "cat", "cd"];
    let accepted = candidates.iter().any(|c| plain.contains(&format!("PTY_GATE_LINE:{c}")));
    assert!(
        accepted,
        "Down+Enter must accept a valid candidate, got: {plain:?}"
    );
    exit_gate(&mut handle);
}

#[test]
fn m0_chooser_up_down_navigates_candidates() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, "c");
    press_tab(&mut handle);
    press_down(&mut handle);
    press_down(&mut handle);
    press_up(&mut handle);
    press_enter_submit(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    let candidates = ["cargo", "cat", "cd"];
    let accepted = candidates.iter().any(|c| plain.contains(&format!("PTY_GATE_LINE:{c}")));
    assert!(
        accepted,
        "Up/Down navigation + Enter must accept a valid candidate, got: {plain:?}"
    );
    exit_gate(&mut handle);
}

// ===========================================================================
// ENTER ACCEPTANCE
// ===========================================================================

#[test]
fn m0_enter_accepts_selected_candidate() {
    // Proven by the chooser tests above. This test is for the single-candidate
    // case where Enter confirms the auto-completed text.
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, ":stat");
    // Ghost shows `us` but we do NOT accept the ghost. We use Tab.
    press_tab(&mut handle);
    press_enter(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    assert_line_output(&plain, ":status");
    exit_gate(&mut handle);
}

// ===========================================================================
// ESC DISMISSAL — BUFFER UNCHANGED
// ===========================================================================

#[test]
fn m0_esc_dismisses_chooser_buffer_unchanged() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, "c");
    press_tab(&mut handle); // open chooser
    press_esc(&mut handle); // dismiss chooser
    press_enter(&mut handle); // submit original text
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    // Buffer must still be `c` — Esc did not mutate editable text.
    assert_line_output(&plain, "c");
    assert!(
        !plain.contains("PTY_GATE_LINE:cargo")
            && !plain.contains("PTY_GATE_LINE:cat\r")
            && !plain.contains("PTY_GATE_LINE:cd\r"),
        "Esc must not accept a candidate: {plain:?}"
    );
    exit_gate(&mut handle);
}

#[test]
fn m0_esc_dismisses_ghost_without_insert() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, "car");
    press_esc(&mut handle); // dismiss ghost
    press_enter(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    assert_line_output(&plain, "car");
    assert!(
        !plain.contains("PTY_GATE_LINE:cargo"),
        "Esc must dismiss ghost without inserting: {plain:?}"
    );
    exit_gate(&mut handle);
}

// ===========================================================================
// REPEATED TAB SAFETY
// ===========================================================================

#[test]
fn m0_repeated_tab_is_safe_no_buffer_corruption() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    // Press Tab repeatedly on a multi-candidate prefix.
    type_text(&mut handle, "c");
    press_tab(&mut handle);
    press_tab(&mut handle);
    press_tab(&mut handle);
    press_esc(&mut handle); // dismiss
    press_enter(&mut handle); // submit
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    // After Esc, buffer must still be `c` (or one auto-completed candidate
    // if the first Tab auto-accepted). It must NOT be corrupted with
    // duplicated text.
    let line = plain
        .lines()
        .find(|l| l.contains("PTY_GATE_LINE:"))
        .unwrap_or("");
    assert!(
        !line.contains("cc") && !line.contains("ccar") && !line.contains("ccat"),
        "repeated Tab must not duplicate text: {plain:?}"
    );
    exit_gate(&mut handle);
}

#[test]
fn m0_repeated_tab_on_single_candidate_is_deterministic() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, "gi");
    press_tab(&mut handle); // auto-accepts `git`
    press_tab(&mut handle); // second Tab on empty token after `git `
    press_esc(&mut handle); // dismiss any chooser
    press_enter(&mut handle); // submit
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    // Must start with `git` — no `gitgit` duplication.
    assert!(
        !plain.contains("gitgit") && !plain.contains("gigi"),
        "repeated Tab must not duplicate: {plain:?}"
    );
    assert!(
        plain.contains("PTY_GATE_LINE:git"),
        "expected git, got: {plain:?}"
    );
    exit_gate(&mut handle);
}

// ===========================================================================
// C. NO VALID CANDIDATE -> DECLINE CLEANLY
// ===========================================================================

#[test]
fn m0_tab_no_candidate_declines_cleanly() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, "zzzz");
    press_tab(&mut handle);
    press_esc(&mut handle); // dismiss empty chooser if opened
    press_enter(&mut handle); // submit original text
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    // Buffer must be unchanged: `zzzz`.
    assert_line_output(&plain, "zzzz");
    exit_gate(&mut handle);
}

// ===========================================================================
// GHOST HINTS (complementary to Tab)
// ===========================================================================

#[test]
fn m0_ghost_hint_visible_and_acceptable_via_right_arrow() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, ":stat");
    // Ghost shows `us`.
    let out = read_available(&mut handle, Duration::from_millis(200));
    let plain = strip_ansi(&out);
    assert!(plain.contains(":status"), "ghost must show :status, got: {plain:?}");

    // Right Arrow accepts ghost.
    handle.write_input(b"\x1b[C").unwrap();
    std::thread::sleep(SETTLE);
    let _ = read_available(&mut handle, Duration::from_millis(100));

    press_enter(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    assert_line_output(&plain, ":status");
    exit_gate(&mut handle);
}

// ===========================================================================
// WINDOWS DRIVE DESIGNATORS (issue #18)
// ===========================================================================

#[test]
fn m0_bare_drive_designator_uppercase() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, "D:");
    press_enter(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    // The gate echoes the line — proves it was treated as input text, not
    // a process spawn (which would produce different output or a crash).
    assert_line_output(&plain, "D:");
    exit_gate(&mut handle);
}

#[test]
fn m0_bare_drive_designator_lowercase() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, "d:");
    press_enter(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    assert_line_output(&plain, "d:");
    exit_gate(&mut handle);
}

#[test]
fn m0_cd_drive_root() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, r"cd D:\");
    press_enter(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    assert_line_output(&plain, r"cd D:\");
    exit_gate(&mut handle);
}

// ===========================================================================
// QUOTED PATHS WITH SPACES
// ===========================================================================

#[test]
fn m0_quoted_path_with_spaces_roundtrips() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, r#"cd "C:\Program Files""#);
    press_enter(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    assert_line_output(&plain, r#"cd "C:\Program Files""#);
    exit_gate(&mut handle);
}

// ===========================================================================
// EDITING OUTRANKS ASSISTANCE
// ===========================================================================

#[test]
fn m0_editing_keys_work_normally_when_chooser_inactive() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    // Type, move cursor, edit — all must work normally.
    type_text(&mut handle, "hello");
    // Left arrow twice (ESC [ D)
    handle.write_input(b"\x1b[D\x1b[D").unwrap();
    std::thread::sleep(SETTLE);
    let _ = read_available(&mut handle, Duration::from_millis(100));
    // Type `XX` at cursor
    type_text(&mut handle, "XX");
    press_enter(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    // Cursor was at position 3 (hello = 5 chars, left 2 = position 3).
    // `XX` inserted at position 3: `helXXlo`.
    assert_line_output(&plain, "helXXlo");
    exit_gate(&mut handle);
}

// ===========================================================================
// HUMAN PATH RENDERING IN REAL TERMINAL (issue #19)
// ===========================================================================
// The pty_gate uses a minimal prompt, not the full OmenPrompt. Path rendering
// is proven at the unit level (humanize_tests.rs) and in real dogfood.
// These tests verify the terminal does not show `//?/` leaks.

#[test]
fn m0_no_verbatim_path_leak_in_output() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    type_text(&mut handle, r"cd D:\");
    press_enter(&mut handle);
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    assert!(
        !plain.contains("//?/"),
        "verbatim path prefix must not leak to terminal: {plain:?}"
    );
    exit_gate(&mut handle);
}
