//! Bounded candidate interrogation (Lucy repair B2, hardened Preview 19).
//!
//! The staged candidate is asked exactly one question (`--version`) under
//! a hard deadline with file-backed output capture. A corrupt, hostile, or
//! merely broken candidate can never hang `omen update`.
//!
//! ONE bounded supervision model (no unbounded operation after supervision
//! begins — no `Child::wait()`, no `Command::output()`, no `read_to_end()`
//! on foreign live pipes):
//! - execution: [`HEALTH_DEADLINE`] (15 s), polled `try_wait` only;
//! - output: stdout/stderr redirected to temp files beside the staged
//!   binary (the isolated update staging area). Files have no EOF
//!   dependency on inherited handles: a descendant holding the write end
//!   open can NEVER block Omen's snapshot read. Snapshots are capped at
//!   [`CAPTURE_CAP`] each;
//! - containment: Windows Job Object (`KILL_ON_JOB_CLOSE`; supervised
//!   taskkill fallback when no job can be assigned) / Unix own process
//!   group (`SIGKILL` to the group). Strays are swept on both the timeout
//!   AND the clean-exit path;
//! - cleanup: [`CLEANUP_BUDGET`] (3 s) covering termination, tree
//!   containment, and CONFIRMED reap (job empty / group empty / helper
//!   done). Every waiter polls `try_wait`; expiry classifies
//!   [`CleanupState::CleanupIncomplete`] — never a blocking wait.
//!
//! Cleanup outcomes are classified, never disguised. A cleanup failure is
//! a HEALTH FAILURE — activation stays forbidden, the previous slot is
//! preserved, and the diagnostic says cleanup could not be confirmed.
//! Unknown stays unknown. No unbounded worker threads.

use crate::error::LifecycleError;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Health deadline: private by Lucy decision (10–15s suggested).
const HEALTH_DEADLINE: Duration = Duration::from_secs(15);
/// Cleanup budget: termination + containment + confirmed reap must finish
/// inside this, or cleanup is reported incomplete (never an infinite wait).
pub const CLEANUP_BUDGET: Duration = Duration::from_secs(3);
/// Per-stream capture cap (1 MiB). Floods are truncated snapshots, never OOM.
const CAPTURE_CAP: u64 = 1024 * 1024;
/// Poll granularity for the waiters.
const POLL: Duration = Duration::from_millis(50);

/// What cleanup achieved. Internal truth, surfaced in diagnostics —
/// NEVER collapsed into "health succeeded".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupState {
    /// Candidate exited on its own; no kill needed.
    NotNeeded,
    /// Deadline fired, termination confirmed, child reaped AND the
    /// containment set confirmed empty (job / group / helper). No zombie,
    /// no linger.
    TerminatedAndReaped,
    /// Budget expired (or containment/helper unconfirmed) before
    /// termination could be confirmed. Carries the reason. Activation is
    /// forbidden.
    CleanupIncomplete(String),
}

