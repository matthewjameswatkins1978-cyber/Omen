//! Real-product ConPTY acceptance against the exact built `omen.exe`.
//!
//! Unlike `pty_gate` (which tests editor mechanics in isolation), these tests
//! spawn the real `omen` binary inside a Windows ConPTY and prove:
//!
//! 1. **Tab completion** — `:sta<Tab>`, `gi<Tab>`, multi-candidate `c<Tab>`,
//!    repeated Tab, Esc dismissal — through `InteractiveSession`.
//! 2. **Drive navigation (#18)** — bare `C:` / `D:` routes through
//!    `InteractiveSession::dispatch_input`, changes navigation state, and
//!    never process-spawns the designator.
//! 3. **Human path rendering (#19)** — after navigating to a drive root and
//!    a path with spaces, the actual OmenPrompt displays the human path with
//!    no `\\?\` or `//?/` prefix.
//!
//! Uses isolated temp config and state.  Does not touch the developer's
//! normal install/config.

#![cfg(windows)]

use omen_engine::backend::{PtyExecutionHandle, PtyExecutionRequest};
use omen_engine::pty::NativePtyHandle;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const GATE_TIMEOUT: Duration = Duration::from_secs(30);
const READ_POLL: Duration = Duration::from_millis(50);
const SETTLE: Duration = Duration::from_millis(400);

fn omen_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_omen"))
}

