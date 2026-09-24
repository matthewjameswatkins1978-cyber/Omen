//! Invariant judgments from POSIX observations.
//!
//! OBSERVATION ≠ INVARIANT ≠ RESULT. Fixtures emit facts; only this layer
//! (and portable `RunOutcome` checks) produce PASS/FAIL/… results.
//!
//! Evidence-model laws (IDO No. 2 measurement-integrity repair):
//! - Never invent identity; missing fixture evidence stays missing.
//! - Reacquisition and distinct fg ownership require a proven prior handoff.
//! - VINTR injection ≠ SIGINT delivery; delivery must be observed.
//! - `/proc` stopped is a process fact, not Omen wait-path observation.

use crate::core::EvidenceGrade;
use crate::invariant::{InvariantId, InvariantResult};
use crate::posix::harness::TermiosSnapshot;
use crate::posix::observe::{
    HandoffEvidence, JobStoppedObserved, PosixProcessIdentity, PosixTerminalState, PosixWaitState,
    SigintReceiptObservation, SignalMaskEvidence,
};

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

/// Judge: while a **distinct** job owns the terminal, fg pgrp == job pgrp.
///
/// `terminal_fg == job_pgrp == shell_pgrp` is a raw equality fact only.
/// It does **not** prove distinct job ownership; that case is INCONCLUSIVE.
pub fn judge_terminal_fg_is_job(
    term: &PosixTerminalState,
    shell: &PosixProcessIdentity,
    job: &PosixProcessIdentity,
) -> InvariantResult {
    let inv = InvariantId::TerminalForegroundPgrpIsJob;
    match (term.foreground_pgrp, shell.pgrp, job.pgrp) {
        (Some(fg), Some(sp), Some(jp)) if sp == jp => InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            format!(
                "raw fact: terminal fg={fg} equals job pgrp={jp} equals shell pgrp={sp}; \
                 no distinct job ownership transition to prove"
            ),
        )
        .with_observations(vec![
            term.observation(),
            shell.observation(),
            job.observation(),
        ]),
        (Some(fg), Some(_sp), Some(jp)) if fg == jp => InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!("terminal fg={fg} equals distinct job pgrp={jp}"),
        )
        .with_observations(vec![
            term.observation(),
            shell.observation(),
            job.observation(),
        ]),
        (Some(fg), Some(_sp), Some(jp)) => InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!("terminal fg={fg} differs from job pgrp={jp}"),
        )
        .with_observations(vec![
            term.observation(),
            shell.observation(),
            job.observation(),
        ]),
        _ => InvariantResult::unavailable(inv, "terminal fg, shell pgrp, or job pgrp missing")
            .with_observations(vec![
                term.observation(),
                shell.observation(),
                job.observation(),
            ]),
    }
}

/// Judge: after job stop, shell regains terminal foreground before prompt.
///
/// PASS requires a proven prior distinct foreground handoff. When
/// `job_pgrp == shell_pgrp` (or handoff links are missing), reacquisition is
/// dependency-blocked / INCONCLUSIVE — never PASS from
/// `fg_after == shell_pgrp` alone.
pub fn judge_shell_regains_tty_after_stop(
    handoff: &HandoffEvidence,
    term_after: &PosixTerminalState,
    shell: &PosixProcessIdentity,
) -> InvariantResult {
    judge_reacquisition(
        InvariantId::ShellRegainsTtyAfterJobStop,
        "stop",
        handoff,
        term_after,
        shell,
    )
}

/// Judge: after job exit, shell regains terminal foreground before prompt.
///
/// Same handoff dependency as [`judge_shell_regains_tty_after_stop`].
pub fn judge_shell_regains_tty_after_exit(
    handoff: &HandoffEvidence,
    term_after: &PosixTerminalState,
    shell: &PosixProcessIdentity,
) -> InvariantResult {
    judge_reacquisition(
        InvariantId::ShellRegainsTtyAfterJobExit,
        "exit",
        handoff,
        term_after,
        shell,
    )
}