/// Outcome of one bounded interrogation. `stdout`/`stderr` are diagnostic
/// snapshots read from regular files — never waited on.
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
/// file-backed captures, contained process tree, bounded kill + confirmed
/// reap on timeout. Capture files live beside the staged binary (the
/// isolated update staging area in production) and are deleted on return.
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
    let capture_dir = capture_dir_for(binary);
    let out_file = tempfile::NamedTempFile::new_in(&capture_dir).map_err(|e| {
        LifecycleError::Health(format!("health capture file would not create: {e}"))
    })?;
    let err_file = tempfile::NamedTempFile::new_in(&capture_dir).map_err(|e| {
        LifecycleError::Health(format!("health capture file would not create: {e}"))
    })?;
    let out_path = out_file.path().to_path_buf();
    let err_path = err_file.path().to_path_buf();

    let mut cmd = std::process::Command::new(binary);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::from(out_file.as_file().try_clone().map_err(
            |e| LifecycleError::Health(format!("health capture handle would not clone: {e}")),
        )?))
        .stderr(Stdio::from(err_file.as_file().try_clone().map_err(
            |e| LifecycleError::Health(format!("health capture handle would not clone: {e}")),
        )?));
    #[cfg(unix)]
    {
        // Own process group: containment set for group termination. Uses
        // only std (stable CommandExt); the kill itself uses rustix.
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| LifecycleError::Health(format!("candidate would not spawn: {e}")))?;

    // Windows containment: fresh Job Object, KILL_ON_JOB_CLOSE. All
    // descendants join automatically; closing the handle kills strays on
    // BOTH the timeout and the clean-exit path. None on assignment failure
    // (nested-job hosts) — the supervised taskkill fallback covers that.
    #[cfg(windows)]
    let job = assign_contained_job(&child);
    #[cfg(not(windows))]
    let job: Option<JobGuard> = None;

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
        contain_terminate(&mut child, &job, cleanup_budget)
    } else {
        // Clean root exit: sweep strays best-effort (they hold no power
        // over the probe — capture is file-backed — this is hygiene).
        sweep_strays(&child, &job);
        CleanupState::NotNeeded
    };

    // NO wait(): the exit status is whatever try_wait last saw. On the
    // timeout path a reaped-by-cleanup child reads back here; an
    // unconfirmed one is simply not exit_ok.
    let exit_ok = child
        .try_wait()
        .map(|s| s.map(|s| s.success()).unwrap_or(false))
        .unwrap_or(false);

    // Snapshot reads are regular-file reads: they return what is there and
    // NEVER wait for EOF from a live foreign process, even if a stray
    // descendant still holds the write end open. Capped — floods truncate.
    let stdout = read_snapshot(&out_path);
    let stderr = read_snapshot(&err_path);
    // NamedTempFiles delete on drop: no capture residue in staging.
    drop(out_file);
    drop(err_file);

    Ok(ProbeOutcome {
        timed_out,
        exit_ok,
        stdout,
        stderr,
        elapsed: start.elapsed(),
        cleanup,
    })
}

/// Capture directory: beside the staged binary (the isolated update
/// staging area in production). Falls back to the OS temp dir when the
/// binary has no usable parent.
fn capture_dir_for(binary: &Path) -> PathBuf {
    binary
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir)
}

/// Bounded read of at most [`CAPTURE_CAP`] bytes from a regular capture
/// file. Regular files have no EOF-blocking behavior against live foreign
/// writers: this always returns promptly.
fn read_snapshot(path: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    if let Ok(f) = std::fs::File::open(path) {
        f.take(CAPTURE_CAP).read_to_end(&mut out).ok();
    }
    out
}

/// Timeout-path containment + confirmed reap, bounded by `budget` via one
/// absolute deadline shared by every poll below. Returns
/// [`CleanupState::TerminatedAndReaped`] only when the root is reaped AND
/// the containment set is confirmed empty; anything else is
/// [`CleanupState::CleanupIncomplete`] — never a blocking wait.
#[cfg(windows)]
fn contain_terminate(
    child: &mut std::process::Child,
    job: &Option<JobGuard>,
    budget: Duration,
) -> CleanupState {
    let deadline = Instant::now() + budget;
    if let Some(guard) = job {
        // Job kill covers the whole tree atomically; the guard close at
        // function end is the backstop (KILL_ON_JOB_CLOSE).
        guard.terminate();
        return match reap_bounded(child, deadline) {
            Ok(()) => match guard.poll_empty(deadline) {
                true => CleanupState::TerminatedAndReaped,
                false => CleanupState::CleanupIncomplete(
                    "job would not confirm empty within budget".to_string(),
                ),
            },
            Err(why) => CleanupState::CleanupIncomplete(why),
        };
    }
    // No-job fallback: supervised tree-kill FIRST while the root is alive
    // (taskkill /T enumerates the live tree), then direct-kill backstop,
    // then bounded root reap. Order matters — direct kill first would
    // orphan the descendants.
    let tree_note = supervised_tree_kill(child.id(), deadline);
    let _ = child.kill();
    match reap_bounded(child, deadline) {
        Ok(()) => match tree_note {
            // Helper ran: tree-kill was delivered to the live tree.
            None => CleanupState::TerminatedAndReaped,
            Some(note) => CleanupState::CleanupIncomplete(format!("tree-kill unconfirmed: {note}")),
        },
        Err(why) => CleanupState::CleanupIncomplete(match tree_note {
            Some(note) => format!("{why}; tree-kill note: {note}"),
            None => why,
        }),
    }
}

