//! Tier POSIX OMEN — real Omen shell under the calibrated PTY.
//!
//! Product compatibility results may be FAIL / INCONCLUSIVE / OPEN_DEFECT
//! while the harness itself remains green. Outer watchdog bounds every
//! scenario. Production Omen is never repaired here.
//!
//! Evidence model (D2-017): no fabricated job identity; reacquisition
//! requires proven prior handoff; VINTR ≠ SIGINT delivery; `/proc` stopped
//! is not Omen wait-path observation.

#![cfg(unix)]

use omen_compat::{
    EvidenceGrade, HandoffEvidence, InvariantId, InvariantOutcome, InvariantResult,
    JobStoppedObserved, ParseEvidenceError, PosixProcessIdentity, PosixTerminalState, PtySession,
    PtyWinsize, SigintReceiptObservation, TermiosSnapshot, judge_child_signal_mask_unblocked,
    judge_job_has_distinct_pgrp, judge_job_shares_session, judge_no_zombie_children,
    judge_shell_regains_tty_after_exit, judge_shell_regains_tty_after_stop,
    judge_shell_survives_foreground_sigint, judge_sigint_targets_foreground_job,
    judge_sigwinch_async_delivered, judge_terminal_fg_is_job, judge_termios_restored,
    parse_posix_report, topology_results_missing_identity,
};
use std::path::PathBuf;
use std::time::Duration;

const SCENARIO_BUDGET: Duration = Duration::from_secs(25);

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn gremlin_exe() -> PathBuf {
    static EXE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    EXE.get_or_init(|| {
        // Prefer a space-free path so interactive command lines tokenize cleanly.
        let stable = PathBuf::from("/tmp/omen_compat_gremlin");
        let mut path = std::env::current_exe().expect("current_exe");
        path.pop();
        if path.ends_with("deps") {
            path.pop();
        }
        let name = "omen-gremlin";
        let candidates = [
            path.join(name),
            workspace_root().join("target").join("debug").join(name),
        ];
        let mut found = None;
        for c in &candidates {
            if c.exists() {
                found = Some(c.clone());
                break;
            }
        }
        if found.is_none() {
            let build = std::process::Command::new("cargo")
                .args(["build", "-p", "omen-test-fixtures", "--bin", "omen-gremlin"])
                .current_dir(workspace_root())
                .status()
                .expect("gremlin build");
            assert!(build.success());
            for c in &candidates {
                if c.exists() {
                    found = Some(c.clone());
                    break;
                }
            }
        }
        let found = found.expect("omen-gremlin");
        let _ = std::fs::copy(&found, &stable);
        if stable.exists() { stable } else { found }
    })
    .clone()
}

fn omen_exe() -> Option<PathBuf> {
    static EXE: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    EXE.get_or_init(|| {
        let mut path = std::env::current_exe().ok()?;
        path.pop();
        if path.ends_with("deps") {
            path.pop();
        }
        let name = if cfg!(windows) { "omen.exe" } else { "omen" };
        let debug = workspace_root().join("target").join("debug").join(name);
        if debug.exists() {
            return Some(debug);
        }
        let local = path.join(name);
        if local.exists() {
            return Some(local);
        }
        None
    })
    .clone()
}

/// Seed HumanSettings so the appearance chooser cannot block PTY tests.
fn seed_human_settings(home: &std::path::Path) {
    let cfg = home.join(".config").join("omen");
    let _ = std::fs::create_dir_all(&cfg);
    let _ = std::fs::write(
        cfg.join("config.toml"),
        "theme = \"omen\"\ndensity = \"normal\"\nepigraph = true\n",
    );
}

fn spawn_omen(cwd: &std::path::Path) -> Option<PtySession> {
    let exe = omen_exe()?;
    seed_human_settings(cwd);
    let env = [
        ("HOME".to_string(), cwd.display().to_string()),
        (
            "XDG_CONFIG_HOME".to_string(),
            cwd.join(".config").display().to_string(),
        ),
        ("TERM".to_string(), "xterm-256color".to_string()),
        ("NO_COLOR".to_string(), "1".to_string()),
        ("COLUMNS".to_string(), "80".to_string()),
        ("LINES".to_string(), "24".to_string()),
    ];
    PtySession::spawn_session_child(
        exe,
        &[],
        true,
        &env,
        Some(cwd),
        PtyWinsize::default(),
        SCENARIO_BUDGET,
    )
    .ok()
}

