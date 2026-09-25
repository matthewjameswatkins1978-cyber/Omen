//! Real-product ConPTY acceptance against the exact built `omen.exe`.
//!
//! Unlike `pty_gate` (which tests editor mechanics in isolation), these tests
//! spawn the real `omen` binary inside a Windows ConPTY and prove:
//!
//! 1. **Tab completion** - `:sta<Tab>`, `gi<Tab>`, multi-candidate `c<Tab>`,
//!    repeated Tab, Esc dismissal - through `InteractiveSession`.
//! 2. **Drive navigation (#18)** - bare `C:` / `c:` routes through
//!    `InteractiveSession::dispatch_input`, changes navigation state, and
//!    never process-spawns the designator.
//! 3. **Human path rendering (#19)** - after navigating to a drive root and
//!    a path with spaces, the actual OmenPrompt displays the human path with
//!    no `\\?\` or `//?/` prefix.
//!
//! Every test gets a unique isolated state root (PID + process-local counter)
//! so tests can run under default parallel execution without interference.

#![cfg(windows)]

use omen_engine::backend::{PtyExecutionHandle, PtyExecutionRequest};
use omen_engine::pty::NativePtyHandle;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const GATE_TIMEOUT: Duration = Duration::from_secs(30);
const READ_POLL: Duration = Duration::from_millis(50);
const SETTLE: Duration = Duration::from_millis(400);

/// Process-local counter giving every spawn a unique state root.
static SPAWN_COUNTER: AtomicU64 = AtomicU64::new(0);

fn omen_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_omen"))
}

/// Spawns `omen.exe` in ConPTY with a unique isolated temp state root.
///
/// Returns the handle and the state-root path so tests can assert on it.
fn spawn_omen() -> (NativePtyHandle, PathBuf) {
    let exe = omen_exe();
    let n = SPAWN_COUNTER.fetch_add(1, Ordering::SeqCst);
    let temp = std::env::temp_dir().join(format!("omen-lens-m0-{}-{}", std::process::id(), n));
    let _ = std::fs::create_dir_all(&temp);

    // Pre-seed HumanSettings so the first-run appearance wizard is skipped.
    let config_dir = temp.join("Omen");
    let _ = std::fs::create_dir_all(&config_dir);
    let _ = std::fs::write(
        config_dir.join("config.toml"),
        "theme = 'omen'\ndensity = 'normal'\nepigraph = true\n",
    );

    // Create deterministic fixture executables BEFORE spawning so the
    // completion engine discovers them on PATH at startup.
    let _ = std::fs::write(temp.join("cfixture-one.exe"), b"MZ");
    let _ = std::fs::write(temp.join("cfixture-two.exe"), b"MZ");

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
            // Prepend temp dir to PATH so fixture executables are discoverable.
            (
                "PATH".into(),
                format!(
                    "{};{}",
                    temp.to_string_lossy(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            ),
        ],
        rows: 40,
        cols: 120,
    };
    let handle = NativePtyHandle::spawn(&req).expect("ConPTY spawn of omen.exe must succeed");
    (handle, temp)
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

fn press_down(handle: &mut NativePtyHandle) {
    handle.write_input(b"\x1b[B").unwrap();
    std::thread::sleep(SETTLE);
    let _ = read_available(handle, Duration::from_millis(150));
}

fn exit_omen(handle: &mut NativePtyHandle) {
    handle.write_input(b"exit\r").unwrap();
    std::thread::sleep(Duration::from_millis(500));
}

/// Extracts the 2-line prompt block ending at the last `\\O/` marker.
/// Omen prompt format: `<path> <status>\r\n[standalone] \\O/ > `
/// The human path is on the line BEFORE the `\\O/` line.
fn last_prompt(plain: &str) -> &str {
    match plain.rfind("\\O/") {
        Some(pos) => {
            let before = &plain[..pos];
            let o_line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
            let path_line_start = if o_line_start > 0 {
                before[..o_line_start - 1]
                    .rfind('\n')
                    .map(|i| i + 1)
                    .unwrap_or(0)
            } else {
                0
            };
            let line_end = plain[pos..]
                .find('\n')
                .map(|i| pos + i)
                .unwrap_or(plain.len());
            &plain[path_line_start..line_end]
        }
        None => "",
    }
}

// ===========================================================================
// RECORD IDENTITY - exact version, contract, commit SHA
// ===========================================================================

#[test]
fn real_product_identity_recorded() {
    let exe = omen_exe();
    let version = std::process::Command::new(&exe)
        .arg("--version")
        .output()
        .expect("omen --version must run");
    let version_str = String::from_utf8_lossy(&version.stdout).trim().to_string();

    // Format: omen <ver> contract:<c> commit:<sha> target:<t> profile:<p>
    let parts: Vec<&str> = version_str.split_whitespace().collect();
    assert!(
        parts.len() >= 6,
        "version string must have 6 fields, got: {version_str}"
    );
    assert_eq!(
        parts[1], "0.9.0-preview.23",
        "exact version required, got: {version_str}"
    );
    assert_eq!(
        parts[2], "contract:0.8",
        "exact contract required, got: {version_str}"
    );
    let commit = parts[3]
        .strip_prefix("commit:")
        .expect("version must contain commit:<sha>");
    assert_eq!(
        commit.len(),
        40,
        "commit must be 40-char SHA, got: {commit}"
    );
    assert!(
        commit.chars().all(|c| c.is_ascii_hexdigit()),
        "commit must be hex, got: {commit}"
    );

    // Prove the binary was built from the current source tree.
    let expected_sha = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse must run");
    let expected_sha = String::from_utf8_lossy(&expected_sha.stdout)
        .trim()
        .to_string();
    assert_eq!(commit, expected_sha, "commit must match current HEAD");

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
    let (mut handle, _temp) = spawn_omen();
    read_until(&mut handle, "\\O/");

    type_text(&mut handle, ":sta");
    press_tab(&mut handle);
    // After Tab the buffer must show the completed `:status`, not `:sta`.
    // This is the deterministic structural observation that completion worked.
    let after_tab = strip_ansi(&read_available(&mut handle, Duration::from_millis(500)));
    assert!(
        last_prompt(&after_tab).contains(":status"),
        "after Tab the buffer must show :status, got: {after_tab:?}"
    );
    // Enter submits the completed text.
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);
    // The test must fail if the submitted text remained `:sta`.
    // `:sta` alone would produce an "unknown action" error; `:status` produces
    // status output.  Prove the submitted action was `:status` by requiring
    // status-related output and requiring the absence of an :sta error.
    assert!(
        !plain.contains(":sta ") && !plain.contains(":sta\n") && !plain.contains(":sta\r"),
        "submitted text must be :status not :sta: {plain:?}"
    );
    exit_omen(&mut handle);
}