fn judge_reacquisition(
    inv: InvariantId,
    event: &str,
    handoff: &HandoffEvidence,
    term_after: &PosixTerminalState,
    shell: &PosixProcessIdentity,
) -> InvariantResult {
    if !handoff.is_proven() {
        let raw = match (term_after.foreground_pgrp, shell.pgrp) {
            (Some(fg), Some(sp)) => format!(
                "raw current ownership after {event}: terminal fg={fg}, shell pgrp={sp} \
                 (equal={})",
                fg == sp
            ),
            _ => format!("raw current ownership after {event}: incomplete fg/shell pgrp"),
        };
        return InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            format!(
                "dependency blocked: no proven prior distinct foreground handoff \
                 (shell_pgrp={} job_pgrp={} fg_during_job={}); {raw}",
                handoff.shell_pgrp, handoff.job_pgrp, handoff.terminal_fg_during_job
            ),
        );
    }
    let shell_pgrp = shell.pgrp.unwrap_or(handoff.shell_pgrp);
    match term_after.foreground_pgrp {
        Some(fg) if fg == shell_pgrp => InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!(
                "after {event}, terminal fg={fg} equals shell pgrp={shell_pgrp}; \
                 prior distinct handoff proven (job_pgrp={})",
                handoff.job_pgrp
            ),
        ),
        Some(fg) => InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!(
                "after {event}, terminal fg={fg} still not shell pgrp={shell_pgrp} \
                 (prior distinct handoff was proven)"
            ),
        ),
        None => InvariantResult::unavailable(
            inv,
            format!("terminal fg missing after {event} despite proven handoff"),
        ),
    }
}

/// Independent process-stopped fact (procfs / kernel state).
///
/// Grade is Strong for "job IS stopped". This is never substituted for
/// [`judge_wait_observes_stopped`].
pub fn judge_job_stopped_observed(
    obs: &JobStoppedObserved,
) -> (InvariantId, EvidenceGrade, String) {
    // Reported as a raw fact alongside scenario results; not an invariant ID.
    (
        InvariantId::WaitObservesStoppedState,
        EvidenceGrade::Strong,
        format!(
            "process stopped fact STRONG: pid={} source={} (not Omen wait-path evidence)",
            obs.pid, obs.source
        ),
    )
}

/// Judge: Omen's wait/job-control path observed STOPPED (not merely `/proc`).
///
/// `source` must name a wait-path mechanism (e.g. `waitpid_wuntraced`).
/// Procfs process-state sources cannot PASS this invariant.
pub fn judge_wait_observes_stopped(wait: &PosixWaitState) -> InvariantResult {
    let inv = InvariantId::WaitObservesStoppedState;
    let wait_path = wait_source_is_wait_path(&wait.source);
    if wait.stopped && wait_path {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!(
                "wait path {} reports stopped for pid={}",
                wait.source, wait.pid
            ),
        )
        .with_observations(vec![wait.observation()])
    } else if wait.stopped && !wait_path {
        InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            format!(
                "process stopped via {} for pid={}; not Omen wait-path observation",
                wait.source, wait.pid
            ),
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

fn wait_source_is_wait_path(source: &str) -> bool {
    let s = source.to_ascii_lowercase();
    s.contains("waitpid") || s.contains("wait_path") || s.contains("wuntraced")
}

/// Judge: terminal-generated SIGINT targeted the foreground job.
///
/// PASS requires **all** of:
/// - job pid/pgrp observed (caller passes `job_pgrp`),
/// - job pgrp distinct from shell,
/// - terminal fg == actual job pgrp before injection,
/// - SIGINT delivery independently observed (not merely VINTR injected),
/// - shell survives.
///
/// Never hard-code delivery. Injection alone is not delivery.
pub fn judge_sigint_targets_foreground_job(
    shell_alive: bool,
    shell_pgrp: Option<u32>,
    job_pgrp: Option<u32>,
    fg_before: Option<u32>,
    receipt: &SigintReceiptObservation,
) -> InvariantResult {
    let inv = InvariantId::TerminalSigintTargetsForegroundJob;

    if !shell_alive {
        return InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            "shell did not survive foreground SIGINT",
        );
    }

    let (Some(sp), Some(jp), Some(fg)) = (shell_pgrp, job_pgrp, fg_before) else {
        return InvariantResult::unavailable(
            inv,
            format!(
                "routing inputs incomplete: shell_pgrp={shell_pgrp:?} job_pgrp={job_pgrp:?} \
                 fg_before={fg_before:?}"
            ),
        );
    };

    if sp == jp {
        // Distinct ownership never established; cannot claim targeting PASS.
        return match receipt {
            SigintReceiptObservation::Observed { source } => InvariantResult::inconclusive(
                inv,
                EvidenceGrade::Partial,
                format!(
                    "SIGINT receipt observed via {source}, but shell pgrp={sp} equals \
                         job pgrp={jp}: no distinct foreground job to target (raw equality only)"
                ),
            ),
            _ => InvariantResult::inconclusive(
                inv,
                EvidenceGrade::Partial,
                format!(
                    "shell pgrp={sp} equals job pgrp={jp}: no distinct foreground job; \
                     delivery state={receipt:?}"
                ),
            ),
        };
    }

    match receipt {
        SigintReceiptObservation::Observed { source } if fg == jp => InvariantResult::pass(
            inv,
            EvidenceGrade::Strong,
            format!(
                "SIGINT delivered to job pgrp {jp} (shell pgrp {sp} survived); receipt via {source}"
            ),
        ),
        SigintReceiptObservation::Observed { source } => InvariantResult::fail(
            inv,
            EvidenceGrade::Strong,
            format!("SIGINT receipt via {source} but terminal fg={fg} != job pgrp={jp}"),
        ),
        SigintReceiptObservation::InjectedNotObserved => InvariantResult::inconclusive(
            inv,
            EvidenceGrade::Partial,
            format!(
                "VINTR injected; SIGINT delivery not observed (fg_before={fg} job_pgrp={jp} \
                 shell_pgrp={sp})"
            ),
        ),
        SigintReceiptObservation::Unavailable { reason } => InvariantResult::unavailable(
            inv,
            format!("SIGINT delivery cannot be observed: {reason}"),
        ),
    }
}