/// Timeout-path containment + confirmed reap on Unix: SIGKILL to the whole
/// process group (own pgid from `process_group(0)`), direct-kill backstop,
/// bounded root reap, then group-empty confirmation (killpg to an empty
/// group returns ESRCH — re-sending KILL to dying members is harmless).
#[cfg(unix)]
fn contain_terminate(
    child: &mut std::process::Child,
    _job: &Option<()>,
    budget: Duration,
) -> CleanupState {
    let deadline = Instant::now() + budget;
    group_kill(child.id());
    let _ = child.kill();
    match reap_bounded(child, deadline) {
        Ok(()) => match poll_group_empty(child.id(), deadline) {
            true => CleanupState::TerminatedAndReaped,
            false => CleanupState::CleanupIncomplete(
                "process group would not confirm empty within budget".to_string(),
            ),
        },
        Err(why) => CleanupState::CleanupIncomplete(why),
    }
}

/// Non-Windows/Unix fallback: direct child only, bounded reap.
#[cfg(not(any(windows, unix)))]
fn contain_terminate(
    child: &mut std::process::Child,
    _job: &Option<()>,
    budget: Duration,
) -> CleanupState {
    let deadline = Instant::now() + budget;
    let _ = child.kill();
    match reap_bounded(child, deadline) {
        Ok(()) => CleanupState::TerminatedAndReaped,
        Err(why) => CleanupState::CleanupIncomplete(why),
    }
}

/// Clean-exit stray sweep (best-effort hygiene, unconfirmed by design —
/// the root exited cleanly so cleanup is [`CleanupState::NotNeeded`];
/// strays cannot affect the already-read snapshots).
#[cfg(windows)]
fn sweep_strays(_child: &std::process::Child, job: &Option<JobGuard>) {
    // Dropping the caller's guard is what kills; here the guard is still
    // alive (it drops at probe end). Explicitly terminate the job now so
    // strays die before the snapshot read rather than after.
    if let Some(guard) = job {
        guard.terminate();
    }
}

/// Clean-exit stray sweep on Unix: SIGKILL the group best-effort.
#[cfg(unix)]
fn sweep_strays(child: &std::process::Child, _job: &Option<()>) {
    group_kill(child.id());
}

/// Other platforms: no sweep available (documented limitation).
#[cfg(not(any(windows, unix)))]
fn sweep_strays(_child: &std::process::Child, _job: &Option<()>) {}

/// Compatibility alias so every platform names the containment token
/// `Option<JobGuard>` in shared signatures.
#[cfg(not(windows))]
type JobGuard = ();

/// Bounded termination after the health deadline (public so integration
/// tests exercise the SAME fallback primitive production uses when no Job
/// Object is available — no replicas): supervised process-tree kill
/// (Windows), direct-child kill, and reap polled under the budget — never
/// an unbounded wait.
///
/// A candidate that exits during cleanup classifies
/// [`CleanupState::TerminatedAndReaped`] (no spurious fatal). A candidate
/// still alive at budget expiry classifies
/// [`CleanupState::CleanupIncomplete`].
pub fn terminate_bounded(child: &mut std::process::Child, budget: Duration) -> CleanupState {
    let deadline = Instant::now() + budget;
    #[cfg(windows)]
    let tree_note = supervised_tree_kill(child.id(), deadline);
    #[cfg(not(windows))]
    let tree_note: Option<String> = None;
    #[cfg(unix)]
    group_kill(child.id());
    let _ = child.kill();
    // Bounded reap poll — the ONLY wait, and it is budgeted.
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

/// How a supervised helper ended. There is no "waited forever" variant.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HelperEnd {
    /// Helper exited (any code) inside the bound.
    Done,
    /// Budget expired: kill attempted, but termination could NOT be
    /// confirmed inside the bound. Reported, never waited on.
    Unconfirmed,
}

