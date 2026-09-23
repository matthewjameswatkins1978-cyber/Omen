//! Measurement-integrity regression tests (IDO No. 2 POSIX repair).
//!
//! These prove dependent claims never exceed observed evidence:
//! no fabricated identity, reacquisition requires handoff, Ctrl-C delivery
//! is observed, `/proc` stopped ≠ Omen wait observation.

#![cfg(unix)]

use omen_compat::{
    EvidenceGrade, HandoffEvidence, InvariantId, InvariantOutcome, InvariantResult,
    ParseEvidenceError, PosixProcessIdentity, PosixTerminalState, PosixWaitState,
    SigintReceiptObservation, judge_job_has_distinct_pgrp, judge_shell_regains_tty_after_exit,
    judge_sigint_targets_foreground_job, judge_terminal_fg_is_job, judge_wait_observes_stopped,
    parse_posix_report, topology_results_missing_identity,
};

fn identity(pid: u32, pgrp: u32, sid: u32) -> PosixProcessIdentity {
    PosixProcessIdentity {
        pid,
        ppid: Some(1),
        pgrp: Some(pgrp),
        session_id: Some(sid),
        source: "test".into(),
    }
}

fn term(fg: u32) -> PosixTerminalState {
    PosixTerminalState {
        foreground_pgrp: Some(fg),
        session_id: Some(1),
        is_controlling_terminal: Some(true),
        rows: Some(24),
        cols: Some(80),
        termios: None,
        source: "test".into(),
    }
}

/// J.1 — Missing/malformed fixture identity must not produce STRONG topology.
#[test]
fn missing_fixture_identity_yields_no_strong_topology() {
    for err in [
        ParseEvidenceError::Missing,
        ParseEvidenceError::Malformed {
            detail: "pid missing".into(),
        },
    ] {
        let results = topology_results_missing_identity(&err);
        assert!(!results.is_empty());
        for r in &results {
            assert_ne!(
                r.evidence_grade,
                EvidenceGrade::Strong,
                "STRONG forbidden without identity: {r:?}"
            );
            assert!(
                !matches!(r.outcome, InvariantOutcome::Pass | InvariantOutcome::Fail),
                "PASS/FAIL topology forbidden without identity: {r:?}"
            );
        }
    }
}

/// J.1b — parse_posix_report never invents identity from partial JSON.
#[test]
fn parse_posix_report_rejects_malformed_and_missing() {
    assert_eq!(parse_posix_report(""), Err(ParseEvidenceError::Missing));
    assert_eq!(
        parse_posix_report("hello world"),
        Err(ParseEvidenceError::Missing)
    );
    let bad = r#"{"fixture":"posix-report","pid":0,"pgrp":-1,"sid":-1}"#;
    assert!(matches!(
        parse_posix_report(bad),
        Err(ParseEvidenceError::Malformed { .. })
    ));
    let missing_fields = r#"{"fixture":"posix-report","pid":42}"#;
    assert!(matches!(
        parse_posix_report(missing_fields),
        Err(ParseEvidenceError::Malformed { .. })
    ));
    let ok = r#"{"fixture":"posix-report","pid":42,"ppid":1,"pgrp":7,"sid":3}"#;
    let id = parse_posix_report(ok).expect("valid report");
    assert_eq!(id.pid, 42);
    assert_eq!(id.pgrp, Some(7));
    assert_eq!(id.session_id, Some(3));
    assert_ne!(id.source, "fallback_same_as_shell");
}

/// J.2 — No-handoff reacquisition (shell == job == fg) must not PASS.
#[test]
fn reacquisition_without_handoff_is_not_pass() {
    let shell = identity(100, 100, 100);
    let handoff = HandoffEvidence {
        shell_pgrp: 100,
        job_pgrp: 100,
        terminal_fg_during_job: 100,
    };
    assert!(!handoff.is_proven());
    let after = term(100);
    let r = judge_shell_regains_tty_after_exit(&handoff, &after, &shell);
    assert_ne!(r.outcome, InvariantOutcome::Pass, "{r:?}");
    assert_eq!(r.outcome, InvariantOutcome::Inconclusive);
}

/// J.3 — Real reacquisition positive control (distinct handoff + fg restore).
#[test]
fn reacquisition_with_proven_handoff_passes() {
    let shell = identity(100, 100, 100);
    let handoff = HandoffEvidence {
        shell_pgrp: 100,
        job_pgrp: 200,
        terminal_fg_during_job: 200,
    };
    assert!(handoff.is_proven());
    let after = term(100);
    let r = judge_shell_regains_tty_after_exit(&handoff, &after, &shell);
    assert_eq!(r.outcome, InvariantOutcome::Pass, "{r:?}");
    assert_eq!(r.evidence_grade, EvidenceGrade::Strong);
}

/// J.3b — Proven handoff but fg not restored → FAIL (not silent PASS).
#[test]
fn reacquisition_proven_handoff_but_fg_not_restored_fails() {
    let shell = identity(100, 100, 100);
    let handoff = HandoffEvidence {
        shell_pgrp: 100,
        job_pgrp: 200,
        terminal_fg_during_job: 200,
    };
    let after = term(200);
    let r = judge_shell_regains_tty_after_exit(&handoff, &after, &shell);
    assert_eq!(r.outcome, InvariantOutcome::Fail);
}