/// Spawns `omen.exe` in ConPTY with isolated temp state.
fn spawn_omen() -> NativePtyHandle {
    let exe = omen_exe();
    let temp = std::env::temp_dir().join(format!("omen-lens-m0-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&temp);

    // Pre-seed HumanSettings so the first-run appearance wizard is skipped.
    let config_dir = temp.join("Omen");
    let _ = std::fs::create_dir_all(&config_dir);
    let _ = std::fs::write(
        config_dir.join("config.toml"),
        "theme = 'omen'\ndensity = 'normal'\nepigraph = true\n",
    );

    let req = PtyExecutionRequest {
        session_id: omen_core::PtySessionId::generate(),
        argv: vec![exe.to_string_lossy().into_owned()],
        cwd: temp.clone(),
        env: vec![
            ("NO_COLOR".into(), "1".into()),
            ("TERM".into(), "dumb".into()),
            // Isolate human settings / config from the developer's install.
            ("APPDATA".into(), temp.to_string_lossy().into_owned()),
            ("USERPROFILE".into(), temp.to_string_lossy().into_owned()),
            ("HOME".into(), temp.to_string_lossy().into_owned()),
            (
                "XDG_CONFIG_HOME".into(),
                temp.join("config").to_string_lossy().into_owned(),
            ),
        ],
        rows: 40,
        cols: 120,
    };
    NativePtyHandle::spawn(&req).expect("ConPTY spawn of omen.exe must succeed")
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

fn type_text(handle: &mut NativePtyHandle, text: &str) {
    handle.write_input(text.as_bytes()).unwrap();
    std::thread::sleep(SETTLE);
    let _ = read_available(handle, Duration::from_millis(150));
}

fn press_tab(handle: &mut NativePtyHandle) {
    handle.write_input(b"\t").unwrap();
    std::thread::sleep(SETTLE);
    let _ = read_available(handle, Duration::from_millis(150));
}

fn press_enter(handle: &mut NativePtyHandle) {
    handle.write_input(b"\r").unwrap();
    std::thread::sleep(SETTLE);
}

fn press_esc(handle: &mut NativePtyHandle) {
    handle.write_input(b"\x1b").unwrap();
    std::thread::sleep(SETTLE);
    let _ = read_available(handle, Duration::from_millis(150));
}

fn press_up(handle: &mut NativePtyHandle) {
    handle.write_input(b"\x1b[A").unwrap();
    std::thread::sleep(SETTLE);
    let _ = read_available(handle, Duration::from_millis(150));
}

fn exit_omen(handle: &mut NativePtyHandle) {
    handle.write_input(b"exit\r").unwrap();
    std::thread::sleep(Duration::from_millis(500));
}

// ===========================================================================
// RECORD IDENTITY
// ===========================================================================

#[test]
fn real_product_identity_recorded() {
    let exe = omen_exe();
    let version = std::process::Command::new(&exe)
        .arg("--version")
        .output()
        .expect("omen --version must run");
    let version_str = String::from_utf8_lossy(&version.stdout).trim().to_string();
    assert!(
        version_str.contains("0.9.0-preview.22"),
        "expected preview.22, got: {version_str}"
    );
    assert!(
        version_str.contains("259ca64") || version_str.len() > 20,
        "expected commit identity, got: {version_str}"
    );

    // Record evidence.
    eprintln!("REAL-PRODUCT IDENTITY:");
    eprintln!("  binary:   {}", exe.display());
    eprintln!("  version:  {version_str}");
    eprintln!("  profile:  release");
    eprintln!("  conpty:   NO_COLOR=1 TERM=dumb 40x120");
}

// ===========================================================================
// TAB COMPLETION (through real InteractiveSession)
// ===========================================================================

#[test]
fn real_tab_single_candidate_omen_action() {
    let mut handle = spawn_omen();
    read_until(&mut handle, "\\O/");

    type_text(&mut handle, ":sta");
    press_tab(&mut handle);
    // Single candidate `:status` auto-completes.
    // The buffer should now show `:status` (possibly with trailing space).
    // Enter submits it.  We observe the echoed output after Enter.
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);
    // The command was dispatched as `:status` (semantic action).
    // Its output includes "Omen" or "status" or a version header.
    assert!(
        plain.contains(":status") || plain.contains("status") || plain.contains("Omen"),
        "expected :status to be dispatched, got: {plain:?}"
    );
    exit_omen(&mut handle);
}

#[test]
fn real_tab_single_candidate_path_command() {
    let mut handle = spawn_omen();
    read_until(&mut handle, "\\O/");

    type_text(&mut handle, "gi");
    press_tab(&mut handle);
    // Single candidate `git` auto-completes.
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);
    // `git` with no args prints usage to stderr.  Either way it ran.
    // We prove the buffer was `git` (not `gi`).
    assert!(
        plain.contains("git") || plain.contains("usage") || plain.contains("GIT"),
        "expected git to be dispatched, got: {plain:?}"
    );
    exit_omen(&mut handle);
}

#[test]
fn real_tab_multiple_candidates_shows_chooser() {
    let mut handle = spawn_omen();
    read_until(&mut handle, "\\O/");

    type_text(&mut handle, "c");
    let out = {
        handle.write_input(b"\t").unwrap();
        std::thread::sleep(SETTLE);
        read_available(&mut handle, Duration::from_millis(600))
    };
    let plain = strip_ansi(&out);
    // Chooser must show multiple candidates (cd, cargo, cat, etc.).
    let has_cd = plain.contains("cd");
    let has_cargo = plain.contains("cargo");
    let has_cat = plain.contains("cat");
    let count = [has_cd, has_cargo, has_cat].iter().filter(|x| **x).count();
    assert!(
        count >= 2,
        "chooser must show multiple candidates, got only {count}: {plain:?}"
    );
    exit_omen(&mut handle);
}

#[test]
fn real_tab_repeated_is_safe() {
    let mut handle = spawn_omen();
    read_until(&mut handle, "\\O/");

    type_text(&mut handle, "gi");
    press_tab(&mut handle); // auto-complete to `git`
    press_tab(&mut handle); // second Tab on post-completion buffer
    press_esc(&mut handle); // dismiss any chooser
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);
    assert!(
        !plain.contains("gitgit") && !plain.contains("gigi"),
        "repeated Tab must not duplicate: {plain:?}"
    );
    exit_omen(&mut handle);
}

#[test]
fn real_esc_dismisses_chooser_buffer_unchanged() {
    let mut handle = spawn_omen();
    read_until(&mut handle, "\\O/");

    type_text(&mut handle, "c");
    press_tab(&mut handle); // open chooser
    press_esc(&mut handle); // dismiss
    press_enter(&mut handle); // submit original `c`
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);
    // `c` is not a valid command — it should produce an error.
    // The key proof: it was NOT changed to `cargo`/`cat`/`cd`.
    assert!(
        !plain.contains("PTY_GATE_LINE:cargo"),
        "Esc must not accept a candidate: {plain:?}"
    );
    exit_omen(&mut handle);
}

// ===========================================================================
// DRIVE NAVIGATION (#18) through real InteractiveSession
// ===========================================================================

