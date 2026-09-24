//! Bounded candidate interrogation (Lucy repair B2, hardened Preview 19,
//! atomic Windows containment Preview 20, fail-closed Preview 21).
//!
//! The staged candidate is asked exactly one question (`--version`) under
//! a hard deadline with file-backed output capture. A corrupt, hostile, or
//! merely broken candidate can never hang `omen update`.
//!
//! HARD H RULE (Preview 21): on Windows, successful candidate execution
//! during health checking MUST imply atomic containment by construction.
//! If the Job Object cannot be created or the atomic job-list spawn is
//! refused, the probe FAILS CLOSED before executing the candidate — no
//! plain-spawn fallback, no post-hoc assign, no timing reliance. There is
//! therefore no production path that runs uncontained candidate code.
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
//! - containment: Windows Job Object (`KILL_ON_JOB_CLOSE`) entered
//!   ATOMICALLY at process creation (`STARTUPINFOEX` +
//!   `PROC_THREAD_ATTRIBUTE_JOB_LIST` — the kernel places the candidate in
//!   the job before any candidate code executes, so no pre-assignment
//!   descendant can escape; establishment failure FAILS CLOSED — the
//!   candidate never executes) / Unix own process group (`SIGKILL` to the
//!   group, set pre-exec — likewise no window). Strays are swept on both
//!   the timeout AND the clean-exit path;
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
    /// True when the candidate ran inside atomically-established
    /// containment: Windows job-list spawn (no pre-containment execution
    /// window), Unix pre-exec process group. On Windows the production
    /// probe fails closed, so a successful Windows outcome ALWAYS reports
    /// true here (kept for diagnostics/API compatibility); false is
    /// possible only on platforms with no containment set — never
    /// inferred, always reported.
    pub atomically_contained: bool,
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

    let out_clone = out_file.as_file().try_clone().map_err(|e| {
        LifecycleError::Health(format!("health capture handle would not clone: {e}"))
    })?;
    let err_clone = err_file.as_file().try_clone().map_err(|e| {
        LifecycleError::Health(format!("health capture handle would not clone: {e}"))
    })?;

    // Windows containment is established BEFORE any candidate code can
    // execute: a fresh Job Object (KILL_ON_JOB_CLOSE) is created first,
    // then the candidate is created atomically inside it via STARTUPINFOEX
    // + PROC_THREAD_ATTRIBUTE_JOB_LIST. The kernel assigns job membership
    // as part of process creation — there is no spawn-then-assign window
    // in which a pre-containment descendant could escape.
    //
    // FAIL CLOSED (Preview 21): if the job cannot be created or the
    // atomic spawn is refused, return a Health error BEFORE executing the
    // candidate. There is deliberately NO plain-spawn fallback and NO
    // post-hoc AssignProcessToJobObject in this path: a successful probe
    // MUST imply atomic containment by construction. The updater reports
    // "atomic containment unavailable / candidate health unavailable" and
    // preserves the previous slot. Running uncontained candidate code is
    // not an option this function can express.
    #[cfg(windows)]
    let job_guard = create_contained_job()?;
    #[cfg(windows)]
    let mut child =
        ProbeChild::Atomic(spawn_contained(binary, &out_clone, &err_clone, &job_guard)?);
    // Always Some on Windows (fail-closed above): keeps the shared
    // supervision signatures; the no-job backstop inside
    // `contain_terminate` is unreachable from production.
    #[cfg(windows)]
    let job: Option<JobGuard> = Some(job_guard);
    #[cfg(not(windows))]
    let job: Option<JobGuard> = None;
    #[cfg(not(windows))]
    let mut child = {
        let mut cmd = std::process::Command::new(binary);
        cmd.arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::from(out_clone))
            .stderr(Stdio::from(err_clone));
        #[cfg(unix)]
        {
            // Own process group: containment set for group termination. Set
            // pre-exec in the child (stable CommandExt) — atomically, with
            // no window, like the Windows job list.
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        cmd.spawn()
            .map_err(|e| LifecycleError::Health(format!("candidate would not spawn: {e}")))?
    };

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

    // Containment provenance: Windows success implies the atomic
    // job-list spawn was used (anything else fails closed above); Unix
    // containment is pre-exec by construction; other platforms have no
    // containment set (false, not unknown-by-silence).
    #[cfg(windows)]
    let atomically_contained = true;
    #[cfg(unix)]
    let atomically_contained = true;
    #[cfg(not(any(windows, unix)))]
    let atomically_contained = false;

    Ok(ProbeOutcome {
        timed_out,
        exit_ok,
        stdout,
        stderr,
        elapsed: start.elapsed(),
        cleanup,
        atomically_contained,
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
    child: &mut ProbeChild,
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
fn sweep_strays(_child: &ProbeChild, job: &Option<JobGuard>) {
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
/// tests exercise the SAME supervised tree-kill primitive — no replicas):
/// supervised process-tree kill (Windows), direct-child kill, and reap
/// polled under the budget — never an unbounded wait. (Preview 21: the
/// production probe always has its Job; this remains for already-spawned
/// test children and as the documented no-job backstop inside
/// `contain_terminate`.)
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
#[cfg(windows)]
pub(crate) fn reap_bounded(child: &mut impl WaitTarget, deadline: Instant) -> Result<(), String> {
    loop {
        match WaitTarget::try_wait(child) {
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

/// Poll `try_wait` until `deadline`. Returns Ok once the child has
/// exited (reaped by the successful `try_wait`); Err if still alive at
/// the deadline. No blocking wait — the bound is absolute.
#[cfg(not(windows))]
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

/// Windows Job Object containment (Preview 20, fail-closed Preview 21):
/// a fresh job with KILL_ON_JOB_CLOSE, created BEFORE the candidate
/// exists. The candidate is then created atomically inside it
/// ([`spawn_contained`]), so the job covers the candidate from its first
/// instruction — every descendant joins automatically, and closing the
/// last handle kills the whole tree on BOTH the timeout and the clean-exit
/// path. Creation failure returns Err (never None): the probe fails closed
/// before executing the candidate. Contained in this section: no
/// architectural spread.
///
/// Fail-closed error shape: every establishment failure names the phase
/// and states the consequence (health unavailable, previous preserved).
#[cfg(windows)]
fn atomic_unavailable(phase: &'static str) -> LifecycleError {
    LifecycleError::Health(format!(
        "atomic containment unavailable at {phase}: candidate health unavailable, previous preserved"
    ))
}

/// Deterministic forced-failure seam (tests only, compiled out of
/// production): a binary whose file name is exactly
/// `force_atomic_unavailable.exe` is REFUSED at containment
/// establishment — job creation and the atomic spawn both fail closed for
/// it, before any candidate code could execute. Name-triggered (not a
/// global flag) so parallel tests cannot interfere with each other.
/// Refusal-only: this seam can only make health MORE strict, never less —
/// no production entry point can reach uncontained execution through it.
#[cfg(all(windows, test))]
fn atomic_failure_forced(binary: &Path) -> bool {
    binary
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("force_atomic_unavailable.exe"))
}
#[cfg(windows)]
struct JobGuard {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
fn create_contained_job() -> Result<JobGuard, LifecycleError> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(atomic_unavailable("create_contained_job.create"));
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
            return Err(atomic_unavailable("create_contained_job.set-limits"));
        }
        Ok(JobGuard { handle: job })
    }
}

/// A Windows candidate created atomically inside its Job Object. Owns the
/// raw process handle (closed on drop); the JOB handle (owned by
/// [`JobGuard`]) is what kills strays, so drop order in the probe — child
/// first, job last — preserves the KILL_ON_JOB_CLOSE backstop.
#[cfg(windows)]
struct AtomicProcess {
    handle: windows_sys::Win32::Foundation::HANDLE,
    pid: u32,
}

#[cfg(windows)]
impl AtomicProcess {
    fn id(&self) -> u32 {
        self.pid
    }

    /// Direct kill (TerminateProcess). Used as a backstop; the job kill is
    /// the primary tree terminator.
    fn kill(&mut self) -> std::io::Result<()> {
        use windows_sys::Win32::System::Threading::TerminateProcess;
        unsafe {
            if TerminateProcess(self.handle, 1) == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }

    /// Zero-timeout poll only: signalled -> read the exit code (valid once
    /// signalled); unsignalled -> None. Never a wait.
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        use std::os::windows::process::ExitStatusExt;
        use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
        unsafe {
            match WaitForSingleObject(self.handle, 0) {
                WAIT_OBJECT_0 => {
                    let mut code: u32 = 0;
                    if GetExitCodeProcess(self.handle, &mut code) == 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(Some(std::process::ExitStatus::from_raw(code)))
                }
                WAIT_TIMEOUT => Ok(None),
                _ => Err(std::io::Error::last_os_error()),
            }
        }
    }
}

#[cfg(windows)]
impl Drop for AtomicProcess {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

/// The probe's child on Windows: atomically contained, created in the
/// job before executing a single instruction. There is exactly one
/// variant — the production probe cannot express an uncontained child.
/// (The negative control in the atomicity test spawns a plain
/// `std::process::Child` directly; it never enters this type and never
/// reaches the supervision body.)
#[cfg(windows)]
enum ProbeChild {
    Atomic(AtomicProcess),
}

#[cfg(windows)]
impl ProbeChild {
    fn id(&self) -> u32 {
        match self {
            ProbeChild::Atomic(p) => p.id(),
        }
    }

    fn kill(&mut self) -> std::io::Result<()> {
        match self {
            ProbeChild::Atomic(p) => p.kill(),
        }
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        match self {
            ProbeChild::Atomic(p) => p.try_wait(),
        }
    }
}

/// Narrow capability the bounded reaper needs. Implemented by
/// [`ProbeChild`] and by `std::process::Child` (so `reap_bounded` stays
/// generic and integration tests keep driving the same primitive).
#[cfg(windows)]
pub(crate) trait WaitTarget {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>>;
}

#[cfg(windows)]
impl WaitTarget for std::process::Child {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        std::process::Child::try_wait(self)
    }
}

#[cfg(windows)]
impl WaitTarget for ProbeChild {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        ProbeChild::try_wait(self)
    }
}

#[cfg(windows)]
impl WaitTarget for AtomicProcess {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        AtomicProcess::try_wait(self)
    }
}

/// Create the candidate atomically inside `job` (Windows): STARTUPINFOEX
/// carries PROC_THREAD_ATTRIBUTE_JOB_LIST, so the kernel places the new
/// process in the job AS PART OF CREATION. Candidate code cannot execute
/// before containment — the spawn→assign race is closed by construction,
/// not by speed, sleeps, or retries.
///
/// Stdio mirrors the plain path exactly: NUL stdin, the two capture files
/// as stdout/stderr (the handle list whitelists EXACTLY these three
/// handles — nothing else leaks in). Batch files run via COMSPEC, as std
/// does. No CREATE_SUSPENDED, no resume dance, no supervisor subsystem.
///
/// Fail closed: EVERY refusal returns Err — job-list refused, attribute
/// setup failed, or CreateProcessW failed — and the caller propagates it
/// before any candidate code runs. All syscalls here are synchronous and
/// local — nothing waits, nothing blocks.
///
/// Takes `&JobGuard` (not Option): an uncontained spawn is unrepresentable
/// in this signature. The test-only forced-failure seam
/// ([`atomic_failure_forced`]) is checked first so the zero-execution
/// regression can force establishment failure deterministically.
#[cfg(windows)]
fn spawn_contained(
    binary: &Path,
    stdout: &std::fs::File,
    stderr: &std::fs::File,
    job: &JobGuard,
) -> Result<AtomicProcess, LifecycleError> {
    #[cfg(test)]
    if atomic_failure_forced(binary) {
        return Err(atomic_unavailable("spawn_contained.forced"));
    }
    use std::os::windows::io::AsRawHandle;
    // Command line (safe code): `<binary> --version`; batch files via
    // COMSPEC (`cmd /c <script> --version`), mirroring std's .bat handling.
    let is_batch = matches!(
        binary
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("bat") | Some("cmd")
    );
    let mut argv: Vec<String> = Vec::with_capacity(4);
    if is_batch {
        argv.push(
            std::env::var_os("COMSPEC")
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "cmd.exe".to_string()),
        );
        argv.push("/c".to_string());
    }
    argv.push(binary.to_string_lossy().into_owned());
    argv.push("--version".to_string());
    let mut cmdline: Vec<u16> = Vec::with_capacity(260);
    for (i, a) in argv.iter().enumerate() {
        if i > 0 {
            cmdline.push(0x20);
        }
        quote_windows_arg(a, &mut cmdline);
    }
    cmdline.push(0);

    use windows_sys::Win32::Foundation::{
        CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
        EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList,
        LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
        PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
        UpdateProcThreadAttribute,
    };
    unsafe {
        // Private stdio handles, marked inheritable and whitelisted: the
        // ONLY handles the candidate inherits.
        let nul =
            std::fs::File::open("NUL").map_err(|_| atomic_unavailable("spawn_contained.stdio"))?;
        for f in [&nul, stdout, stderr] {
            if SetHandleInformation(
                f.as_raw_handle() as HANDLE,
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            ) == 0
            {
                return Err(atomic_unavailable("spawn_contained.stdio"));
            }
        }
        let mut inherit = [
            nul.as_raw_handle() as HANDLE,
            stdout.as_raw_handle() as HANDLE,
            stderr.as_raw_handle() as HANDLE,
        ];
        // Attribute list: two slots (job list + handle whitelist). Sized
        // first via the documented NULL call, then initialized.
        let mut size: usize = 0;
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 2, 0, &mut size);
        if size == 0 {
            return Err(atomic_unavailable("spawn_contained.attribute-list"));
        }
        let mut buf = vec![0u8; size];
        let list = buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        if InitializeProcThreadAttributeList(list, 2, 0, &mut size) == 0 {
            return Err(atomic_unavailable("spawn_contained.attribute-list"));
        }
        // Every failure path from here deletes the list before returning.
        let mut job_handle: HANDLE = job.handle();
        let ok_job = UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            &mut job_handle as *mut HANDLE as *const std::ffi::c_void,
            std::mem::size_of::<HANDLE>(),
            std::ptr::null_mut(),
            std::ptr::null(),
        ) != 0;
        let ok_handles = UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            inherit.as_mut_ptr() as *const std::ffi::c_void,
            inherit.len() * std::mem::size_of::<HANDLE>(),
            std::ptr::null_mut(),
            std::ptr::null(),
        ) != 0;
        if !ok_job || !ok_handles {
            DeleteProcThreadAttributeList(list);
            return Err(atomic_unavailable("spawn_contained.job-list-attribute"));
        }
        let mut si: STARTUPINFOEXW = std::mem::zeroed();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = inherit[0];
        si.StartupInfo.hStdOutput = inherit[1];
        si.StartupInfo.hStdError = inherit[2];
        si.lpAttributeList = list;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let created = CreateProcessW(
            std::ptr::null(),
            cmdline.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            std::ptr::null(),
            std::ptr::null(),
            &si.StartupInfo as *const _,
            &mut pi,
        );
        DeleteProcThreadAttributeList(list);
        if created == 0 {
            return Err(atomic_unavailable("spawn_contained.create-process"));
        }
        CloseHandle(pi.hThread);
        if pi.hProcess.is_null() {
            return Err(atomic_unavailable("spawn_contained.process-handle"));
        }
        Ok(AtomicProcess {
            handle: pi.hProcess,
            pid: pi.dwProcessId,
        })
    }
}