/// J.4 — VINTR without observed SIGINT receipt must not PASS targeting.
#[test]
fn sigint_targeting_without_delivery_is_not_pass() {
    let r = judge_sigint_targets_foreground_job(
        true,
        Some(100),
        Some(200),
        Some(200),
        &SigintReceiptObservation::InjectedNotObserved,
    );
    assert_ne!(r.outcome, InvariantOutcome::Pass, "{r:?}");
}

/// J.4b — Shared pgrp (no distinct job) cannot PASS even with receipt.
#[test]
fn sigint_targeting_shared_pgrp_is_not_pass() {
    let r = judge_sigint_targets_foreground_job(
        true,
        Some(100),
        Some(100),
        Some(100),
        &SigintReceiptObservation::Observed {
            source: "marker".into(),
        },
    );
    assert_ne!(r.outcome, InvariantOutcome::Pass, "{r:?}");
}

/// J.5 — Observed delivery + distinct job + fg == job + shell alive → PASS.
#[test]
fn sigint_targeting_with_observed_delivery_control_passes() {
    let r = judge_sigint_targets_foreground_job(
        true,
        Some(100),
        Some(200),
        Some(200),
        &SigintReceiptObservation::Observed {
            source: "fixture_marker".into(),
        },
    );
    assert_eq!(r.outcome, InvariantOutcome::Pass, "{r:?}");
    assert_eq!(r.evidence_grade, EvidenceGrade::Strong);
}

/// J.6 — /proc-style stopped fact does not PASS WAIT_OBSERVES_STOPPED_STATE.
#[test]
fn procfs_stopped_does_not_pass_wait_invariant() {
    let wait = PosixWaitState {
        pid: 200,
        exited: false,
        signaled: false,
        stopped: true,
        continued: false,
        exit_code: None,
        signal: None,
        source: "linux_proc".into(),
    };
    let r = judge_wait_observes_stopped(&wait);
    assert_ne!(r.outcome, InvariantOutcome::Pass, "{r:?}");
}

/// J.7 — Direct Compat waitpid(WUNTRACED) control passes the wait invariant.
#[test]
fn waitpid_untraced_control_passes_wait_invariant() {
    let wait = PosixWaitState {
        pid: 200,
        exited: false,
        signaled: false,
        stopped: true,
        continued: false,
        exit_code: None,
        signal: None,
        source: "waitpid_wuntraced".into(),
    };
    let r = judge_wait_observes_stopped(&wait);
    assert_eq!(r.outcome, InvariantOutcome::Pass, "{r:?}");
}

/// C — fg == job == shell is raw equality, not distinct job ownership PASS.
#[test]
fn terminal_fg_is_job_requires_distinct_pgrp() {
    let shell = identity(100, 100, 100);
    let job = identity(50, 100, 100);
    let t = term(100);
    let r = judge_terminal_fg_is_job(&t, &shell, &job);
    assert_ne!(r.outcome, InvariantOutcome::Pass, "{r:?}");
    assert_eq!(r.outcome, InvariantOutcome::Inconclusive);
}

/// Distinct job owning terminal → fg-is-job PASS.
#[test]
fn terminal_fg_is_distinct_job_passes() {
    let shell = identity(100, 100, 100);
    let job = identity(50, 200, 100);
    let t = term(200);
    let r = judge_terminal_fg_is_job(&t, &shell, &job);
    assert_eq!(r.outcome, InvariantOutcome::Pass, "{r:?}");
}

/// G — primary defect judge still FAILs STRONG when pgrps are equal.
#[test]
fn distinct_pgrp_equal_still_strong_fail() {
    let shell = identity(100, 100, 100);
    let job = identity(50, 100, 100);
    let r = judge_job_has_distinct_pgrp(&shell, &job);
    assert_eq!(r.outcome, InvariantOutcome::Fail);
    assert_eq!(r.evidence_grade, EvidenceGrade::Strong);
    assert_eq!(r.invariant, InvariantId::ShellJobHasDistinctProcessGroup);
}

/// Handoff builder refuses incomplete chains.
#[test]
fn handoff_try_from_observations_requires_all_links() {
    assert!(HandoffEvidence::try_from_observations(None, Some(1), Some(1)).is_none());
    assert!(HandoffEvidence::try_from_observations(Some(1), None, Some(1)).is_none());
    assert!(HandoffEvidence::try_from_observations(Some(1), Some(2), None).is_none());
    let h = HandoffEvidence::try_from_observations(Some(1), Some(2), Some(2)).unwrap();
    assert!(h.is_proven());
}

/// Shell death fails targeting regardless of delivery.
#[test]
fn sigint_targeting_shell_dead_fails() {
    let r = judge_sigint_targets_foreground_job(
        false,
        Some(100),
        Some(200),
        Some(200),
        &SigintReceiptObservation::InjectedNotObserved,
    );
    assert_eq!(r.outcome, InvariantOutcome::Fail);
}

/// InvariantResult sanity for missing-identity helper used by scenario A.
#[test]
fn missing_identity_results_are_inconclusive() {
    let results = topology_results_missing_identity(&ParseEvidenceError::Missing);
    assert_eq!(results.len(), 4);
    for r in results {
        assert!(matches!(
            r,
            InvariantResult {
                outcome: InvariantOutcome::Inconclusive,
                ..
            }
        ));
    }
}