fn wait_prompt(session: &mut PtySession) -> bool {
    match session.wait_for_text("\\O/", Duration::from_secs(15)) {
        Ok(t) => t.contains("\\O/"),
        Err(_) => false,
    }
}

fn shell_identity(session: &PtySession) -> PosixProcessIdentity {
    let pid = session.child_pid();
    #[cfg(target_os = "linux")]
    {
        if let Some(id) = omen_compat::read_process_identity(pid) {
            return id;
        }
    }
    // Harness established setsid + tcsetpgrp(self) before exec: session
    // leader identity is a harness fact, not a fabricated job identity.
    PosixProcessIdentity {
        pid,
        ppid: None,
        pgrp: Some(pid),
        session_id: Some(pid),
        source: "harness_session_leader_fallback".into(),
    }
}

fn terminal_state(session: &PtySession) -> PosixTerminalState {
    let fg = session.observe_foreground_pgrp().ok();
    let sid = session.observe_terminal_session().ok();
    let ws = session.observe_winsize().ok();
    let termios = session.observe_termios_snapshot().ok();
    PosixTerminalState {
        foreground_pgrp: fg,
        session_id: sid,
        is_controlling_terminal: Some(true),
        rows: ws.map(|w| w.rows),
        cols: ws.map(|w| w.cols),
        termios,
        source: "rustix_tcgetpgrp_tcgetsid".into(),
    }
}

/// Wait for needle; returns false if bound-out without the needle.
fn wait_has(session: &mut PtySession, needle: &str, budget: Duration) -> bool {
    match session.wait_for_text(needle, budget) {
        Ok(t) => t.contains(needle),
        Err(_) => false,
    }
}

fn write_line(session: &mut PtySession, line: &str) {
    let mut s = line.to_string();
    s.push('\n');
    let _ = session.write_master(s.as_bytes());
}

fn print_report(scenario: &str, results: &[InvariantResult]) {
    println!("SCENARIO {scenario}");
    for r in results {
        println!(
            "{}\t{}\t{}\t{}",
            r.invariant.stable_id(),
            r.outcome.stable_id(),
            r.evidence_grade.stable_id(),
            r.reason
        );
    }
}

/// Scenario A — foreground process topology through interactive handoff.
#[test]
fn omen_a_foreground_process_topology() {
    let Some(mut omen) = spawn_omen(&workspace_root()) else {
        eprintln!("SKIP: omen binary not built");
        return;
    };
    assert!(wait_prompt(&mut omen), "prompt readiness");
    let shell = shell_identity(&omen);

    let gremlin = gremlin_exe();
    write_line(
        &mut omen,
        &format!("--interactive {} --posix-report", gremlin.display()),
    );

    let ready = wait_has(&mut omen, "OMEN_COMPAT_READY", Duration::from_secs(15));
    let mut results = Vec::new();
    if !ready {
        results.push(InvariantResult::new(
            InvariantId::ShellJobHasDistinctProcessGroup,
            InvariantOutcome::OpenDefect,
            EvidenceGrade::Partial,
            "interactive handoff path did not produce fixture READY within bound; cannot prove distinct job pgrp",
        ));
        print_report("foreground-job", &results);
        assert!(omen.child_pid() > 0);
        return;
    }

    let text = omen.transcript().as_lossy();
    let job = match parse_posix_report(&text) {
        Ok(job) => job,
        Err(err) => {
            // Missing/malformed identity: never invent pid/pgrp/sid.
            assert!(matches!(
                err,
                ParseEvidenceError::Missing | ParseEvidenceError::Malformed { .. }
            ));
            results.extend(topology_results_missing_identity(&err));
            print_report("foreground-job", &results);
            // No STRONG topology result may be present.
            for r in &results {
                assert_ne!(
                    r.evidence_grade,
                    EvidenceGrade::Strong,
                    "STRONG topology result forbidden without fixture identity: {r:?}"
                );
            }
            assert!(omen.child_pid() > 0);
            return;
        }
    };

    let term_job = terminal_state(&omen);
    results.push(judge_job_shares_session(&shell, &job));
    results.push(judge_job_has_distinct_pgrp(&shell, &job));
    results.push(judge_terminal_fg_is_job(&term_job, &shell, &job));

    let handoff =
        HandoffEvidence::try_from_observations(shell.pgrp, job.pgrp, term_job.foreground_pgrp);

    let _ = omen.wait_for_text("\\O/", Duration::from_secs(15));
    let term_after = terminal_state(&omen);
    match &handoff {
        Some(h) => results.push(judge_shell_regains_tty_after_exit(h, &term_after, &shell)),
        None => results.push(InvariantResult::inconclusive(
            InvariantId::ShellRegainsTtyAfterJobExit,
            EvidenceGrade::Partial,
            "handoff evidence incomplete (shell/job/fg-during); reacquisition dependency blocked",
        )),
    }

    print_report("foreground-job", &results);
    assert!(!results.is_empty());
}

