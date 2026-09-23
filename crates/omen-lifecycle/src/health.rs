//! Bounded candidate interrogation (Lucy repair B2, hardened Preview 18).
//!
//! The staged candidate is asked exactly one question (`--version`) under
//! a hard deadline with bounded output capture. A corrupt, hostile, or
//! merely broken candidate can never hang `omen update`. Every phase is
//! bounded:
//! - execution: [`HEALTH_DEADLINE`] (15 s);
//! - cleanup: [`CLEANUP_BUDGET`] (3 s) covering primary-child kill,
//!   supervised Windows tree-kill, and the reap poll.
//!
//! Cleanup outcomes are classified, never disguised:
//! [`CleanupState::TerminatedAndReaped`] vs
//! [`CleanupState::CleanupIncomplete`]. A cleanup failure is a HEALTH
//! FAILURE — activation stays forbidden, the previous slot is preserved,
//! and the diagnostic says cleanup could not be confirmed. Unknown stays
//! unknown. No unbounded worker threads: all waiters poll `try_wait`.

use crate::error::LifecycleError;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Health deadline: private by Lucy decision (10–15s suggested).
const HEALTH_DEADLINE: Duration = Duration::from_secs(15);
/// Cleanup budget: kill + supervised tree-kill + reap must finish inside
/// this, or cleanup is reported incomplete (never an infinite wait).
pub const CLEANUP_BUDGET: Duration = Duration::from_secs(3);
/// Per-stream capture cap (1 MiB). Floods are truncated, never OOM.
const CAPTURE_CAP: u64 = 1024 * 1024;
/// Poll granularity for the waiters.
const POLL: Duration = Duration::from_millis(50);

/// What cleanup achieved. Internal truth, surfaced in diagnostics —
/// NEVER collapsed into "health succeeded".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupState {
    /// Candidate exited on its own; no kill needed.
    NotNeeded,
    /// Deadline fired, kill confirmed, child reaped. No zombie, no linger.
    TerminatedAndReaped,
    /// Budget expired (or helper failed) before termination could be
    /// confirmed. Carries the reason. Activation is forbidden.
    CleanupIncomplete(String),
}

/// Outcome of one bounded interrogation.
#[derive(Debug)]
pub struct ProbeOutcome {
    pub timed_out: bool,
    pub exit_ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
    pub cleanup: CleanupState,
}

/// Interrogate `binary --version` with an explicit deadline. Closed stdin,
/// bounded captures, bounded kill + supervised tree-kill + bounded reap
/// on timeout.
pub fn probe_candidate(binary: &Path, deadline: Duration) -> Result<ProbeOutcome, LifecycleError> {
    probe_candidate_with_budget(binary, deadline, CLEANUP_BUDGET)
}

/// Interrogate with an explicit cleanup budget (tests use small budgets;
/// production uses [`CLEANUP_BUDGET`]).
pub fn probe_candidate_with_budget(
    binary: &Path,
    deadline: Duration,
    cleanup_budget: Duration,
) -> Result<ProbeOutcome, LifecycleError> {
    let mut child = std::process::Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| LifecycleError::Health(format!("candidate would not spawn: {e}")))?;

    let start = Instant::now();
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) => {
                if start.elapsed() >= deadline {
                    break true;
                }
                std::thread::sleep(POLL);
            }
            Err(e) => {
                return Err(LifecycleError::Health(format!(
                    "candidate wait failed: {e}"
                )));
            }
        }
    };

    let cleanup = if timed_out {
        terminate_bounded(&mut child, cleanup_budget)
    } else {
        CleanupState::NotNeeded
    };

    let exit_ok = child
        .try_wait()
        .map(|s| s.map(|s| s.success()).unwrap_or(false))
        .unwrap_or(false);
    // Drain ONLY on the clean-exit path. On timeout the pipes are
    // DROPPED, not drained: an escaped descendant holding a pipe handle
    // would make read_to_end wait for IT (unbounded linger through a
    // handle, not a process). Timed-out output is partial and untrusted
    // anyway — identity comes only from clean exits — so dropping is
    // both the bounded and the honest choice.
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if timed_out {
        drop(child.stdout.take());
        drop(child.stderr.take());
    } else {
        if let Some(out) = child.stdout.take() {
            out.take(CAPTURE_CAP).read_to_end(&mut stdout).ok();
        }
        if let Some(err) = child.stderr.take() {
            err.take(CAPTURE_CAP).read_to_end(&mut stderr).ok();
        }
    }
    // Final reap for the exited path (idempotent when already waited).
    let _ = child.wait();

    Ok(ProbeOutcome {
        timed_out,
        exit_ok,
        stdout,
        stderr,
        elapsed: start.elapsed(),
        cleanup,
    })
}

