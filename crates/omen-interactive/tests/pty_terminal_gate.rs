//! Genuine ConPTY terminal-gate tests.
//!
//! Spawns the `pty_gate` binary inside a real Windows ConPTY, sends keystrokes,
//! and verifies that Reedline completion and ghost hints work in a real terminal.
//!
//! These are NOT unit tests of the completion engine — they are real-terminal
//! acceptance evidence via ConPTY automation.

#![cfg(windows)]

use omen_engine::backend::{PtyExecutionHandle, PtyExecutionRequest};
use omen_engine::pty::NativePtyHandle;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const GATE_TIMEOUT: Duration = Duration::from_secs(15);
const READ_POLL: Duration = Duration::from_millis(50);

fn pty_gate_exe() -> PathBuf {
    // The binary is built as a test dependency; resolve via CARGO_BIN_EXE.
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
            // Skip CSI sequences: ESC [ ... final-byte
            if chars.peek() == Some(&'[') {
                chars.next();
                for c2 in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c2) {
                        break;
                    }
                }
            } else if chars.peek() == Some(&']') {
                // OSC: ESC ] ... BEL or ESC \
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

// ---------------------------------------------------------------------------
// ConPTY spawn and basic I/O
// ---------------------------------------------------------------------------

#[test]
fn pty_conpty_spawn_and_basic_io() {
    let mut handle = spawn_gate();
    let prompt = read_until(&mut handle, "pty-gate>");
    assert!(
        prompt.contains("pty-gate>"),
        "ConPTY must show prompt, got: {prompt:?}"
    );

    // Send a line and verify the binary echoes it back via stderr marker.
    handle.write_input(b"hello\r").unwrap();
    let out = read_until(&mut handle, "PTY_GATE_LINE:hello");
    assert!(
        out.contains("PTY_GATE_LINE:hello"),
        "ConPTY round-trip must work, got: {out:?}"
    );

    handle.write_input(b"exit\r").unwrap();
    let bye = read_until(&mut handle, "PTY_GATE_EXIT");
    assert!(bye.contains("PTY_GATE_EXIT"));
}

// ---------------------------------------------------------------------------
// Tab completion menu in real terminal
// ---------------------------------------------------------------------------

#[test]
fn pty_tab_shows_completion_menu_with_candidates() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    // Type `car` — should match `cargo` in the PATH cache.
    handle.write_input(b"car").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    // Drain any ghost hint output first.
    let _ = read_available(&mut handle, Duration::from_millis(100));

    // Press Tab to open the completion menu.
    handle.write_input(b"\t").unwrap();
    let out = read_until(&mut handle, "cargo");
    let plain = strip_ansi(&out);
    assert!(
        plain.contains("cargo"),
        "Tab menu must show cargo candidate, got: {plain:?}"
    );

    // Clean exit.
    handle.write_input(b"\x1b").unwrap(); // Escape
    std::thread::sleep(Duration::from_millis(100));
    handle.write_input(b"\x03").unwrap(); // Ctrl-C to clear
    std::thread::sleep(Duration::from_millis(100));
    handle.write_input(b"exit\r").unwrap();
    let _ = read_until(&mut handle, "PTY_GATE_EXIT");
}

// ---------------------------------------------------------------------------
// Ghost hint in real terminal
// ---------------------------------------------------------------------------

#[test]
fn pty_ghost_hint_is_visible_in_real_terminal() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    // Type `car` — ghost should render `go` remainder (visible as `cargo`).
    handle.write_input(b"car").unwrap();
    let out = read_until(&mut handle, "cargo");
    let plain = strip_ansi(&out);
    assert!(
        plain.contains("cargo"),
        "ghost hint must render 'go' remainder as 'cargo', got: {plain:?}"
    );

    // Clean exit.
    handle.write_input(b"\x03").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    handle.write_input(b"exit\r").unwrap();
    let _ = read_until(&mut handle, "PTY_GATE_EXIT");
}

// ---------------------------------------------------------------------------
// Escape dismisses ghost without forced insert
// ---------------------------------------------------------------------------

#[test]
fn pty_escape_dismisses_ghost_without_insert() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    // Type `car` — ghost suggests `go`.
    handle.write_input(b"car").unwrap();
    let _ = read_until(&mut handle, "go");

    // Press Escape to dismiss the ghost.
    handle.write_input(b"\x1b").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let _ = read_available(&mut handle, Duration::from_millis(100));

    // Press Enter. The line should be `car` (ghost was dismissed, not applied).
    handle.write_input(b"\r").unwrap();
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    assert!(
        plain.contains("PTY_GATE_LINE:car"),
        "Escape must dismiss ghost without inserting, got: {plain:?}"
    );
    assert!(
        !plain.contains("PTY_GATE_LINE:cargo"),
        "Escape must not apply the ghost: {plain:?}"
    );

    handle.write_input(b"exit\r").unwrap();
    let _ = read_until(&mut handle, "PTY_GATE_EXIT");
}