/// Scenario B — normal exit + shell tty reacquisition (requires handoff).
#[test]
fn omen_b_exit_reacquisition() {
    let Some(mut omen) = spawn_omen(&workspace_root()) else {
        eprintln!("SKIP: omen binary not built");
        return;
    };
    assert!(wait_prompt(&mut omen));
    let shell = shell_identity(&omen);
    let pre = terminal_state(&omen);
    assert_eq!(
        pre.foreground_pgrp,
        Some(shell.pid),
        "shell must own terminal before job"
    );

    write_line(
        &mut omen,
        &format!("--interactive {} --posix-report", gremlin_exe().display()),
    );
    let ready = wait_has(&mut omen, "OMEN_COMPAT_READY", Duration::from_secs(15));
    let mut handoff: Option<HandoffEvidence> = None;
    let mut results = Vec::new();
    if ready {
        let text = omen.transcript().as_lossy();
        match parse_posix_report(&text) {
            Ok(job) => {
                let fg_during = omen.observe_foreground_pgrp().ok();
                handoff = HandoffEvidence::try_from_observations(shell.pgrp, job.pgrp, fg_during);
            }
            Err(err) => {
                results.push(InvariantResult::inconclusive(
                    InvariantId::ShellRegainsTtyAfterJobExit,
                    EvidenceGrade::Partial,
                    format!("missing fixture identity; reacquisition dependency blocked: {err}"),
                ));
            }
        }
    } else {
        results.push(InvariantResult::inconclusive(
            InvariantId::ShellRegainsTtyAfterJobExit,
            EvidenceGrade::Partial,
            "interactive handoff did not announce READY; no proven prior handoff",
        ));
    }

    let _ = omen.wait_for_text("\\O/", Duration::from_secs(15));
    let post = terminal_state(&omen);
    match &handoff {
        Some(h) if h.is_proven() => {
            results.push(judge_shell_regains_tty_after_exit(h, &post, &shell));
        }
        Some(h) => results.push(InvariantResult::inconclusive(
            InvariantId::ShellRegainsTtyAfterJobExit,
            EvidenceGrade::Partial,
            format!(
                "dependency blocked: prior handoff not distinct (shell_pgrp={} job_pgrp={} fg_during={}); \
                 raw current ownership fg_after={:?}",
                h.shell_pgrp, h.job_pgrp, h.terminal_fg_during_job, post.foreground_pgrp
            ),
        )),
        None if results.is_empty() => results.push(InvariantResult::inconclusive(
            InvariantId::ShellRegainsTtyAfterJobExit,
            EvidenceGrade::Partial,
            "no proven prior distinct handoff; reacquisition dependency blocked",
        )),
        None => {}
    }
    print_report("exit-reacquisition", &results);
    assert!(omen.child_pid() > 0);
}

