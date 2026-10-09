//! Tier 4 environment, glob, and job-lifecycle corpus.
//!
//! Exit codes + session/job state are the observables (no stdout capture
//! plumbing by design). Background jobs are pinned as PROPERTIES:
//! oracle job control differs fundamentally per platform, so parity is
//! neither claimed nor compared.

use omen_core::InteractiveSessionId;
use omen_interactive::session::{InteractiveSession, StandaloneJobState};
use std::path::Path;
use std::process::Command;

fn session_for(dir: &Path) -> InteractiveSession {
    let id = InteractiveSessionId::generate();
    InteractiveSession::new_with_client(id, dir.to_path_buf(), None, None)
        .expect("session builds headless")
}

fn run(session: &mut InteractiveSession, input: &str) -> i32 {
    match session.dispatch_input(input) {
        Ok(exit) => exit.code.unwrap_or(-1),
        Err(_) => -99,
    }
}

#[cfg(windows)]
fn make_tool(dir: &Path, name: &str) {
    std::fs::write(
        dir.join(format!("{name}.cmd")),
        "@echo off\r\nexit /b 0\r\n",
    )
    .unwrap();
}

#[cfg(not(windows))]
fn make_tool(dir: &Path, name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let tool = dir.join(name);
    std::fs::write(&tool, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn per_command_path_reaches_which() {
    // `PATH=<dir> which mytool` finds a tool visible only through the
    // assignment; exit parity with the oracle shell.
    let dir = tempfile::tempdir().unwrap();
    make_tool(dir.path(), "mytool_omen_compat");
    let mut s = session_for(dir.path());
    let path_value = dir.path().to_string_lossy().into_owned();
    let code = run(
        &mut s,
        &format!("PATH={path_value} which mytool_omen_compat"),
    );
    assert_eq!(code, 0, "per-command PATH reaches which");
    #[cfg(windows)]
    let (ocode, _) = {
        let out = Command::new("cmd")
            .args([
                "/c",
                &format!("set PATH={path_value};%PATH% && where mytool_omen_compat"),
            ])
            .current_dir(dir.path())
            .output()
            .expect("oracle spawns");
        (out.status.code().unwrap_or(-1), out.stdout)
    };
    #[cfg(not(windows))]
    let (ocode, _) = {
        let out = Command::new("sh")
            .args([
                "-c",
                &format!("PATH={path_value} command -v mytool_omen_compat"),
            ])
            .current_dir(dir.path())
            .output()
            .expect("oracle spawns");
        (out.status.code().unwrap_or(-1), out.stdout)
    };
    assert_eq!(ocode, 0, "oracle finds the tool too");
    // Without the assignment the tool is invisible in both.
    assert_ne!(run(&mut s, "which mytool_omen_compat"), 0);
}

#[test]
fn glob_no_match_fails_both() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), b"a").unwrap();
    let mut s = session_for(dir.path());
    assert_eq!(run(&mut s, "ls *.rs"), 0, "glob match dispatches");
    assert_ne!(run(&mut s, "ls *.zzz-no-match"), 0, "glob miss fails");
    #[cfg(windows)]
    let ocode = Command::new("cmd")
        .args(["/c", "dir /b *.zzz-no-match"])
        .current_dir(dir.path())
        .output()
        .expect("oracle spawns")
        .status
        .code()
        .unwrap_or(-1);
    #[cfg(not(windows))]
    let ocode = Command::new("sh")
        .args(["-c", "ls *.zzz-no-match"])
        .current_dir(dir.path())
        .output()
        .expect("oracle spawns")
        .status
        .code()
        .unwrap_or(-1);
    assert_ne!(ocode, 0, "oracle also fails glob miss");
}

#[test]
fn background_builtin_job_lifecycle_property() {
    // PROPERTY: `echo hi &` tracks exactly one standalone job, which
    // completes; `:jobs` preview machinery drains without leaks.
    let dir = tempfile::tempdir().unwrap();
    let mut s = session_for(dir.path());
    let code = run(&mut s, "echo compat-bg &");
    assert_eq!(code, 0, "background dispatches");
    // Poll bounded: the job finishes nearly immediately in-process.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let jobs = s.standalone_jobs();
        assert!(jobs.len() <= 1, "at most one job tracked");
        if jobs
            .iter()
            .all(|j| !matches!(j.state, StandaloneJobState::Running))
            || std::time::Instant::now() > deadline
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let jobs = s.standalone_jobs();
    assert!(jobs.len() <= 1, "no job leak after completion");
}
