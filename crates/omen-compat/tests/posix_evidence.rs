//! Measurement-integrity regression tests (IDO No. 2 POSIX repair).
//!
//! These prove dependent claims never exceed observed evidence:
//! no fabricated identity, reacquisition requires handoff, Ctrl-C delivery
//! is observed, `/proc` stopped ≠ Omen wait observation.

#![cfg(unix)]

use omen_compat::{
    EvidenceGrade, HandoffEvidence, InvariantId, InvariantOutcome, InvariantResult,
    ParseEvidenceError, PosixProcessIdentity, PosixTerminalState, PosixWaitState,
    SigintReceiptObservation, SignalMaskEvidence, derive_controlling_terminal,
    judge_child_signal_mask_unblocked, judge_job_has_distinct_pgrp, judge_job_shares_session,
    judge_shell_regains_tty_after_exit, judge_sigint_targets_foreground_job,
    judge_terminal_fg_is_job, judge_wait_observes_stopped, observe_shell_identity,
    parse_posix_report, parse_signal_mask_evidence, topology_results_missing_identity,
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
        // Test inputs never hard-code product truth; ctty is observed only
        // in live helpers. Judge tests below do not read this field.
        is_controlling_terminal: None,
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

/// Shell identity sources are observation mechanisms only — never a
/// synthetic session-leader fallback (D2-018). Built at runtime so a
/// literal search for the forbidden source name finds no product code.
#[test]
fn shell_identity_source_is_observation_only() {
    let forbidden = ["harness", "session", "leader", "fallback"].join("_");
    let id = observe_shell_identity(std::process::id());
    assert!(
        !id.source.contains(&forbidden),
        "synthetic fallback forbidden: {}",
        id.source
    );
    assert!(
        id.source.starts_with("rustix_get")
            || id.source.contains("linux_proc_stat")
            || id.source == "identity_observation_failed",
        "source must name an observation mechanism: {}",
        id.source
    );
}

/// Shell identity: failed observation leaves pgrp/sid missing (not synthesized).
#[test]
fn shell_identity_failed_observation_leaves_pgrp_sid_missing() {
    // Unlikely-to-exist high pid: getpgid/getsid fail; Linux /proc absent.
    let id = observe_shell_identity(u32::MAX - 16);
    assert!(
        id.pgrp.is_none(),
        "failed observation must not invent pgrp: {id:?}"
    );
    assert!(
        id.session_id.is_none(),
        "failed observation must not invent sid: {id:?}"
    );
    assert_eq!(id.source, "identity_observation_failed");
}

/// Shell identity: successful observation records live process topology.
#[test]
fn shell_identity_observed_pgrp_and_sid() {
    let id = observe_shell_identity(std::process::id());
    assert!(id.pgrp.is_some(), "getpgid must observe pgrp: {id:?}");
    assert!(id.session_id.is_some(), "getsid must observe sid: {id:?}");
}

/// Missing shell topology cannot produce STRONG topology results.
#[test]
fn missing_shell_topology_cannot_produce_strong_topology() {
    let shell = PosixProcessIdentity {
        pid: 100,
        ppid: None,
        pgrp: None,
        session_id: None,
        source: "identity_observation_failed".into(),
    };
    let job = identity(50, 200, 100);
    for r in [
        judge_job_shares_session(&shell, &job),
        judge_job_has_distinct_pgrp(&shell, &job),
    ] {
        assert_ne!(
            r.evidence_grade,
            EvidenceGrade::Strong,
            "STRONG forbidden without shell topology: {r:?}"
        );
        assert_ne!(
            r.outcome,
            InvariantOutcome::Pass,
            "PASS forbidden without shell topology: {r:?}"
        );
        assert_ne!(
            r.outcome,
            InvariantOutcome::Fail,
            "FAIL forbidden without shell topology: {r:?}"
        );
    }
}

/// A — measured empty blocked set is a semantic PASS (not unavailable).
#[test]
fn signal_mask_measured_empty_passes() {
    let ev = SignalMaskEvidence::measured("proc_self_status", vec![], vec![], vec![]);
    let r = judge_child_signal_mask_unblocked(&ev, &["SIGINT"]);
    assert_eq!(r.outcome, InvariantOutcome::Pass, "{r:?}");
    assert_eq!(r.evidence_grade, EvidenceGrade::Partial);
}

/// B — measured relevant blocked signal FAILs.
#[test]
fn signal_mask_measured_blocked_fails() {
    let ev =
        SignalMaskEvidence::measured("proc_self_status", vec!["SIGINT".into()], vec![], vec![]);
    let r = judge_child_signal_mask_unblocked(&ev, &["SIGINT"]);
    assert_eq!(r.outcome, InvariantOutcome::Fail, "{r:?}");
}

/// C — unavailable evidence is UNAVAILABLE, never empty-measured PASS.
#[test]
fn signal_mask_unavailable_is_unavailable_not_pass() {
    let ev = SignalMaskEvidence::unavailable("unavailable");
    let r = judge_child_signal_mask_unblocked(&ev, &["SIGINT"]);
    assert_eq!(r.outcome, InvariantOutcome::Unavailable, "{r:?}");
    assert_ne!(r.outcome, InvariantOutcome::Pass);
}

/// D — fixture JSON parser preserves availability through parse.
#[test]
fn signal_mask_parser_preserves_availability() {
    let available = r#"{"signal_masks_available":true,"signal_masks_source":"proc_self_status","blocked":[],"ignored":[],"caught":[]}"#;
    let ev = parse_signal_mask_evidence(available);
    assert!(ev.available);
    assert_eq!(ev.blocked, Some(vec![]));
    let r = judge_child_signal_mask_unblocked(&ev, &["SIGINT"]);
    assert_eq!(r.outcome, InvariantOutcome::Pass);

    let unavailable = r#"{"signal_masks_available":false,"signal_masks_source":"unavailable","blocked":null,"ignored":null,"caught":null}"#;
    let ev = parse_signal_mask_evidence(unavailable);
    assert!(!ev.available);
    assert!(ev.blocked.is_none());
    let r = judge_child_signal_mask_unblocked(&ev, &["SIGINT"]);
    assert_eq!(r.outcome, InvariantOutcome::Unavailable);

    // Missing availability field is unavailable, not empty measured.
    let missing = r#"{"blocked":[]}"#;
    let ev = parse_signal_mask_evidence(missing);
    assert!(!ev.available);
    let r = judge_child_signal_mask_unblocked(&ev, &["SIGINT"]);
    assert_eq!(r.outcome, InvariantOutcome::Unavailable);

    // available=true with blocked=null is not a measurement (D2-018).
    let available_null_blocked = r#"{"signal_masks_available":true,"signal_masks_source":"proc_self_status","blocked":null}"#;
    let ev = parse_signal_mask_evidence(available_null_blocked);
    assert!(
        !ev.available,
        "null blocked must not become measured: {ev:?}"
    );
    let r = judge_child_signal_mask_unblocked(&ev, &["SIGINT"]);
    assert_eq!(r.outcome, InvariantOutcome::Unavailable);
}

/// Controlling terminal: matching observed sessions → Some(true).
#[test]
fn controlling_terminal_observed_match_is_true() {
    assert_eq!(
        derive_controlling_terminal(Some(100), Some(100)),
        Some(true)
    );
}

/// Controlling terminal: failed observation → None (never hard-coded true).
#[test]
fn controlling_terminal_unavailable_observation_is_none() {
    assert_eq!(derive_controlling_terminal(None, Some(100)), None);
    assert_eq!(derive_controlling_terminal(Some(100), None), None);
    assert_eq!(derive_controlling_terminal(None, None), None);
    // Unequal sessions are not over-interpreted without negative evidence.
    assert_eq!(derive_controlling_terminal(Some(100), Some(200)), None);
}