/// Run a helper child under `deadline`, then confirm it. Returns
/// [`HelperEnd::Done`] with its exit status, or
/// [`HelperEnd::Unconfirmed`] on budget expiry (helper killed on the way
/// out, confirmation polled ONLY until the deadline — never an unbounded
/// `wait()`). The tree-kill helper going silent can never stall the
/// updater. Production needs it only on Windows (supervised taskkill);
/// tests on every platform exercise the bound through it.
#[cfg(any(windows, test))]
pub(crate) fn run_bounded_helper(
    mut helper: std::process::Child,
    deadline: Instant,
) -> (HelperEnd, Option<std::process::ExitStatus>) {
    loop {
        match helper.try_wait() {
            Ok(Some(status)) => return (HelperEnd::Done, Some(status)),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = helper.kill();
                    // Confirmation poll ONLY: bounded by the same deadline.
                    // A pathological helper that ignores the kill is
                    // reported Unconfirmed — never waited on forever.
                    loop {
                        match helper.try_wait() {
                            Ok(Some(status)) => return (HelperEnd::Done, Some(status)),
                            Ok(None) => {
                                if Instant::now() >= deadline {
                                    return (HelperEnd::Unconfirmed, None);
                                }
                                std::thread::sleep(POLL);
                            }
                            Err(_) => return (HelperEnd::Unconfirmed, None),
                        }
                    }
                }
                std::thread::sleep(POLL);
            }
            Err(_) => return (HelperEnd::Unconfirmed, None),
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
            (HelperEnd::Done, _) => None,
            (HelperEnd::Unconfirmed, _) => {
                Some("tree-kill helper exceeded its budget and was killed".to_string())
            }
        },
        Err(e) => Some(format!("tree-kill helper would not spawn: {e}")),
    }
}

/// Windows Job Object containment (Preview 19): a fresh job with
/// KILL_ON_JOB_CLOSE; the candidate is assigned immediately after spawn so
/// every descendant joins automatically. Closing the last handle kills the
/// whole tree — atomic containment with no PID-race enumeration. All
/// failures return None (nested-job hosts) and the caller falls back to
/// supervised taskkill. Contained in this one function: no architectural
/// spread.
#[cfg(windows)]
struct JobGuard {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
fn assign_contained_job(child: &std::process::Child) -> Option<JobGuard> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return None;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if set == 0 {
            CloseHandle(job);
            return None;
        }
        let child_handle = child.as_raw_handle();
        if AssignProcessToJobObject(job, child_handle) == 0 {
            CloseHandle(job);
            return None;
        }
        Some(JobGuard { handle: job })
    }
}

#[cfg(windows)]
impl JobGuard {
    /// Kill the whole tree now (atomic; no enumeration race).
    fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.handle, 1);
        }
    }

    /// Poll job accounting until no active processes remain or `deadline`
    /// passes. Bounded confirmation — never a wait.
    fn poll_empty(&self, deadline: Instant) -> bool {
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
            QueryInformationJobObject,
        };
        loop {
            unsafe {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
                let mut got = 0u32;
                let ok = QueryInformationJobObject(
                    self.handle,
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as *mut _,
                    std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    &mut got,
                );
                if ok != 0 && info.ActiveProcesses == 0 {
                    return true;
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(POLL);
        }
    }
}

#[cfg(windows)]
impl Drop for JobGuard {
    /// Last handle close kills the tree (KILL_ON_JOB_CLOSE backstop).
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

/// Unix group SIGKILL, best-effort (already-exited races report ESRCH —
/// that is success, not failure).
#[cfg(unix)]
fn group_kill(pid: u32) {
    let pgid = rustix::process::Pid::from_raw(pid as i32);
    let _ = rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL);
}