/// Scenario C — stopped foreground job.
#[test]
fn omen_c_stopped_foreground_job() {
    let Some(mut omen) = spawn_omen(&workspace_root()) else {
        eprintln!("SKIP: omen binary not built");
        return;
    };
    assert!(wait_prompt(&mut omen));
    let shell = shell_identity(&omen);

    write_line(
        &mut omen,
        &format!(
            "--interactive {} --posix-stop-report",
            gremlin_exe().display()
        ),
    );
    let stopping = omen
        .wait_for_text("OMEN_COMPAT_STOPPING", Duration::from_secs(15))
        .map(|t| t.contains("OMEN_COMPAT_STOPPING"))
        .unwrap_or(false);

    let mut results = Vec::new();
    if stopping {
        // Capture handoff evidence while the job is stopped / before prompt.
        let fg_during = omen.observe_foreground_pgrp().ok();
        let job_pgrp = {
            let text = omen.transcript().as_lossy();
            parse_posix_report(&text).ok().and_then(|j| j.pgrp)
        };
        let handoff = HandoffEvidence::try_from_observations(shell.pgrp, job_pgrp, fg_during);

        let got_prompt = omen
            .wait_for_text("\\O/", Duration::from_secs(6))
            .map(|t| t.contains("\\O/"))
            .unwrap_or(false);
        let term = terminal_state(&omen);
        match &handoff {
            Some(h) if h.is_proven() && got_prompt => {
                results.push(judge_shell_regains_tty_after_stop(h, &term, &shell));
            }
            Some(h) => results.push(InvariantResult::inconclusive(
                InvariantId::ShellRegainsTtyAfterJobStop,
                EvidenceGrade::Partial,
                format!(
                    "dependency blocked: prior handoff not proven (shell={} job={} fg_during={:?})",
                    h.shell_pgrp, h.job_pgrp, h.terminal_fg_during_job
                ),
            )),
            None => results.push(InvariantResult::inconclusive(
                InvariantId::ShellRegainsTtyAfterJobStop,
                EvidenceGrade::Partial,
                "no proven prior distinct handoff; reacquisition dependency blocked",
            )),
        }

        // Process-stopped fact (STRONG) from /proc is NOT wait-path evidence.
        #[cfg(target_os = "linux")]
        {
            let stopped_pids: Vec<u32> = omen_compat::direct_children(omen.child_pid())
                .into_iter()
                .filter(|c| omen_compat::is_stopped(*c))
                .collect();
            if let Some(&pid) = stopped_pids.first() {
                let obs = JobStoppedObserved {
                    pid,
                    source: "linux_proc".into(),
                };
                println!(
                    "FACT\tJOB_PROCESS_STOPPED\tSTRONG\t{}",
                    judge_job_stopped_note(&obs)
                );
                let _ = obs.observation();
            }
            // Omen wait-path STOP observation is unavailable without product
            // instrumentation — never PASS from /proc alone.
            results.push(InvariantResult::inconclusive(
                InvariantId::WaitObservesStoppedState,
                EvidenceGrade::Partial,
                "no Omen wait/job-control path observation of STOPPED; /proc process fact recorded separately",
            ));
        }
        #[cfg(not(target_os = "linux"))]
        {
            results.push(InvariantResult::unavailable(
                InvariantId::WaitObservesStoppedState,
                "external stopped-state observation unavailable without Linux /proc",
            ));
        }
    } else {
        results.push(InvariantResult::inconclusive(
            InvariantId::WaitObservesStoppedState,
            EvidenceGrade::Partial,
            "fixture did not report STOPPING within bound",
        ));
        results.push(InvariantResult::inconclusive(
            InvariantId::ShellRegainsTtyAfterJobStop,
            EvidenceGrade::Partial,
            "no stop barrier; reacquisition dependency blocked",
        ));
    }

    #[cfg(target_os = "linux")]
    {
        for child in omen_compat::direct_children(omen.child_pid()) {
            if omen_compat::is_stopped(child) {
                use rustix::process::{Pid, Signal, kill_process};
                if let Some(p) = Pid::from_raw(child as i32) {
                    let _ = kill_process(p, Signal::CONT);
                }
            }
        }
    }
    let _ = omen.wait_for_text("\\O/", Duration::from_secs(10));
    print_report("stop", &results);
}