/// Quote one command-line argument per MSVCRT rules (what
/// CommandLineToArgvW inverts): quote when empty or containing
/// whitespace/quotes; double backslashes preceding a quote or the end.
#[cfg(windows)]
fn quote_windows_arg(arg: &str, out: &mut Vec<u16>) {
    const QUOTE: u16 = b'"' as u16;
    const SLASH: u16 = b'\\' as u16;
    let needs = arg.is_empty()
        || arg
            .bytes()
            .any(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\x0B' | b'"'));
    if !needs {
        out.extend(arg.encode_utf16());
        return;
    }
    out.push(QUOTE);
    let mut slashes = 0usize;
    for c in arg.encode_utf16() {
        if c == SLASH {
            slashes += 1;
        } else {
            if c == QUOTE {
                for _ in 0..slashes {
                    out.push(SLASH);
                }
                slashes = 0;
                out.push(SLASH);
            } else {
                for _ in 0..slashes {
                    out.push(SLASH);
                }
                slashes = 0;
            }
            out.push(c);
        }
    }
    for _ in 0..slashes * 2 {
        out.push(SLASH);
    }
    out.push(QUOTE);
}

#[cfg(windows)]
impl JobGuard {
    fn handle(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.handle
    }

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

/// Convert a raw PID to rustix's non-zero Pid (None only for 0, which a
/// live child never is).
#[cfg(unix)]
fn pid_of(raw: u32) -> Option<rustix::process::Pid> {
    rustix::process::Pid::from_raw(raw as i32)
}

/// Unix group SIGKILL, best-effort (already-exited races report ESRCH —
/// that is success, not failure).
#[cfg(unix)]
fn group_kill(pid: u32) {
    if let Some(pgid) = pid_of(pid) {
        let _ = rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL);
    }
}

/// Poll until the process group is empty (signal-0 probe reports ESRCH
/// once every member is gone) or `deadline` passes. Bounded confirmation
/// — never a wait.
#[cfg(unix)]
fn poll_group_empty(pid: u32, deadline: Instant) -> bool {
    let Some(pgid) = pid_of(pid) else {
        return true;
    };
    loop {
        match rustix::process::test_kill_process_group(pgid) {
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
            "@echo off\r\necho omen 0.9.0-preview.21 contract:0.8 commit:abc123\r\n",
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
            "#!/bin/sh\necho 'omen 0.9.0-preview.21 contract:0.8 commit:abc123'\n",
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
        // Root waits (bounded, ~6 s) for the grandchild to record itself
        // BEFORE exiting: containment must cover an already-running
        // descendant, so the test never depends on a spawn-vs-sweep race.
        std::fs::write(
            &bat,
            format!(
                "@echo off\r\nstart \"\" /b powershell -NoProfile -Command \"$PID | Out-File -FilePath '{}' -Encoding ascii; Start-Sleep 60\"\r\nset /a n=0\r\n:wait\r\nif exist \"{}\" goto done\r\nset /a n+=1\r\nif %n% GEQ 6 goto done\r\ntimeout /t 1 /nobreak >nul\r\ngoto wait\r\n:done\r\necho omen 0.9.0-preview.21 contract:0.8 commit:abc123\r\n",
                marker.display(),
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
                "#!/bin/sh\n( sleep 60 & echo $! > '{}' )\necho 'omen 0.9.0-preview.21 contract:0.8 commit:abc123'\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, sh, marker)
    }

    /// Atomic-containment fixture (Windows only): a NATIVE zero-dependency
    /// helper (compiled once per test with rustc — no PowerShell, no C#
    /// compile, millisecond startup, no console needed). Omen fixes
    /// candidate argv to `--version`, so fixture configuration travels in
    /// a `helper.cfg` sibling file (KEY=VALUE lines) — NEVER via
    /// process-wide env vars, which leak across parallel tests. Grandchild
    /// sleepers take `--sleep <pidfile>` via argv (record PID, hold
    /// inherited stdio open, sleep past deadlines). `tree=1` selects the
    /// TEST 9 tree parent (spawn the sleeper, sleep past the deadline,
    /// print nothing). The default prints a valid identity line and exits
    /// immediately, spawning the sleeper first when `gc_path` is set.
    /// Job membership is NEVER self-reported by the fixture: a NULL
    /// `IsProcessInJob` query is confounded by ambient (e.g. Cargo-owned)
    /// job membership inherited from the test runner. The deterministic
    /// proof (TEST 10b) queries membership against an EXPLICIT job handle
    /// from the test side instead — TRUE iff the job-list creation
    /// mechanism placed the child, FALSE for a plain spawn, with zero
    /// scheduler dependence (placement is atomic at creation, so query
    /// timing is irrelevant).
    #[cfg(windows)]
    const NATIVE_HELPER_RS: &str = r#"#![windows_subsystem = "windows"]
fn cfg_value(key: &str) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let text = std::fs::read_to_string(exe.with_extension("cfg")).ok()?;
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}
fn spawn_sleeper() {
    if let Some(gc) = cfg_value("gc_path") {
        let exe = std::env::current_exe().unwrap();
        std::process::Command::new(&exe)
            .arg("--sleep")
            .arg(&gc)
            .spawn()
            .unwrap();
    }
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Sleeper grandchild: record PID, hold inherited stdio open, sleep.
    if args.iter().any(|a| a == "--sleep") {
        if let Some(m) = args.iter().skip_while(|a| *a != "--sleep").nth(1) {
            let _ = std::fs::write(m, std::process::id().to_string());
        }
        std::thread::sleep(std::time::Duration::from_secs(60));
        return;
    }
    // Tree-parent mode (TEST 9): spawn the sleeper, sleep past deadline.
    if cfg_value("tree").as_deref() == Some("1") {
        spawn_sleeper();
        std::thread::sleep(std::time::Duration::from_secs(60));
        return;
    }
    // Probe mode: spawn the sleeper (if configured), print identity, exit
    // immediately — no waiting, so any descendant is necessarily already
    // born when the root exits.
    spawn_sleeper();
    println!("omen 0.9.0-preview.21 contract:0.8 commit:abc123");
}
"#;

    /// Build the native helper exe in a fresh tempdir (returns dir +
    /// helper path). rustc is on PATH wherever cargo test runs.
    #[cfg(windows)]
    fn native_helper() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("helper.rs"), NATIVE_HELPER_RS).unwrap();
        let out = std::process::Command::new("rustc")
            .arg("--edition=2021")
            .arg("--crate-name")
            .arg("omen_h_helper")
            .arg("helper.rs")
            .arg("-o")
            .arg("helper.exe")
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .output()
            .expect("rustc must be on PATH to build the containment fixture");
        assert!(
            out.status.success(),
            "helper build failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let helper_path = dir.path().join("helper.exe");
        (dir, helper_path)
    }

    /// Atomic-containment fixture: helper exe + grandchild marker. The
    /// helper is configured via a `helper.cfg` sibling (per-tempdir, so
    /// parallel tests never share state): probe mode with the gc marker
    /// when `tree` is false.
    #[cfg(windows)]
    fn atomic_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let (dir, helper) = native_helper();
        let gcpid = dir.path().join("atomic_gc.pid");
        write_helper_cfg(&dir, false, &gcpid);
        (dir, helper, gcpid)
    }

    /// Write the helper's `helper.cfg` sibling: `tree=1` selects
    /// tree-parent mode, `gc_path` selects the sleeper marker. Per-fixture
    /// tempdir — fully parallel-safe, no env involved.
    #[cfg(windows)]
    fn write_helper_cfg(dir: &tempfile::TempDir, tree: bool, gc: &Path) {
        let mut cfg = String::new();
        if tree {
            cfg.push_str("tree=1\n");
        }
        cfg.push_str(&format!("gc_path={}\n", gc.display()));
        std::fs::write(dir.path().join("helper.cfg"), cfg).unwrap();
    }

    /// Explicit job-membership query (Windows tests): is `process` a
    /// member of the EXPLICIT job `job`? Unlike a NULL-handle query (which
    /// is confounded by ambient job membership inherited from the test
    /// runner), this answers about OUR containment set only.
    #[cfg(windows)]
    fn in_explicit_job(
        process: windows_sys::Win32::Foundation::HANDLE,
        job: windows_sys::Win32::Foundation::HANDLE,
    ) -> bool {
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;
        let mut r: i32 = 0;
        unsafe { IsProcessInJob(process, job, &mut r) != 0 && r != 0 }
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
        // Signal-0 existence probe (test_kill_process): check only, never
        // a wait.
        pid_of(pid).is_some_and(|p| rustix::process::test_kill_process(p).is_ok())
    }

    fn kill_pid_hygiene(pid: u32) {
        #[cfg(windows)]
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output();
        #[cfg(unix)]
        {
            if let Some(p) = pid_of(pid) {
                let _ = rustix::process::kill_process(p, rustix::process::Signal::KILL);
            }
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
        assert_eq!(v.as_deref(), Some("0.9.0-preview.21"));
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

    /// TEST 4 — candidate exits during the cleanup race. On slow
    /// platforms the zero deadline takes the timeout path against the
    /// already-exiting process (must classify TerminatedAndReaped, no
    /// spurious fatal); on fast platforms the child may exit before the
    /// first poll (clean path: NotNeeded, exit_ok, identity captured).
    /// BOTH are correct — the race must never produce Incomplete or hang.
    #[test]
    fn exit_during_cleanup_race_is_terminated() {
        let (_dir, bin) = quick_fixture();
        let out =
            probe_candidate_with_budget(&bin, Duration::from_millis(0), Duration::from_secs(5))
                .unwrap();
        if out.timed_out {
            assert_eq!(out.cleanup, CleanupState::TerminatedAndReaped);
        } else {
            assert_eq!(out.cleanup, CleanupState::NotNeeded);
            assert!(out.exit_ok);
            let (v, c, _) = parse_identity(&out.stdout);
            assert_eq!(v.as_deref(), Some("0.9.0-preview.21"));
            assert_eq!(c.as_deref(), Some("0.8"));
        }
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

    /// TEST 9 (Windows) — tree cleanup through the timeout path: a NATIVE
    /// parent that spawns a live sleeper grandchild in its process tree
    /// within milliseconds, then sleeps past the deadline. The probe times
    /// out and must take parent AND grandchild. The grandchild PID is
    /// tracked through a marker file and asserted absent. Native fixture
    /// (no shell cold-start on the critical path): millisecond startup
    /// against a 5 s deadline, so slow shared CI runners cannot flake it.
    #[cfg(windows)]
    #[test]
    fn tree_child_is_reaped_on_windows() {
        // Tree mode via the helper.cfg sibling (per-tempdir: no shared
        // state with parallel tests).
        let (_dir, helper) = native_helper();
        let gcpid = _dir.path().join("tree_gc.pid");
        write_helper_cfg(&_dir, true, &gcpid);
        let out =
            probe_candidate_with_budget(&helper, Duration::from_secs(5), Duration::from_secs(5))
                .unwrap();
        assert!(out.timed_out);
        assert_eq!(out.cleanup, CleanupState::TerminatedAndReaped);
        let text =
            std::fs::read_to_string(&gcpid).expect("parent must have recorded the grandchild PID");
        let grand_pid: u32 = text.trim().parse().expect("marker holds a PID");
        // Hygiene first (never leave a 60 s sleeper), then the real
        // assertion: the grandchild must ALREADY be gone.
        let already_gone = !pid_runs(grand_pid);
        kill_pid_hygiene(grand_pid);
        assert!(already_gone, "grandchild {grand_pid} survived tree-kill");
    }

    /// TEST 10 (Windows) — end-to-end under atomic containment: the
    /// native candidate spawns a 60 s descendant and exits immediately
    /// (millisecond startup, no shell). The probe must return bounded
    /// with the identity captured, the atomic flag set, and the immediate
    /// descendant already gone via the clean-exit sweep.
    #[cfg(windows)]
    #[test]
    fn immediate_descendant_is_contained_atomically() {
        let (_dir, bin, gcpid) = atomic_fixture();
        let t0 = Instant::now();
        // Fail-closed environments (nested-job host: the OS refuses the
        // job list) cannot prove atomicity HERE — the probe refuses
        // instead of running uncontained. Reported, not disguised.
        let out = match probe_candidate_with_budget(
            &bin,
            Duration::from_secs(15),
            Duration::from_secs(3),
        ) {
            Ok(o) => o,
            Err(e) => {
                let msg = format!("{e:?}");
                if msg.contains("atomic containment unavailable") {
                    eprintln!("SKIP-ATOMIC: job-list spawn unavailable (nested-job host?)");
                    return;
                }
                panic!("probe failed unexpectedly: {e:?}");
            }
        };
        let total = t0.elapsed();
        assert!(
            !out.timed_out,
            "atomic probe must not hang on descendant-held handles"
        );
        assert!(out.exit_ok);
        assert_eq!(out.cleanup, CleanupState::NotNeeded);
        let (v, c, _) = parse_identity(&out.stdout);
        assert_eq!(v.as_deref(), Some("0.9.0-preview.21"));
        assert_eq!(c.as_deref(), Some("0.8"));
        // Fail-closed: a successful Windows production probe MUST imply
        // atomic containment by construction — the flag is asserted, not
        // skipped.
        assert!(
            out.atomically_contained,
            "successful Windows probe must report atomic containment"
        );
        assert!(
            total < Duration::from_secs(13),
            "atomic probe took {total:?} — deadline-dependent"
        );
        // The immediate descendant (born inside the job by inheritance)
        // must already be gone via the clean-exit sweep. Poll briefly for
        // the PID file (scheduling is never assumed), then assert absence
        // with hygiene afterwards.
        let gc = (0..60).find_map(|_| {
            let pid = read_marker_pid(&gcpid);
            if pid.is_none() {
                std::thread::sleep(Duration::from_millis(50));
            }
            pid
        });
        if let Some(pid) = gc {
            std::thread::sleep(Duration::from_millis(500));
            let already_gone = !pid_runs(pid);
            kill_pid_hygiene(pid);
            assert!(
                already_gone,
                "immediate descendant {pid} survived atomic containment"
            );
        }
    }

    /// TEST 10b (Windows) — THE atomicity proof, deterministic: drive the
    /// exact production primitive ([`spawn_contained`]) and query the
    /// child's membership against the EXPLICIT job handle. Placement via
    /// PROC_THREAD_ATTRIBUTE_JOB_LIST happens at creation, before any
    /// candidate instruction — so the verdict cannot depend on scheduling,
    /// polling speed, or spawn-vs-assign races: there is no window to
    /// race in. The negative control (plain spawn, same binary, direct
    /// `std::process::Child` — a low-level test control that never enters
    /// [`ProbeChild`] and never reaches the supervision body) must
    /// report NOT-a-member of the same job, proving the query
    /// discriminates. Together: TRUE iff the atomic mechanism placed the
    /// child. No hangs possible (no waits, one bounded reap each).
    #[cfg(windows)]
    #[test]
    fn atomic_spawn_places_child_in_job_at_creation() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::HANDLE;
        let (_dir, helper) = native_helper();
        let job = match create_contained_job() {
            Ok(j) => j,
            Err(_) => {
                eprintln!("SKIP-ATOMIC: job creation unavailable in this environment");
                return;
            }
        };
        let tmp = tempfile::tempdir().unwrap();
        let cap_out = std::fs::File::create(tmp.path().join("o")).unwrap();
        let cap_err = std::fs::File::create(tmp.path().join("e")).unwrap();
        let mut atom = match spawn_contained(&helper, &cap_out, &cap_err, &job) {
            Ok(a) => a,
            Err(_) => {
                eprintln!("SKIP-ATOMIC: job-list spawn refused (nested-job host?)");
                return;
            }
        };
        assert!(
            in_explicit_job(atom.handle, job.handle()),
            "atomically spawned child must be IN the job from creation"
        );
        // Negative control: the same binary, plain-spawned, is NOT a
        // member of our job (whatever ambient jobs exist).
        let plain = std::process::Command::new(&helper)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("control spawn works");
        assert!(
            !in_explicit_job(plain.as_raw_handle() as HANDLE, job.handle()),
            "plain-spawned child must NOT join our job — the query is vacuous"
        );
        // Bounded hygiene: both helpers exit on their own in milliseconds
        // (identity + exit); reap under a fixed bound, never .wait().
        let end = Instant::now() + Duration::from_secs(10);
        assert!(
            reap_bounded(&mut atom, end).is_ok(),
            "atomic child must exit promptly"
        );
        let mut plain = plain;
        assert!(
            reap_bounded(&mut plain, end).is_ok(),
            "control child must exit promptly"
        );
    }

    /// Marker-first fixture source (Windows): its FIRST action on startup
    /// is creating the `executed.marker` sibling — so marker absence after
    /// a probe proves ZERO candidate execution, not merely later cleanup.
    /// Afterwards it prints a valid identity and exits 0 (so that, were
    /// it ever executed uncontained, the probe would SUCCEED — making
    /// marker absence + probe failure jointly conclusive).
    #[cfg(windows)]
    const MARKER_RS: &str = r#"
fn main() {
    let exe = std::env::current_exe().expect("current exe");
    let marker = exe.parent().expect("exe dir").join("executed.marker");
    std::fs::write(&marker, std::process::id().to_string()).expect("marker write");
    println!("omen 0.9.0-preview.21 contract:0.8 commit:abc123");
}
"#;

    /// Build the marker fixture under the seam name
    /// (`force_atomic_unavailable.exe`) that trips
    /// [`atomic_failure_forced`]. Returns dir + exe + marker paths.
    #[cfg(windows)]
    fn marker_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker.rs"), MARKER_RS).unwrap();
        let out = std::process::Command::new("rustc")
            .arg("--edition=2021")
            .arg("--crate-name")
            .arg("omen_h_marker")
            .arg("marker.rs")
            .arg("-o")
            .arg("force_atomic_unavailable.exe")
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .output()
            .expect("rustc must be on PATH to build the zero-execution fixture");
        assert!(
            out.status.success(),
            "marker build failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let exe = dir.path().join("force_atomic_unavailable.exe");
        let marker = dir.path().join("executed.marker");
        (dir, exe, marker)
    }

    /// TEST 11 (Windows) — forced atomic-failure zero-execution proof,
    /// deterministic: the seam trips containment establishment, so the
    /// production probe must fail explicitly BEFORE executing the
    /// candidate. Proves, in order: (1) probe returns Err naming atomic
    /// containment unavailability; (2) the marker DOES NOT EXIST, i.e. the
    /// candidate's first instruction never ran — ZERO execution, not
    /// cleanup; (3) fail-fast return far below any deadline (no
    /// deadline-dependence); (4) no descendant can exist (nothing was ever
    /// spawned — verified by re-asserting marker absence after a
    /// scheduling grace period, with hygiene). Name-triggered seam: no
    /// global state, fully parallel-safe.
    #[cfg(windows)]
    #[test]
    fn forced_atomic_failure_never_executes_candidate() {
        let (_dir, exe, marker) = marker_fixture();
        assert!(
            !marker.exists(),
            "fixture setup must not pre-create the marker"
        );
        let t0 = Instant::now();
        let err =
            probe_candidate_with_budget(&exe, Duration::from_secs(15), Duration::from_secs(3))
                .expect_err("forced atomic failure must fail the probe");
        let total = t0.elapsed();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("atomic containment unavailable"),
            "probe must fail explicitly on containment establishment, got: {msg}"
        );
        assert!(
            !marker.exists(),
            "ZERO-EXECUTION VIOLATED: candidate ran despite failed containment"
        );
        assert!(
            total < Duration::from_secs(10),
            "fail-closed probe took {total:?} — must fail fast, not deadline-dependent"
        );
        // Scheduling grace: if anything had been spawned it would have
        // written the marker by now. Re-assert absence, then hygiene.
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            !marker.exists(),
            "ZERO-EXECUTION VIOLATED after grace period: candidate ran late"
        );
    }
}