/// Poll until the process group is empty (killpg to an empty group fails
/// ESRCH — re-sending KILL to dying members is harmless) or `deadline`
/// passes. Bounded confirmation — never a wait.
#[cfg(unix)]
fn poll_group_empty(pid: u32, deadline: Instant) -> bool {
    let pgid = rustix::process::Pid::from_raw(pid as i32);
    loop {
        match rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL) {
            Err(rustix::io::Errno::SRCH) => return true,
            _ => {
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(POLL);
            }
        }
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
            "@echo off\r\necho omen 0.9.0-preview.19 contract:0.8 commit:abc123\r\n",
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
            "#!/bin/sh\necho 'omen 0.9.0-preview.19 contract:0.8 commit:abc123'\n",
        )
        .unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, sh)
    }

    /// Hostile fixture: root spawns a 60 s descendant that INHERITS the
    /// redirected stdout/stderr, prints a valid identity line, and exits 0
    /// immediately. The descendant's PID is recorded in `marker` for
    /// containment assertions and test hygiene. This is the exact
    /// regression for the pipe-EOF hang: anonymous pipes would wait for the
    /// descendant; capture files never do.
    #[cfg(windows)]
    fn descendant_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("grandchild.pid");
        let bat = dir.path().join("spawn_and_exit.bat");
        std::fs::write(
            &bat,
            format!(
                "@echo off\r\nstart \"\" /b powershell -NoProfile -Command \"$PID | Out-File -FilePath '{}' -Encoding ascii; Start-Sleep 60\"\r\necho omen 0.9.0-preview.19 contract:0.8 commit:abc123\r\n",
                marker.display()
            ),
        )
        .unwrap();
        (dir, bat, marker)
    }

    #[cfg(not(windows))]
    fn descendant_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("grandchild.pid");
        let sh = dir.path().join("spawn_and_exit.sh");
        std::fs::write(
            &sh,
            format!(
                "#!/bin/sh\n( sleep 60 & echo $! > '{}' )\necho 'omen 0.9.0-preview.19 contract:0.8 commit:abc123'\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, sh, marker)
    }

    /// Flood fixture: writes 3 MiB (> CAPTURE_CAP) to stdout, exits 0.
    #[cfg(windows)]
    fn flood_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let ps1 = dir.path().join("flood.ps1");
        std::fs::write(&ps1, "$s = 'x' * 3000000\r\n$s\r\n").unwrap();
        let bat = dir.path().join("flood.bat");
        std::fs::write(
            &bat,
            format!(
                "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"{}\"\r\n",
                ps1.display()
            ),
        )
        .unwrap();
        (dir, bat)
    }

    #[cfg(not(windows))]
    fn flood_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let sh = dir.path().join("flood.sh");
        std::fs::write(&sh, "#!/bin/sh\nhead -c 3000000 /dev/zero | tr '\\0' 'x'\n").unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, sh)
    }

    fn read_marker_pid(marker: &Path) -> Option<u32> {
        std::fs::read_to_string(marker)
            .ok()
            .and_then(|t| t.trim().parse().ok())
    }

    #[cfg(windows)]
    fn pid_runs(pid: u32) -> bool {
        // tasklist itself is a trusted local OS query (not a foreign
        // candidate): bounded by the OS, used for assertions only.
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output();
        match out {
            Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")),
            Err(_) => false,
        }
    }

    #[cfg(unix)]
    fn pid_runs(pid: u32) -> bool {
        // Signal 0 to our own fixture descendant: existence check only,
        // never a wait.
        let p = rustix::process::Pid::from_raw(pid as i32);
        rustix::process::kill(p, rustix::process::Signal::from_raw(0)).is_ok()
    }

    fn kill_pid_hygiene(pid: u32) {
        #[cfg(windows)]
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output();
        #[cfg(unix)]
        {
            let p = rustix::process::Pid::from_raw(pid as i32);
            let _ = rustix::process::kill(p, rustix::process::Signal::KILL);
        }
    }

    /// TEST 1 — staller: the health deadline fires, containment + confirmed
    /// reap complete, and the TOTAL stays within deadline + budget +
    /// scheduling margin.
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

    /// TEST 2 — clean-exit descendant holding inherited output handles:
    /// root prints valid identity and exits 0 while a 60 s descendant
    /// keeps the redirected stdout/stderr open. The probe must return
    /// WITHOUT timing out (a pipe-EOF implementation would hang to the
    /// deadline), capture the identity, and the descendant must be GONE
    /// (job kill on Windows, group kill on Unix).
    #[test]
    fn clean_exit_descendant_holding_handles_returns_bounded() {
        let (_dir, bin, marker) = descendant_fixture();
        let t0 = Instant::now();
        let out =
            probe_candidate_with_budget(&bin, Duration::from_secs(10), Duration::from_secs(3))
                .unwrap();
        let total = t0.elapsed();
        assert!(
            !out.timed_out,
            "probe must not hang on descendant-held handles"
        );
        assert!(out.exit_ok);
        assert_eq!(out.cleanup, CleanupState::NotNeeded);
        let (v, c, _) = parse_identity(&out.stdout);
        assert_eq!(v.as_deref(), Some("0.9.0-preview.19"));
        assert_eq!(c.as_deref(), Some("0.8"));
        // No deadline dependence: root exits in ~1 s; the probe must be
        // nowhere near the 10 s deadline.
        assert!(
            total < Duration::from_secs(8),
            "probe took {total:?} — deadline-dependent, handles blocked capture"
        );
        // Containment: the 60 s descendant must already be gone. The
        // grandchild starts slower than the root exits, so poll for the
        // marker first (bounded); hygiene kills any survivor afterwards.
        let pid = (0..100).find_map(|_| {
            let pid = read_marker_pid(&marker);
            if pid.is_none() {
                std::thread::sleep(Duration::from_millis(50));
            }
            pid
        });
        if let Some(pid) = pid {
            std::thread::sleep(Duration::from_millis(500));
            let already_gone = !pid_runs(pid);
            kill_pid_hygiene(pid);
            assert!(already_gone, "descendant {pid} survived containment sweep");
        }
    }

    /// TEST 3 — cleanup-incomplete through the FULL probe: zero budget
    /// forces the termination path to expire unconfirmed. The probe must
    /// return inside deadline + budget + small margin with
    /// CleanupIncomplete — and absolutely no blocking wait afterwards
    /// (this test would hang forever under the old `child.wait()`).
    #[test]
    fn zero_budget_probe_classifies_cleanup_incomplete() {
        let (_dir, bin) = stall_fixture();
        let t0 = Instant::now();
        let out =
            probe_candidate_with_budget(&bin, Duration::from_millis(0), Duration::from_millis(0))
                .unwrap();
        let total = t0.elapsed();
        assert!(out.timed_out);
        assert!(
            matches!(out.cleanup, CleanupState::CleanupIncomplete(_)),
            "expected CleanupIncomplete, got {:?}",
            out.cleanup
        );
        assert!(
            total <= Duration::from_secs(5),
            "total {total:?} exceeded margin — something blocked after expiry"
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

    /// TEST 5 — the supervised helper runner bounds a stalling helper:
    /// returns within budget + margin, no infinite wait. This is the
    /// mechanism that keeps a silent tree-kill helper from stalling the
    /// updater. Either outcome (Done after kill, or Unconfirmed) is
    /// acceptable — the requirement is the BOUND.
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
        let (end, _) = run_bounded_helper(helper, t0 + Duration::from_millis(500));
        let total = t0.elapsed();
        let _ = end;
        assert!(
            total <= Duration::from_millis(500) + Duration::from_secs(3),
            "helper runner exceeded its bound: {total:?}"
        );
    }

    /// TEST 6 — helper with an already-expired deadline: kill is attempted
    /// and the function still returns inside a fixed margin. Exercises the
    /// post-kill confirmation poll (never a blocking wait).
    #[test]
    fn expired_deadline_helper_returns_bounded() {
        let (_dir, bin) = stall_fixture();
        let helper = std::process::Command::new(&bin)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let t0 = Instant::now();
        let (end, _) = run_bounded_helper(helper, t0);
        assert!(
            t0.elapsed() <= Duration::from_secs(4),
            "expired-deadline helper blocked"
        );
        let _ = end;
    }

    /// TEST 7 — a live child polled WITHOUT killing classifies
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

    /// TEST 8 — output flood: 3 MiB on stdout, clean exit. The snapshot is
    /// exactly CAPTURE_CAP (truncated, no OOM), the probe returns fast and
    /// clean. File-backed capture has no pipe backpressure: the candidate
    /// is never blocked by the reader.
    #[test]
    fn output_flood_is_truncated_snapshot() {
        let (_dir, bin) = flood_fixture();
        let t0 = Instant::now();
        let out =
            probe_candidate_with_budget(&bin, Duration::from_secs(15), Duration::from_secs(3))
                .unwrap();
        assert!(!out.timed_out);
        assert!(out.exit_ok);
        assert_eq!(
            out.stdout.len() as u64,
            CAPTURE_CAP,
            "snapshot must truncate at the cap"
        );
        assert!(
            t0.elapsed() < Duration::from_secs(14),
            "flood probe too slow: {:?}",
            t0.elapsed()
        );
    }

    /// TEST 9 (Windows) — tree cleanup through the timeout path: a parent
    /// that holds a live grandchild in its process tree. The probe spawns
    /// the parent, times out, and must take parent AND grandchild. The
    /// grandchild PID is tracked through a marker file and asserted absent.
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
        kill_pid_hygiene(grand_pid);
        assert!(already_gone, "grandchild {grand_pid} survived tree-kill");
    }
}