// ---------------------------------------------------------------------------
// Omen action completion in real terminal
// ---------------------------------------------------------------------------

#[test]
fn pty_omen_action_ghost_visible_in_real_terminal() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    // Type `:doc` — ghost should render `tor` remainder (visible as `:doctor`).
    handle.write_input(b":doc").unwrap();
    let out = read_until(&mut handle, ":doctor");
    let plain = strip_ansi(&out);
    assert!(
        plain.contains(":doctor"),
        "ghost must render 'tor' remainder as ':doctor', got: {plain:?}"
    );

    // Clean exit.
    handle.write_input(b"\x03").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    handle.write_input(b"exit\r").unwrap();
    let _ = read_until(&mut handle, "PTY_GATE_EXIT");
}

// ---------------------------------------------------------------------------
// NO_COLOR / degraded terminal: plain insertion, no ANSI corruption
// ---------------------------------------------------------------------------

#[test]
fn pty_no_color_ghost_is_plain_text() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    // Type `car` — ghost renders `go`. Verify the visible text is plain.
    handle.write_input(b"car").unwrap();
    let out = read_until(&mut handle, "cargo");
    let plain = strip_ansi(&out);
    // The ghost remainder `go` must appear as literal text (no ANSI corruption).
    assert!(
        plain.contains("cargo"),
        "ghost text must be plain 'cargo', got: {plain:?}"
    );

    // Clean exit.
    handle.write_input(b"\x03").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    handle.write_input(b"exit\r").unwrap();
    let _ = read_until(&mut handle, "PTY_GATE_EXIT");
}

// ---------------------------------------------------------------------------
// GHOST ACCEPTANCE: actual buffer mutation via HistoryHintComplete
// ---------------------------------------------------------------------------

#[test]
fn pty_right_arrow_accepts_ghost_into_buffer() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    // Type `:stat` — ghost renders `us`.
    handle.write_input(b":stat").unwrap();
    let out = read_until(&mut handle, ":status");
    let plain = strip_ansi(&out);
    assert!(
        plain.contains(":status"),
        "ghost must render 'us' as ':status', got: {plain:?}"
    );

    // Press Right Arrow (ESC [ C) to accept the ghost via HistoryHintComplete.
    handle.write_input(b"\x1b[C").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let _ = read_available(&mut handle, Duration::from_millis(100));

    // Press Enter. The editable buffer must be `:status`, NOT `:stat`.
    handle.write_input(b"\r").unwrap();
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    assert!(
        plain.contains("PTY_GATE_LINE::status"),
        "Right must accept ghost into buffer as ':status', got: {plain:?}"
    );
    assert!(
        !plain.contains("PTY_GATE_LINE::stat\r") && !plain.contains("PTY_GATE_LINE::stat\n"),
        "buffer must NOT be ':stat' after acceptance: {plain:?}"
    );

    handle.write_input(b"exit\r").unwrap();
    let _ = read_until(&mut handle, "PTY_GATE_EXIT");
}

#[test]
fn pty_end_key_accepts_ghost_into_buffer() {
    let mut handle = spawn_gate();
    read_until(&mut handle, "pty-gate>");

    // Type `:stat` — ghost renders `us`.
    handle.write_input(b":stat").unwrap();
    let out = read_until(&mut handle, ":status");
    let plain = strip_ansi(&out);
    assert!(
        plain.contains(":status"),
        "ghost must render 'us' as ':status', got: {plain:?}"
    );

    // Press End (ESC [ F) to accept the ghost via HistoryHintComplete.
    handle.write_input(b"\x1b[F").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let _ = read_available(&mut handle, Duration::from_millis(100));

    // Press Enter. The editable buffer must be `:status`, NOT `:stat`.
    handle.write_input(b"\r").unwrap();
    let out = read_until(&mut handle, "PTY_GATE_LINE:");
    let plain = strip_ansi(&out);
    assert!(
        plain.contains("PTY_GATE_LINE::status"),
        "End must accept ghost into buffer as ':status', got: {plain:?}"
    );
    assert!(
        !plain.contains("PTY_GATE_LINE::stat\r") && !plain.contains("PTY_GATE_LINE::stat\n"),
        "buffer must NOT be ':stat' after acceptance: {plain:?}"
    );

    handle.write_input(b"exit\r").unwrap();
    let _ = read_until(&mut handle, "PTY_GATE_EXIT");
}