/// Judge: foreground SIGINT termination does not kill the shell.
///
/// Independent of child SIGINT receipt: shell survival alone can PASS.
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
/// Availability is explicit (D2-018): unavailable evidence is UNAVAILABLE,
/// never an empty measured set that could PASS. Only asserts signals the
/// fixture actually measured.
pub fn judge_child_signal_mask_unblocked(
    evidence: &SignalMaskEvidence,
    relevant: &[&str],
) -> InvariantResult {
    let inv = InvariantId::ChildSignalMaskUnblockedBeforeExec;
    if !evidence.available {
        return InvariantResult::unavailable(
            inv,
            format!(
                "signal-mask evidence unavailable (source={}): not an empty measurement",
                evidence.source
            ),
        );
    }
    let Some(blocked) = evidence.blocked.as_deref() else {
        return InvariantResult::unavailable(
            inv,
            format!(
                "signal-mask marked available but blocked set missing (source={})",
                evidence.source
            ),
        );
    };
    let relevant_lower: Vec<String> = relevant.iter().map(|s| s.to_string()).collect();
    let leaked: Vec<&String> = blocked
        .iter()
        .filter(|b| relevant_lower.iter().any(|r| r.eq_ignore_ascii_case(b)))
        .collect();
    if leaked.is_empty() {
        InvariantResult::pass(
            inv,
            EvidenceGrade::Partial,
            format!(
                "fixture-reported blocked set {blocked:?} (source={}) does not include {relevant_lower:?}",
                evidence.source
            ),
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

/// Topology judgments when fixture identity is missing/malformed.
///
/// Never fabricates a `PosixProcessIdentity`. Never returns STRONG PASS/FAIL
/// topology results — only missing-evidence outcomes.
pub fn topology_results_missing_identity(
    parse_error: &crate::posix::observe::ParseEvidenceError,
) -> Vec<InvariantResult> {
    let detail = parse_error.to_string();
    vec![
        InvariantResult::inconclusive(
            InvariantId::ShellJobSharesShellSession,
            EvidenceGrade::Partial,
            format!("missing fixture identity; no STRONG topology: {detail}"),
        ),
        InvariantResult::inconclusive(
            InvariantId::ShellJobHasDistinctProcessGroup,
            EvidenceGrade::Partial,
            format!("missing fixture identity; no STRONG topology: {detail}"),
        ),
        InvariantResult::inconclusive(
            InvariantId::TerminalForegroundPgrpIsJob,
            EvidenceGrade::Partial,
            format!("missing fixture identity; no STRONG topology: {detail}"),
        ),
        InvariantResult::inconclusive(
            InvariantId::ShellRegainsTtyAfterJobExit,
            EvidenceGrade::Partial,
            format!("missing fixture identity; reacquisition dependency blocked: {detail}"),
        ),
    ]
}