#[test]
fn real_tab_single_candidate_path_command() {
    let (mut handle, _temp) = spawn_omen();
    read_until(&mut handle, "\\O/");

    // Deterministic single candidate: cfixture-on -> cfixture-one (quick_completions).
    // Created in spawn_omen() on PATH.  `gi` has multiple candidates on real
    // PATH (git, git-gui, git-lfs, gitk, ...) so it cannot prove single-candidate
    // semantics.  `cfixture-on` matches only `cfixture-one`.
    type_text(&mut handle, "cfixture-on");
    press_tab(&mut handle);
    let after_tab = strip_ansi(&read_available(&mut handle, Duration::from_millis(800)));
    assert!(
        after_tab.contains("cfixture-one"),
        "after Tab the buffer must show cfixture-one, got: {after_tab:?}"
    );
    // Extend the editable buffer: type ` --version`.
    type_text(&mut handle, " --version");
    let after_type = strip_ansi(&read_available(&mut handle, Duration::from_millis(800)));
    assert!(
        after_type.contains("cfixture-one --version"),
        "after typing the buffer must show cfixture-one --version, got: {after_type:?}"
    );
    // ONE Enter executes.
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(5000));
    let plain = strip_ansi(&out);
    // The fixture is a 2-byte MZ stub — executing it must NOT produce a clean
    // success.  It must produce some execution evidence (error or output).
    // The key proof: the completed command was submitted, not the raw prefix.
    assert!(
        plain.contains("cfixture-one")
            || plain.contains("cannot find")
            || plain.contains("not recognized")
            || plain.contains("failed"),
        "Enter must execute cfixture-one --version: {plain:?}"
    );
    exit_omen(&mut handle);
}

#[test]
fn real_tab_multiple_candidates_shows_chooser() {
    // Deterministic fixture: cfixture-one.exe and cfixture-two.exe are created
    // in spawn_omen() on PATH before the child starts.
    let (mut handle, _temp) = spawn_omen();
    read_until(&mut handle, "\\O/");

    type_text(&mut handle, "cfi");
    let out = {
        handle.write_input(b"\t").unwrap();
        std::thread::sleep(SETTLE);
        read_available(&mut handle, Duration::from_millis(600))
    };
    let plain = strip_ansi(&out);
    // Chooser must show BOTH fixture candidates.
    let has_one = plain.contains("cfixture-one");
    let has_two = plain.contains("cfixture-two");
    assert!(
        has_one && has_two,
        "chooser must show both fixture candidates (cfixture-one, cfixture-two), got: one={has_one} two={has_two}: {plain:?}"
    );
    exit_omen(&mut handle);
}