fn judge_job_stopped_note(obs: &JobStoppedObserved) -> String {
    format!(
        "pid={} source={} (process fact only; not Omen wait-path)",
        obs.pid, obs.source
    )
}

/// Scenario E — terminal Ctrl-C with observed SIGINT delivery.
#[test]
fn omen_e_terminal_ctrl_c() {
    let Some(mut omen) = spawn_omen(&workspace_root()) else {
        eprintln!("SKIP: omen binary not built");
        return;
    };
    assert!(wait_prompt(&mut omen));
    let shell = shell_identity(&omen);

    write_line(
        &mut omen,
        &format!(
            "--interactive {} --posix-sigint-observe",
            gremlin_exe().display()
        ),
    );
    // READY alone is not identity; wait for the posix-report JSON line too.
    let ready = wait_has(&mut omen, "OMEN_COMPAT_READY", Duration::from_secs(15))
        && wait_has(
            &mut omen,
            "\"fixture\":\"posix-report\"",
            Duration::from_secs(10),
        );
    let mut results = Vec::new();
    if !ready {
        let shell_alive = omen.try_wait_child().ok().flatten().is_none();
        results.push(judge_shell_survives_foreground_sigint(
            shell_alive,
            shell.pid,
        ));
        results.push(InvariantResult::inconclusive(
            InvariantId::TerminalSigintTargetsForegroundJob,
            EvidenceGrade::Partial,
            "interactive handoff did not announce fixture READY+identity; no job identity for routing",
        ));
        print_report("ctrl-c", &results);
        return;
    }

    // Observe fixture identity, shell pgrp, and terminal fg BEFORE injection.
    let text = omen.transcript().as_lossy();
    let job = match parse_posix_report(&text) {
        Ok(job) => job,
        Err(err) => {
            let shell_alive = omen.try_wait_child().ok().flatten().is_none();
            results.push(judge_shell_survives_foreground_sigint(
                shell_alive,
                shell.pid,
            ));
            results.push(InvariantResult::inconclusive(
                InvariantId::TerminalSigintTargetsForegroundJob,
                EvidenceGrade::Partial,
                format!("missing fixture identity before VINTR; routing unproven: {err}"),
            ));
            print_report("ctrl-c", &results);
            return;
        }
    };
    let fg_before = omen.observe_foreground_pgrp().ok();
    let job_pgrp = job.pgrp;
    let fixture_pid_observed = job.pid > 0;
    let job_pgrp_observed = job_pgrp.is_some();
    let fg_observed = fg_before.is_some();
    println!(
        "FACT\tCTRLC_PRE\tfixture_pid={fixture_pid_observed} fixture_pgrp={job_pgrp_observed} fg_observed={fg_observed} shell_pgrp={:?} job_pgrp={job_pgrp:?} fg_before={fg_before:?}",
        shell.pgrp
    );

    // Inject VINTR only after identity/fg observations. One bounded retry:
    // line-discipline delivery can race interactive handoff setup.
    let mut vintr_ok = omen.write_ctrl(0x03).is_ok();
    let _ = omen.pump_until(
        |t| {
            t.as_lossy().lines().any(|l| {
                l.trim() == "OMEN_COMPAT_SIGINT" || l.trim() == "OMEN_COMPAT_SIGINT_TIMEOUT"
            })
        },
        Duration::from_millis(600),
    );
    if !transcript_has_line(&omen.transcript().as_lossy(), "OMEN_COMPAT_SIGINT") {
        vintr_ok |= omen.write_ctrl(0x03).is_ok();
    }
    println!("FACT\tVINTR_INJECTED\t{vintr_ok}");

    // Delivery must come from an independent exact-line observation
    // (`OMEN_COMPAT_SIGINT`), never hard-coded. ARMED/TIMEOUT share the
    // prefix but are not equal lines.
    let _ = omen.pump_until(
        |t| {
            t.as_lossy().lines().any(|l| {
                l.trim() == "OMEN_COMPAT_SIGINT" || l.trim() == "OMEN_COMPAT_SIGINT_TIMEOUT"
            })
        },
        Duration::from_secs(8),
    );
    let receipt_observed = transcript_has_line(&omen.transcript().as_lossy(), "OMEN_COMPAT_SIGINT");
    println!("FACT\tSIGINT_RECEIPT\t{receipt_observed}");

    let receipt = if receipt_observed {
        SigintReceiptObservation::Observed {
            source: "fixture_omen_compat_sigint_marker".into(),
        }
    } else if vintr_ok {
        SigintReceiptObservation::InjectedNotObserved
    } else {
        SigintReceiptObservation::Unavailable {
            reason: "VINTR write failed".into(),
        }
    };

    let shell_alive = omen.try_wait_child().ok().flatten().is_none();
    results.push(judge_shell_survives_foreground_sigint(
        shell_alive,
        shell.pid,
    ));
    results.push(judge_sigint_targets_foreground_job(
        shell_alive,
        shell.pgrp,
        job_pgrp,
        fg_before,
        &receipt,
    ));
    let _ = omen.wait_for_text("\\O/", Duration::from_secs(10));
    print_report("ctrl-c", &results);
}

