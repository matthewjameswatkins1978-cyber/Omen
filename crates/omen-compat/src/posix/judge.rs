//! Invariant judgments from POSIX observations.
//!
//! OBSERVATION ≠ INVARIANT ≠ RESULT. Fixtures emit facts; only this layer
//! (and portable `RunOutcome` checks) produce PASS/FAIL/… results.

use crate::core::EvidenceGrade;
use crate::invariant::{InvariantId, InvariantResult};
use crate::posix::harness::TermiosSnapshot;
use crate::posix::observe::{PosixProcessIdentity, PosixTerminalState, PosixWaitState};

/// Judge: foreground job remains in the shell's session.
pub fn judge_job_shares_session(
    shell: &PosixProcessIdentity,
    job: &PosixProcessIdentity,
) -> InvariantResult {
    let inv = InvariantId::ShellJobSharesShellSession;
    match (shell.session_id, job.session_id) {
        (Some(ss), Some(js)) if ss == js => InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("shell sid={ss} equals job sid={js}"),
        )
        .with_observations(vec![shell.observation(), job.observation()]),
        (Some(ss), Some(js)) => InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!("shell sid={ss} differs from job sid={js}"),
        )
        .with_observations(vec![shell.observation(), job.observation()]),
        _ => InvariantResult::unavailable(inv, "session id missing for shell or job")
            .with_observations(vec![shell.observation(), job.observation()]),
    }
}

/// Judge: job pgrp is distinct from the shell's pgrp.
pub fn judge_job_has_distinct_pgrp(
    shell: &PosixProcessIdentity,
    job: &PosixProcessIdentity,
) -> InvariantResult {
    let inv = InvariantId::ShellJobHasDistinctProcessGroup;
    match (shell.pgrp, job.pgrp) {
        (Some(sp), Some(jp)) if sp != jp => InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("shell pgrp={sp} differs from job pgrp={jp}"),
        )
        .with_observations(vec![shell.observation(), job.observation()]),
        (Some(sp), Some(jp)) => InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!("shell pgrp={sp} equals job pgrp={jp} (no job-control split)"),
        )
        .with_observations(vec![shell.observation(), job.observation()]),
        _ => InvariantResult::unavailable(inv, "pgrp missing for shell or job")
            .with_observations(vec![shell.observation(), job.observation()]),
    }
}

/// Judge: while the job owns the terminal, fg pgrp == job pgrp.
pub fn judge_terminal_fg_is_job(
    term: &PosixTerminalState,
    job: &PosixProcessIdentity,
) -> InvariantResult {
    let inv = InvariantId::TerminalForegroundPgrpIsJob;
    match (term.foreground_pgrp, job.pgrp) {
        (Some(fg), Some(jp)) if fg == jp => InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("terminal fg={fg} equals job pgrp={jp}"),
        )
        .with_observations(vec![term.observation(), job.observation()]),
        (Some(fg), Some(jp)) => InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!("terminal fg={fg} differs from job pgrp={jp}"),
        )
        .with_observations(vec![term.observation(), job.observation()]),
        _ => InvariantResult::unavailable(inv, "terminal fg or job pgrp missing")
            .with_observations(vec![term.observation(), job.observation()]),
    }
}

/// Judge: after job stop, shell regains terminal foreground before prompt.
pub fn judge_shell_regains_tty_after_stop(
    term_after: &PosixTerminalState,
    shell: &PosixProcessIdentity,
) -> InvariantResult {
    let inv = InvariantId::ShellRegainsTtyAfterJobStop;
    match (term_after.foreground_pgrp, shell.pgrp) {
        (Some(fg), Some(sp)) if fg == sp => InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("after stop, terminal fg={fg} equals shell pgrp={sp}"),
        )
        .with_observations(vec![term_after.observation(), shell.observation()]),
        (Some(fg), Some(sp)) => InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!("after stop, terminal fg={fg} still not shell pgrp={sp}"),
        )
        .with_observations(vec![term_after.observation(), shell.observation()]),
        _ => InvariantResult::unavailable(inv, "fg or shell pgrp missing after stop")
            .with_observations(vec![term_after.observation(), shell.observation()]),
    }
}

/// Judge: after job exit, shell regains terminal foreground before prompt.
pub fn judge_shell_regains_tty_after_exit(
    term_after: &PosixTerminalState,
    shell: &PosixProcessIdentity,
) -> InvariantResult {
    let inv = InvariantId::ShellRegainsTtyAfterJobExit;
    match (term_after.foreground_pgrp, shell.pgrp) {
        (Some(fg), Some(sp)) if fg == sp => InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("after exit, terminal fg={fg} equals shell pgrp={sp}"),
        )
        .with_observations(vec![term_after.observation(), shell.observation()]),
        (Some(fg), Some(sp)) => InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!("after exit, terminal fg={fg} still not shell pgrp={sp}"),
        )
        .with_observations(vec![term_after.observation(), shell.observation()]),
        _ => InvariantResult::unavailable(inv, "fg or shell pgrp missing after exit")
            .with_observations(vec![term_after.observation(), shell.observation()]),
    }
}