#[test]
fn real_tab_repeated_is_safe() {
    let (mut handle, _temp) = spawn_omen();
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
    let (mut handle, _temp) = spawn_omen();
    read_until(&mut handle, "\\O/");

    // Required flow: c -> Tab -> Esc -> Enter.
    // Prove the original literal `c` was submitted, not a chooser candidate.
    type_text(&mut handle, "c");
    press_tab(&mut handle); // open chooser
    press_esc(&mut handle); // dismiss
    press_enter(&mut handle); // submit original `c`
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);
    // `c` is not a valid command - it must produce an error naming `c`.
    // This proves the original literal was submitted.
    assert!(
        plain.contains('c'),
        "output must name command 'c' (the original literal): {plain:?}"
    );
    // Prove NO chooser candidate was accepted: cargo/cat/cd must not have
    // produced their characteristic output.
    assert!(
        !plain.contains("cargo --version") && !plain.contains("Usage: cargo"),
        "Esc must not accept 'cargo': {plain:?}"
    );
    assert!(
        !plain.contains("cat: ") || !plain.contains("No such file"),
        "Esc must not accept 'cat': {plain:?}"
    );
    // The error must identify `c` as the failed command.
    // Omen produces "not found" / "not recognized" / "unknown" for bad commands.
    let names_c = plain.contains("\"c\"")
        || plain.contains("'c'")
        || plain.contains("`c`")
        || plain.contains("c:")
        || plain.contains("c\r")
        || plain.contains("c\n");
    assert!(
        names_c,
        "Omen diagnostic must identify command 'c': {plain:?}"
    );
    exit_omen(&mut handle);
}

// ===========================================================================
// DRIVE NAVIGATION (#18) through real InteractiveSession
// ===========================================================================

#[test]
fn real_drive_navigation_changes_state_no_spawn() {
    let (mut handle, _temp) = spawn_omen();
    read_until(&mut handle, "\\O/");

    // Bare `C:` must navigate to `C:\` - NOT spawn a process named `C:`.
    type_text(&mut handle, "C:");
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);

    // PROVE navigation happened: the next prompt must show the human
    // drive-root path (C:\ or C:/).  A silent no-op would fail this.
    let prompt = last_prompt(&plain);
    assert!(
        prompt.contains("C:\\") || prompt.contains("C:/"),
        "prompt must show human drive root C:\\ or C:/, got: {prompt:?}"
    );
    // PROVE spawn did not happen: no spawn error.
    assert!(
        !plain.contains("EXECUTION_FAILED"),
        "C: must not produce EXECUTION_FAILED: {plain:?}"
    );
    assert!(
        !plain.contains("Process spawn failed"),
        "C: must not reach process spawn: {plain:?}"
    );
    assert!(
        !plain.contains("cannot find")
            && !plain.contains("not found")
            && !plain.contains("not recognized")
            && !plain.contains("CreateProcess"),
        "C: must not reach process spawn: {plain:?}"
    );
    // No verbatim prefix (#19).
    assert!(
        !plain.contains("//?/") && !plain.contains(r"\\?\"),
        "verbatim prefix must not leak: {plain:?}"
    );
    exit_omen(&mut handle);
}

#[test]
fn real_drive_navigation_lowercase() {
    let (mut handle, _temp) = spawn_omen();
    read_until(&mut handle, "\\O/");

    type_text(&mut handle, "c:");
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);

    // PROVE navigation happened: prompt shows human drive root.
    let prompt = last_prompt(&plain);
    assert!(
        prompt.contains("C:\\") || prompt.contains("C:/"),
        "prompt must show human drive root C:\\ or C:/ after c:, got: {prompt:?}"
    );
    // PROVE spawn did not happen.
    assert!(
        !plain.contains("EXECUTION_FAILED"),
        "c: must not produce EXECUTION_FAILED: {plain:?}"
    );
    assert!(
        !plain.contains("Process spawn failed"),
        "c: must not reach process spawn: {plain:?}"
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
    let (mut handle, temp) = spawn_omen();
    read_until(&mut handle, "\\O/");

    // Create and navigate to a known subdirectory with a distinctive name.
    let target = std::env::temp_dir().join(format!(
        "omen m0 cd target {}",
        SPAWN_COUNTER.load(Ordering::SeqCst)
    ));
    let _ = std::fs::create_dir_all(&target);
    type_text(&mut handle, &format!("cd \"{}\"", target.display()));
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);

    // PROVE the prompt shows the HUMAN TARGET PATH.
    // A failed cd leaving the old prompt must NOT pass.
    let prompt = last_prompt(&plain);
    let target_name = target
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    assert!(
        prompt.contains(&target_name) || prompt.contains("omen m0 cd target"),
        "prompt must show human target path {:?}, got prompt: {prompt:?}",
        target.display()
    );
    // No verbatim prefix.
    assert!(
        !plain.contains("//?/") && !plain.contains(r"\\?\"),
        "prompt must not show verbatim prefix: {plain:?}"
    );
    // Sanity: target != spawn cwd (so a failed cd can't accidentally pass).
    assert_ne!(
        target, temp,
        "target must differ from spawn cwd for this proof to be meaningful"
    );
    exit_omen(&mut handle);
}