fn transcript_has_line(text: &str, needle: &str) -> bool {
    text.lines().any(|l| l.trim() == needle)
}

/// Scenario G — signal-mask/disposition child report.
#[test]
fn omen_g_signal_mask_child_report() {
    let Some(mut omen) = spawn_omen(&workspace_root()) else {
        eprintln!("SKIP: omen binary not built");
        return;
    };
    assert!(wait_prompt(&mut omen));
    write_line(
        &mut omen,
        &format!("{} --posix-report", gremlin_exe().display()),
    );
    let text = omen
        .wait_for_text("\"blocked\"", Duration::from_secs(15))
        .map(|t| t.as_lossy())
        .unwrap_or_default();
    let blocked = parse_json_str_array(&text, "blocked");
    let results = vec![judge_child_signal_mask_unblocked(
        &blocked,
        &["SIGINT", "SIGTSTP", "SIGQUIT", "SIGTTIN", "SIGTTOU"],
    )];
    print_report("signal-mask", &results);
    assert!(omen.child_pid() > 0);
}

/// Scenario H — SIGWINCH on foreground job.
#[test]
fn omen_h_sigwinch_on_foreground_job() {
    let Some(mut omen) = spawn_omen(&workspace_root()) else {
        eprintln!("SKIP: omen binary not built");
        return;
    };
    assert!(wait_prompt(&mut omen));
    write_line(
        &mut omen,
        &format!(
            "--interactive {} --posix-winch-report",
            gremlin_exe().display()
        ),
    );
    let waiting = wait_has(
        &mut omen,
        "OMEN_COMPAT_SIGWINCH_WAIT",
        Duration::from_secs(15),
    );
    if !waiting {
        let results = vec![InvariantResult::new(
            InvariantId::SigwinchAsyncDeliveredToForegroundPgrpOnResize,
            InvariantOutcome::OpenDefect,
            EvidenceGrade::Partial,
            "interactive handoff did not start winch fixture within bound",
        )];
        print_report("sigwinch", &results);
        assert!(omen.child_pid() > 0);
        return;
    }

    let before = omen.observe_winsize().unwrap_or(PtyWinsize::default());
    omen.set_winsize(PtyWinsize {
        rows: 40,
        cols: 120,
    })
    .expect("resize");
    let after = omen.observe_winsize().expect("winsize after");
    let got = wait_has(&mut omen, "OMEN_COMPAT_SIGWINCH", Duration::from_secs(10))
        && !omen.transcript().contains("OMEN_COMPAT_SIGWINCH_TIMEOUT");

    let results = vec![judge_sigwinch_async_delivered(
        (before.rows, before.cols) != (after.rows, after.cols),
        got,
        (before.rows, before.cols),
        (after.rows, after.cols),
    )];
    print_report("sigwinch", &results);
}