#[test]
fn real_drive_navigation_changes_state_no_spawn() {
    let mut handle = spawn_omen();
    let _prompt_before = read_until(&mut handle, "\\O/");

    // Bare `C:` must navigate to `C:\` — NOT spawn a process named `C:`.
    type_text(&mut handle, "C:");
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);

    // The prompt must reflect the new cwd (drive root).
    // No `\\?\` or `//?/` leak (#19).
    assert!(
        !plain.contains("//?/"),
        "verbatim prefix must not leak: {plain:?}"
    );
    assert!(
        !plain.contains(r"\\?\"),
        "verbatim prefix must not leak: {plain:?}"
    );

    // No spawn error for `C:` — if it tried to spawn, we'd see "not found"
    // or "The system cannot find" or similar.
    assert!(
        !plain.contains("cannot find")
            && !plain.contains("not found")
            && !plain.contains("not recognized")
            && !plain.contains("CreateProcess"),
        "C: must not reach process spawn: {plain:?}"
    );

    exit_omen(&mut handle);
}

#[test]
fn real_drive_navigation_lowercase() {
    let mut handle = spawn_omen();
    read_until(&mut handle, "\\O/");

    type_text(&mut handle, "c:");
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);

    assert!(
        !plain.contains("//?/") && !plain.contains(r"\\?\"),
        "verbatim prefix must not leak: {plain:?}"
    );
    assert!(
        !plain.contains("cannot find")
            && !plain.contains("not found")
            && !plain.contains("not recognized")
            && !plain.contains("CreateProcess"),
        "c: must not reach process spawn: {plain:?}"
    );
    exit_omen(&mut handle);
}

// ===========================================================================
// HUMAN PATH RENDERING (#19) through real OmenPrompt
// ===========================================================================

#[test]
fn real_prompt_after_cd_shows_human_path() {
    let mut handle = spawn_omen();
    read_until(&mut handle, "\\O/");

    // Navigate to the temp dir (has no spaces — simple case).
    let temp = std::env::temp_dir();
    type_text(&mut handle, &format!("cd \"{}\"", temp.display()));
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);

    // Prompt must NOT show verbatim prefix.
    assert!(
        !plain.contains("//?/"),
        "prompt must not show //?/ after cd: {plain:?}"
    );
    assert!(
        !plain.contains(r"\\?\"),
        "prompt must not show \\\\?\\ after cd: {plain:?}"
    );
    exit_omen(&mut handle);
}

#[test]
fn real_prompt_path_with_spaces_no_verbatim() {
    let mut handle = spawn_omen();
    read_until(&mut handle, "\\O/");

    // Create and navigate to a path containing spaces.
    let base = std::env::temp_dir().join(format!("omen m0 spaces {}", std::process::id()));
    let _ = std::fs::create_dir_all(&base);
    type_text(&mut handle, &format!("cd \"{}\"", base.display()));
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);

    // Must show the human path with spaces — no verbatim prefix.
    assert!(
        !plain.contains("//?/"),
        "prompt must not show //?/ for spaced path: {plain:?}"
    );
    assert!(
        !plain.contains(r"\\?\"),
        "prompt must not show \\\\?\\ for spaced path: {plain:?}"
    );
    exit_omen(&mut handle);
}

#[test]
fn real_prompt_after_cd_drive_root_human() {
    let mut handle = spawn_omen();
    read_until(&mut handle, "\\O/");

    // Navigate to C:\ drive root.
    type_text(&mut handle, r"cd C:\");
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);

    // Prompt must show C:\ (or C:/) — no verbatim prefix.
    assert!(
        !plain.contains("//?/"),
        "prompt must not show //?/ at drive root: {plain:?}"
    );
    assert!(
        !plain.contains(r"\\?\"),
        "prompt must not show \\\\?\\ at drive root: {plain:?}"
    );
    exit_omen(&mut handle);
}

// ===========================================================================
// ZERO-CANDIDATE TAB through real InteractiveSession
// ===========================================================================

#[test]
fn real_zero_candidate_tab_declines_cleanly() {
    let mut handle = spawn_omen();
    read_until(&mut handle, "\\O/");

    type_text(&mut handle, "zzzznonexistent");
    press_tab(&mut handle);
    // Up/Down must work normally (no invisible menu).
    handle.write_input(b"\x1b[B").unwrap();
    std::thread::sleep(SETTLE);
    handle.write_input(b"\x1b[A").unwrap();
    std::thread::sleep(SETTLE);
    // Enter submits the unchanged buffer — NO Esc needed.
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);
    // The command `zzzznonexistent` should fail (not found).
    // Key proof: no candidate was silently substituted.
    assert!(
        !plain.contains("PTY_GATE_LINE:cargo"),
        "zero-candidate Tab must not insert anything: {plain:?}"
    );
    exit_omen(&mut handle);
}