/// Bounded termination after the health deadline (public so integration
/// tests exercise the SAME primitive production uses — no replicas):
/// 1. direct primary-child kill immediately;
/// 2. supervised process-tree kill (Windows);
/// 3. reap polled under the budget — never an unbounded wait.
///
/// A candidate that exits during cleanup classifies
/// [`CleanupState::TerminatedAndReaped`] (no spurious fatal). A candidate
/// still alive at budget expiry classifies
/// [`CleanupState::CleanupIncomplete`].
pub fn terminate_bounded(child: &mut std::process::Child, budget: Duration) -> CleanupState {
    let deadline = Instant::now() + budget;
    // Order matters on Windows: the tree-kill runs FIRST while the root
    // is still alive so /T can enumerate the tree. A direct kill first
    // would destroy the root and orphan the descendants (taskkill on a
    // dead PID walks nothing). The direct kill below is the backstop.
    #[cfg(windows)]
    let tree_note = supervised_tree_kill(child.id(), deadline);
    #[cfg(not(windows))]
    let tree_note: Option<String> = None;
    let _ = child.kill();
    // Bounded reap poll.
    match reap_bounded(child, deadline) {
        Ok(()) => CleanupState::TerminatedAndReaped,
        Err(why) => CleanupState::CleanupIncomplete(match tree_note {
            Some(note) => format!("{why}; tree-kill note: {note}"),
            None => why,
        }),
    }
}

/// Poll `try_wait` until `deadline`. Returns Ok once the child has
/// exited (reaped by the successful `try_wait`); Err if still alive at
/// the deadline. No blocking wait — the bound is absolute.
pub(crate) fn reap_bounded(
    child: &mut std::process::Child,
    deadline: Instant,
) -> Result<(), String> {
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {
                if Instant::now() >= deadline {
                    return Err("candidate would not confirm termination within budget".to_string());
                }
                std::thread::sleep(POLL);
            }
            Err(e) => return Err(format!("candidate wait failed during cleanup: {e}")),
        }
    }
}

/// Run a helper child to completion under `deadline`, then reap it.
/// Returns its exit status, or None on budget expiry (helper killed).
/// The tree-kill helper going silent can never stall the updater.
pub(crate) fn run_bounded_helper(
    mut helper: std::process::Child,
    deadline: Instant,
) -> Option<std::process::ExitStatus> {
    loop {
        match helper.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = helper.kill();
                    let _ = helper.wait();
                    return None;
                }
                std::thread::sleep(POLL);
            }
            Err(_) => return None,
        }
    }
}

/// Windows process-tree termination as a SUPERVISED child: taskkill runs
/// with the same bounded discipline as everything else — if the helper
/// itself stalls past `deadline`, it is killed and the stall is reported
/// (Some(note)) instead of hanging the updater. Returns None when the
/// helper ran (whatever its exit code — taskkill exits nonzero if the
/// target already died in the direct-kill race; that is fine).
#[cfg(windows)]
fn supervised_tree_kill(pid: u32, deadline: Instant) -> Option<String> {
    let helper = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match helper {
        Ok(h) => match run_bounded_helper(h, deadline) {
            Some(_) => None,
            None => Some("tree-kill helper exceeded its budget and was killed".to_string()),
        },
        Err(e) => Some(format!("tree-kill helper would not spawn: {e}")),
    }
}