/// Scenario I — abnormal termios mutation + shell recovery.
#[test]
fn omen_i_termios_recovery_after_abnormal_child() {
    let Some(mut omen) = spawn_omen(&workspace_root()) else {
        eprintln!("SKIP: omen binary not built");
        return;
    };
    assert!(wait_prompt(&mut omen));
    let pre = omen.observe_termios_snapshot().expect("shell termios pre");

    write_line(
        &mut omen,
        &format!(
            "--interactive {} --posix-termios-dirty-exit --exit 9",
            gremlin_exe().display()
        ),
    );
    let _ = omen.wait_for_text("OMEN_COMPAT_DIRTY", Duration::from_secs(15));
    let _ = omen.wait_for_text("\\O/", Duration::from_secs(15));

    let post = omen.observe_termios_snapshot().expect("shell termios post");
    let results = vec![judge_termios_restored(&pre, &post)];
    print_report("termios", &results);
}

/// Scenario J — zombie/direct-child observation (Linux-strong).
#[test]
fn omen_j_no_zombie_children_after_normal_exit() {
    let Some(mut omen) = spawn_omen(&workspace_root()) else {
        eprintln!("SKIP: omen binary not built");
        return;
    };
    assert!(wait_prompt(&mut omen));
    write_line(
        &mut omen,
        &format!("{} --compat-report --exit 0", gremlin_exe().display()),
    );
    let _ = omen.wait_for_text("compat-report", Duration::from_secs(15));
    let _ = omen.wait_for_text("\\O/", Duration::from_secs(15));

    #[cfg(target_os = "linux")]
    {
        let zombies = omen_compat::zombie_direct_children(omen.child_pid());
        let results = vec![judge_no_zombie_children(
            omen.child_pid(),
            &zombies,
            omen_compat::zombie_evidence_available(),
        )];
        print_report("zombie", &results);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let results = vec![judge_no_zombie_children(omen.child_pid(), &[], false)];
        print_report("zombie", &results);
    }
}

/// Scenario F — signal-faithful exit identity (fixture SIGINT path).
#[test]
fn omen_f_signal_faithful_exit_identity() {
    let Some(mut omen) = spawn_omen(&workspace_root()) else {
        eprintln!("SKIP: omen binary not built");
        return;
    };
    assert!(wait_prompt(&mut omen));
    let shell = shell_identity(&omen);
    write_line(
        &mut omen,
        &format!(
            "--interactive {} --posix-sigint-report",
            gremlin_exe().display()
        ),
    );
    let _ = omen.wait_for_text("OMEN_COMPAT_READY", Duration::from_secs(15));
    let _ = omen.write_ctrl(0x03);
    let shell_alive = omen.try_wait_child().ok().flatten().is_none();
    // ExitStatusPreservesSignalNumber for the grandchild is judged only when
    // Omen surfaces a typed signal; otherwise UNAVAILABLE (honest gap).
    let results = vec![
        judge_shell_survives_foreground_sigint(shell_alive, shell.pid),
        InvariantResult::unavailable(
            InvariantId::ExitStatusPreservesSignalNumber,
            "Omen interactive handoff does not surface grandchild wait-signal identity to Compat without production instrumentation",
        ),
    ];
    let _ = omen.wait_for_text("\\O/", Duration::from_secs(10));
    print_report("signal-faithful", &results);
}

fn parse_json_str_array(text: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\":[");
    if let Some(idx) = text.rfind(&needle) {
        let rest = &text[idx + needle.len()..];
        if let Some(end) = rest.find(']') {
            let inner = &rest[..end];
            return inner
                .split(',')
                .filter_map(|s| {
                    let t = s.trim().trim_matches('"');
                    if t.is_empty() {
                        None
                    } else {
                        Some(t.to_string())
                    }
                })
                .collect();
        }
    }
    Vec::new()
}

#[allow(dead_code)]
fn _types(_: TermiosSnapshot) {}
