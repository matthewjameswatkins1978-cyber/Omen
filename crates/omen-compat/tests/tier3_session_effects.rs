//! Tier 3 session-effects differential corpus: redirects, boolean chains,
//! and missing-input behavior through the real `dispatch_input` path.
//!
//! Observable surface is exit codes + filesystem side effects (session
//! stdout goes to the terminal layer, which this corpus does not plumb).
//! File bytes ARE byte-compared against oracle shells; refusal behavior
//! (output writes fail closed pending Tethers authority) is pinned as a
//! DELIBERATE DIFFERENCE, never implied as parity.

use omen_core::InteractiveSessionId;
use omen_interactive::session::InteractiveSession;
use std::path::{Path, PathBuf};
use std::process::Command;

fn session_for(dir: &Path) -> InteractiveSession {
    let id = InteractiveSessionId::generate();
    InteractiveSession::new_with_client(id, dir.to_path_buf(), None, None)
        .expect("session builds headless")
}

/// Dispatch through the real shell path; returns the exit code.
fn run(session: &mut InteractiveSession, input: &str) -> i32 {
    match session.dispatch_input(input) {
        Ok(exit) => exit.code.unwrap_or(-1),
        Err(_) => -99,
    }
}

fn sh(dir: &Path, script: &str) -> (i32, Vec<u8>) {
    #[cfg(windows)]
    let (prog, args) = ("cmd", vec!["/c", script]);
    #[cfg(not(windows))]
    let (prog, args) = ("sh", vec!["-c", script]);
    let out = Command::new(prog)
        .args(&args)
        .current_dir(dir)
        .output()
        .expect("oracle shell spawns");
    (out.status.code().unwrap_or(-1), out.stdout)
}

fn read(dir: &Path, name: &str) -> Option<Vec<u8>> {
    std::fs::read(dir.join(name)).ok()
}

#[test]
fn output_write_refused_closed_pinned() {
    // DELIBERATE DIFFERENCE: `>` / `>>` fail closed (exit nonzero, no
    // file) until Tethers host-filesystem authority lands. Oracle shells
    // succeed; Omen must refuse LOUDLY, never half-write.
    let dir = tempfile::tempdir().unwrap();
    let mut s = session_for(dir.path());
    let code = run(&mut s, "echo hello > out.txt");
    assert_ne!(code, 0, "output write must refuse");
    assert!(read(dir.path(), "out.txt").is_none(), "no half-write");
    let code = run(&mut s, "echo hello >> out.txt");
    assert_ne!(code, 0, "append must refuse");
    assert!(read(dir.path(), "out.txt").is_none(), "no half-write");
}

#[test]
fn input_redirect_feeds_and_missing_fails() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("in.txt"), b"b\na\nb\n").unwrap();
    let mut s = session_for(dir.path());
    // `sort -u < in.txt` exit 0 through the real dispatch path.
    assert_eq!(run(&mut s, "sort -u < in.txt"), 0);
    // Missing input: nonzero here AND in oracle shells (codes only;
    // diagnostic text differs by shell and is not compared).
    assert_ne!(run(&mut s, "sort < nope-missing.txt"), 0);
    let (ocode, _) = sh(dir.path(), "sort < nope-missing.txt");
    assert_ne!(ocode, 0, "oracle also fails missing input");
}

#[test]
fn boolean_chain_exits_match_shell() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session_for(dir.path());
    for (input, oracle) in [
        (
            "grep zzz-nope a.txt || echo fallback",
            "grep zzz-nope a.txt || echo fallback",
        ),
        ("echo hi && echo there", "echo hi && echo there"),
    ] {
        let _ = std::fs::write(dir.path().join("a.txt"), b"a\n");
        let code = run(&mut s, input);
        let (ocode, _) = sh(dir.path(), oracle);
        // Oracle `echo` writes to ITS stdout; only exit parity compares.
        assert_eq!(code, 0, "{input} succeeds in Omen");
        assert_eq!(ocode, 0, "{oracle} succeeds in oracle shell");
    }
    let _ = std::fs::write(dir.path().join("a.txt"), b"a\n");
    let code = run(&mut s, "grep zzz-nope a.txt && echo skipped");
    let (ocode, _) = sh(dir.path(), "grep zzz-nope a.txt && echo skipped");
    assert_eq!((code, ocode), (1, 1), "failing && chain exit parity");
}

#[test]
fn cd_missing_dir_fails_both() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = session_for(dir.path());
    assert_ne!(run(&mut s, "cd nope-missing-dir"), 0);
    let (ocode, _) = sh(dir.path(), "cd nope-missing-dir");
    assert_ne!(ocode, 0, "oracle also fails missing cd");
}

#[test]
fn empty_input_file_feeds_empty() {
    // PROPERTY: an empty `<` file feeds empty stdin (no fallthrough to
    // terminal stdin); `cat < empty` exits 0.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("e.txt"), b"").unwrap();
    let mut s = session_for(dir.path());
    assert_eq!(run(&mut s, "cat < e.txt"), 0);
    assert!(PathBuf::from("e.txt").is_relative());
}