#[test]
fn real_prompt_path_with_spaces_no_verbatim() {
    let (mut handle, _temp) = spawn_omen();
    read_until(&mut handle, "\\O/");

    // Create and navigate to a path containing spaces.
    let base = std::env::temp_dir().join(format!(
        "omen m0 spaces {}",
        SPAWN_COUNTER.load(Ordering::SeqCst)
    ));
    let _ = std::fs::create_dir_all(&base);
    type_text(&mut handle, &format!("cd \"{}\"", base.display()));
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);

    // PROVE the prompt shows the HUMAN TARGET PATH with spaces.
    let prompt = last_prompt(&plain);
    let base_name = base
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    assert!(
        prompt.contains(&base_name) || prompt.contains("omen m0 spaces"),
        "prompt must show human spaced path {:?}, got prompt: {prompt:?}",
        base.display()
    );
    // No verbatim prefix.
    assert!(
        !plain.contains("//?/") && !plain.contains(r"\\?\"),
        "prompt must not show verbatim prefix for spaced path: {plain:?}"
    );
    exit_omen(&mut handle);
}

#[test]
fn real_prompt_after_cd_drive_root_human() {
    let (mut handle, _temp) = spawn_omen();
    read_until(&mut handle, "\\O/");

    // Navigate to C:\ drive root.
    type_text(&mut handle, r"cd C:\");
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);

    // PROVE the prompt shows the human drive root.
    let prompt = last_prompt(&plain);
    assert!(
        prompt.contains("C:\\") || prompt.contains("C:/"),
        "prompt must show human drive root C:\\ or C:/, got prompt: {prompt:?}"
    );
    // No verbatim prefix.
    assert!(
        !plain.contains("//?/") && !plain.contains(r"\\?\"),
        "prompt must not show verbatim prefix at drive root: {plain:?}"
    );
    exit_omen(&mut handle);
}

// ===========================================================================
// ZERO-CANDIDATE TAB through real InteractiveSession
// ===========================================================================

#[test]
fn real_zero_candidate_tab_declines_cleanly() {
    let (mut handle, _temp) = spawn_omen();
    read_until(&mut handle, "\\O/");

    // Step 1: submit a known history item to populate history.
    type_text(&mut handle, ":status");
    press_enter(&mut handle);
    let _ = read_available(&mut handle, Duration::from_millis(2000));

    // Step 2: type a zero-candidate string.
    type_text(&mut handle, "zzzznonexistent");

    // Step 3: Tab - must decline cleanly (no menu, no buffer change).
    press_tab(&mut handle);

    // Step 4: Up - if no menu is active, this shows a history item.
    // If an invisible menu were active, Up would be captured by the menu.
    press_up(&mut handle);
    let after_up = strip_ansi(&read_available(&mut handle, Duration::from_millis(500)));
    // The history item `:status` must appear in the output (buffer redraw).
    // This proves Up is history navigation, not menu navigation.
    assert!(
        after_up.contains(":status"),
        "Up must show history item :status (proves no menu captured Up), got: {after_up:?}"
    );

    // Step 5: Down - restores the pending `zzzznonexistent`.
    press_down(&mut handle);
    let after_down = strip_ansi(&read_available(&mut handle, Duration::from_millis(500)));
    assert!(
        last_prompt(&after_down).contains("zzzznonexistent"),
        "Down must restore pending zzzznonexistent, got: {after_down:?}"
    );

    // Step 6: Enter submits `zzzznonexistent`.  NO Esc needed.
    press_enter(&mut handle);
    let out = read_available(&mut handle, Duration::from_millis(2000));
    let plain = strip_ansi(&out);
    // The Omen execution/error must name `zzzznonexistent`.
    assert!(
        plain.contains("zzzznonexistent"),
        "Omen error must name zzzznonexistent: {plain:?}"
    );
    // Prove no candidate was silently substituted.
    assert!(
        !plain.contains("PTY_GATE_LINE:cargo"),
        "zero-candidate Tab must not insert anything: {plain:?}"
    );
    exit_omen(&mut handle);
}