/// Judge: a stopped foreground child is observed as STOPPED (not only exit).
pub fn judge_wait_observes_stopped(wait: &PosixWaitState) -> InvariantResult {
    let inv = InvariantId::WaitObservesStoppedState;
    if wait.stopped {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("wait status reports stopped for pid={}", wait.pid),
        )
        .with_observations(vec![wait.observation()])
    } else if wait.exited || wait.signaled {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!(
                "child reached terminal exit/signal without observed stop (pid={})",
                wait.pid
            ),
        )
        .with_observations(vec![wait.observation()])
    } else {
        InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            "no stopped and no terminal wait observed",
        )
        .with_observations(vec![wait.observation()])
    }
}

/// Judge: terminal-generated SIGINT targets the foreground job, not the shell.
pub fn judge_sigint_targets_foreground_job(
    job_signaled: bool,
    shell_alive: bool,
    shell_pgrp: Option<u32>,
    job_pgrp: Option<u32>,
    fg_before: Option<u32>,
) -> InvariantResult {
    let inv = InvariantId::TerminalSigintTargetsForegroundJob;
    let routing_ok = job_signaled
        && shell_alive
        && shell_pgrp.is_some_and(|s| job_pgrp.is_some_and(|j| s != j))
        && fg_before == job_pgrp;
    if routing_ok {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!(
                "Ctrl-C delivered to job pgrp {job_pgrp:?} (shell pgrp {shell_pgrp:?} survived)"
            ),
        )
    } else if !shell_alive {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            "shell did not survive foreground SIGINT",
        )
    } else {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Partial,
            format!(
                "job_signaled={job_signaled} shell_alive={shell_alive} fg_before={fg_before:?} job_pgrp={job_pgrp:?}"
            ),
        )
    }
}

/// Judge: foreground SIGINT termination does not kill the shell.
pub fn judge_shell_survives_foreground_sigint(
    shell_alive: bool,
    shell_pid: u32,
) -> InvariantResult {
    let inv = InvariantId::ShellSurvivesForegroundJobSigint;
    if shell_alive {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("shell pid={shell_pid} still alive after foreground SIGINT"),
        )
    } else {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!("shell pid={shell_pid} terminated after foreground SIGINT"),
        )
    }
}

/// Judge: signal termination retains the exact signal number.
pub fn judge_signal_faithful_exit(
    observed: Option<i32>,
    expected_signal: i32,
    observed_as_signal: bool,
) -> InvariantResult {
    let inv = InvariantId::ExitStatusPreservesSignalNumber;
    if observed_as_signal && observed == Some(expected_signal) {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("exit identity preserves signal {expected_signal}"),
        )
    } else {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!(
                "expected signal {expected_signal}, observed signal={observed:?} as_signal={observed_as_signal}"
            ),
        )
    }
}

/// Judge: relevant inherited blocked signals do not poison the exec'd child.
///
/// Only asserts signals the fixture actually measured.
pub fn judge_child_signal_mask_unblocked(blocked: &[String], relevant: &[&str]) -> InvariantResult {
    let inv = InvariantId::ChildSignalMaskUnblockedBeforeExec;
    let relevant_lower: Vec<String> = relevant.iter().map(|s| s.to_string()).collect();
    let leaked: Vec<&String> = blocked
        .iter()
        .filter(|b| relevant_lower.iter().any(|r| r.eq_ignore_ascii_case(b)))
        .collect();
    if leaked.is_empty() {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Partial,
            format!("fixture-reported blocked set {blocked:?} does not include {relevant_lower:?}"),
        )
    } else {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Partial,
            format!("fixture reported blocked relevant signals: {leaked:?}"),
        )
    }
}

/// Judge: resize produced both a new size and asynchronous SIGWINCH delivery.
pub fn judge_sigwinch_async_delivered(
    size_changed: bool,
    signal_observed: bool,
    before: (u16, u16),
    after: (u16, u16),
) -> InvariantResult {
    let inv = InvariantId::SigwinchAsyncDeliveredToForegroundPgrpOnResize;
    if size_changed && signal_observed {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("winsize {before:?} -> {after:?}; fixture observed SIGWINCH asynchronously"),
        )
    } else if size_changed {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Partial,
            format!("winsize changed {before:?} -> {after:?} but fixture did not observe SIGWINCH"),
        )
    } else {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!("winsize did not change (still {before:?})"),
        )
    }
}

/// Judge: shell termios snapshot restored after abnormal child exit.
pub fn judge_termios_restored(pre: &TermiosSnapshot, post: &TermiosSnapshot) -> InvariantResult {
    let inv = InvariantId::ShellTermiosSnapshotRestoredAfterAbnormalChildExit;
    if pre == post {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!(
                "selected termios restored: icanon={} echo={} isig={}",
                pre.icanon, pre.echo, pre.isig
            ),
        )
    } else {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!("termios mismatch pre={pre:?} post={post:?}"),
        )
    }
}

/// Judge: no reapable zombie direct child of the shell after the lifecycle.
pub fn judge_no_zombie_children(
    shell_pid: u32,
    zombie_direct_children: &[u32],
    evidence_available: bool,
) -> InvariantResult {
    let inv = InvariantId::NoZombieChildrenOfShell;
    if !evidence_available {
        return InvariantResult::unavailable(
            inv,
            "independent zombie observation unavailable on this platform",
        );
    }
    if zombie_direct_children.is_empty() {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("no zombie direct children of shell pid={shell_pid}"),
        )
    } else {
        InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!("zombie direct children of shell {shell_pid}: {zombie_direct_children:?}"),
        )
    }
}
