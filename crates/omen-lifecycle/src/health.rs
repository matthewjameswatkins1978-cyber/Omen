//! Bounded candidate interrogation (Lucy repair B2).
//!
//! The staged candidate is asked exactly one question (`--version`) under
//! a hard deadline with bounded output capture. A corrupt, hostile, or
//! merely broken candidate can never hang `omen update`: on timeout the
//! probe terminates the process TREE where the platform allows it
//! (Windows Job/child-tree kill), reaps the child, and reports a truthful
//! timeout. No unbounded worker threads: the waiter polls `try_wait`.

use crate::error::LifecycleError;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Health deadline: private by Lucy decision (10–15s suggested).
const HEALTH_DEADLINE: Duration = Duration::from_secs(15);
/// Per-stream capture cap (1 MiB). Floods are truncated, never OOM.
const CAPTURE_CAP: u64 = 1024 * 1024;
/// Poll granularity for the waiter.
const POLL: Duration = Duration::from_millis(50);

/// Outcome of one bounded interrogation.
#[derive(Debug)]
pub struct ProbeOutcome {
    pub timed_out: bool,
    pub exit_ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
}

/// Interrogate `binary --version` with an explicit deadline. Closed stdin,
/// bounded captures, tree-kill + reap on timeout.
pub fn probe_candidate(binary: &Path, deadline: Duration) -> Result<ProbeOutcome, LifecycleError> {
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

    if timed_out {
        kill_tree(&mut child);
        // Reap: wait() after kill returns promptly on a dead child —
        // never a zombie, never a hang.
        let _ = child.wait();
    }

    let exit_ok = child
        .try_wait()
        .map(|s| s.map(|s| s.success()).unwrap_or(false))
        .unwrap_or(false);
    // Drain AFTER the process is gone: no pipe deadlock possible, captures
    // hard-capped against floods.
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(out) = child.stdout.take() {
        out.take(CAPTURE_CAP).read_to_end(&mut stdout).ok();
    }
    if let Some(err) = child.stderr.take() {
        err.take(CAPTURE_CAP).read_to_end(&mut stderr).ok();
    }
    // Final reap for the exited path (idempotent when already waited).
    let _ = child.wait();

    Ok(ProbeOutcome {
        timed_out,
        exit_ok,
        stdout,
        stderr,
        elapsed: start.elapsed(),
    })
}

/// Default-deadline entry used by production health checks.
pub fn probe_candidate_default(binary: &Path) -> Result<ProbeOutcome, LifecycleError> {
    probe_candidate(binary, HEALTH_DEADLINE)
}

#[cfg(windows)]
fn kill_tree(child: &mut std::process::Child) {
    // taskkill /T terminates the process TREE (descendants included).
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    // Direct-handle backstop for the primary child.
    let _ = child.kill();
}

#[cfg(not(windows))]
fn kill_tree(child: &mut std::process::Child) {
    // No portable tree-kill primitive: terminate the direct child. The
    // caller reaps it; orphaned grandchildren are the documented unix
    // limitation (Windows primary acceptance has the /T guarantee).
    let _ = child.kill();
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
}