/// Default-deadline entry used by production health checks.
pub fn probe_candidate_default(binary: &Path) -> Result<ProbeOutcome, LifecycleError> {
    probe_candidate(binary, HEALTH_DEADLINE)
}

/// Parse bounded `--version` output into (version, contract, sha), using
/// the same rules as the pre-repair check (clap prints
/// `<bin> <version> contract:<c> commit:<sha> ...`).
pub fn parse_identity(output: &[u8]) -> (Option<String>, Option<String>, Option<String>) {
    let text = String::from_utf8_lossy(output);
    let version = text
        .split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_string);
    let contract = text
        .split_whitespace()
        .find(|w| w.starts_with("contract:"))
        .map(|w| w.trim_start_matches("contract:").to_string());
    let sha = text
        .split_whitespace()
        .find(|w| w.starts_with("commit:"))
        .map(|w| w.trim_start_matches("commit:").to_string());
    (version, contract, sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_parse_rules() {
        let (v, c, s) = parse_identity(
            b"omen 0.9.0-preview.16 contract:0.8 commit:abc123 target:x profile:release",
        );
        assert_eq!(v.as_deref(), Some("0.9.0-preview.16"));
        assert_eq!(c.as_deref(), Some("0.8"));
        assert_eq!(s.as_deref(), Some("abc123"));
        let (v, _, _) = parse_identity(b"garbage with no digits?? yes 1 has");
        assert_eq!(v.as_deref(), Some("1"));
        let (v, c, s) = parse_identity(b"");
        assert!(v.is_none() && c.is_none() && s.is_none());
    }

    /// Write an executable stall fixture: ignores ALL args (the probe
    /// appends `--version`) and sleeps 30 s. Hermetic: only OS-core
    /// primitives (cmd+powershell / sh+sleep), staged in a TempDir.
    #[cfg(windows)]
    fn stall_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let bat = dir.path().join("stall.bat");
        std::fs::write(
            &bat,
            "@echo off\r\npowershell -NoProfile -Command \"Start-Sleep 30\"\r\n",
        )
        .unwrap();
        (dir, bat)
    }

    #[cfg(not(windows))]
    fn stall_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let sh = dir.path().join("stall.sh");
        std::fs::write(&sh, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, sh)
    }

    /// Fast fixture printing a valid identity line and exiting 0.
    #[cfg(windows)]
    fn quick_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let bat = dir.path().join("quick.bat");
        std::fs::write(
            &bat,
            "@echo off\r\necho omen 0.9.0-preview.18 contract:0.8 commit:abc123\r\n",
        )
        .unwrap();
        (dir, bat)
    }

    #[cfg(not(windows))]
    fn quick_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let sh = dir.path().join("quick.sh");
        std::fs::write(
            &sh,
            "#!/bin/sh\necho 'omen 0.9.0-preview.18 contract:0.8 commit:abc123'\n",
        )
        .unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, sh)
    }

    /// TEST 1 — staller: the health deadline fires, cleanup begins, and
    /// the TOTAL stays within deadline + budget + scheduling margin.
    /// Kill always works on the fixture, so cleanup must classify
    /// TerminatedAndReaped (the child is reaped by construction).
    #[test]
    fn stall_returns_within_deadline_plus_budget() {
        let (_dir, bin) = stall_fixture();
        let t0 = Instant::now();
        let out = probe_candidate_with_budget(&bin, Duration::from_secs(1), Duration::from_secs(1))
            .unwrap();
        let total = t0.elapsed();
        assert!(out.timed_out);
        assert_eq!(out.cleanup, CleanupState::TerminatedAndReaped);
        assert!(
            total <= Duration::from_secs(2) + Duration::from_secs(3),
            "total {total:?} exceeded deadline(1s)+budget(1s)+margin(3s)"
        );
    }

    /// TEST 4 — candidate exits during the cleanup race: deadline 0
    /// forces the timeout path against a fast-exiting process. Must
    /// classify terminated with no spurious fatal cleanup error.
    #[test]
    fn exit_during_cleanup_race_is_terminated() {
        let (_dir, bin) = quick_fixture();
        let out =
            probe_candidate_with_budget(&bin, Duration::from_millis(0), Duration::from_secs(5))
                .unwrap();
        assert!(out.timed_out, "zero deadline must take the timeout path");
        assert_eq!(out.cleanup, CleanupState::TerminatedAndReaped);
    }

    /// TEST 3 — the supervised helper runner bounds a stalling helper:
    /// returns None within budget + margin, no infinite wait. This is the
    /// mechanism that keeps a silent tree-kill helper from stalling the
    /// updater.
    #[test]
    fn stalling_helper_is_bounded() {
        let (_dir, bin) = stall_fixture();
        let helper = std::process::Command::new(&bin)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let t0 = Instant::now();
        let status = run_bounded_helper(helper, t0 + Duration::from_millis(500));
        assert!(status.is_none(), "stalling helper must expire");
        assert!(
            t0.elapsed() <= Duration::from_millis(500) + Duration::from_secs(3),
            "helper runner exceeded its bound"
        );
    }

    /// TEST 5 — a live child polled WITHOUT killing classifies
    /// CleanupIncomplete: the refuses-to-confirm path, exercised with a
    /// real process. The child is killed afterwards (test hygiene).
    #[test]
    fn live_child_classifies_cleanup_incomplete() {
        let (_dir, bin) = stall_fixture();
        let mut helper = std::process::Command::new(&bin)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let err =
            reap_bounded(&mut helper, Instant::now() + Duration::from_millis(200)).unwrap_err();
        assert!(err.contains("would not confirm termination"));
        let _ = helper.kill();
        let _ = helper.wait();
    }

    /// TEST 2 (Windows) — tree cleanup: a parent that holds a live
    /// grandchild in its process tree. The probe spawns the parent, times
    /// out, and must take parent AND grandchild (taskkill /T via the
    /// supervised helper). The grandchild PID is tracked through a marker
    /// file and asserted absent with tasklist.
    #[cfg(windows)]
    #[test]
    fn tree_child_is_reaped_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("grandchild.pid");
        let parent_ps1 = dir.path().join("tree_parent.ps1");
        std::fs::write(
            &parent_ps1,
            format!(
                "$g = Start-Process powershell -ArgumentList '-NoProfile','-Command','Start-Sleep 60' -PassThru\r\n\
                 $g.Id | Out-File -FilePath '{}' -Encoding ascii\r\n\
                 Start-Sleep 60\r\n",
                marker.display()
            ),
        )
        .unwrap();
        let bat = dir.path().join("tree.bat");
        std::fs::write(
            &bat,
            format!(
                "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"{}\"\r\n",
                parent_ps1.display()
            ),
        )
        .unwrap();
        // The 8 s deadline gives the parent (~2 s startup) ample time to
        // record the grandchild before the timeout fires.
        let out = probe_candidate_with_budget(&bat, Duration::from_secs(8), Duration::from_secs(5))
            .unwrap();
        assert!(out.timed_out);
        assert_eq!(out.cleanup, CleanupState::TerminatedAndReaped);
        let text =
            std::fs::read_to_string(&marker).expect("parent must have recorded the grandchild PID");
        let grand_pid: u32 = text.trim().parse().expect("marker holds a PID");
        // Hygiene first (never leave a 60 s sleeper), then the real
        // assertion: the grandchild must ALREADY be gone.
        let already_gone = !pid_runs(grand_pid);
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &grand_pid.to_string(), "/F"])
            .output();
        assert!(already_gone, "grandchild {grand_pid} survived tree-kill");
    }

    #[cfg(windows)]
    fn pid_runs(pid: u32) -> bool {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output();
        match out {
            Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")),
            Err(_) => false,
        }
    }
}
