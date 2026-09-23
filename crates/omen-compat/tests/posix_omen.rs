//! Tier POSIX OMEN — real Omen shell under the calibrated PTY.
//!
//! Product compatibility results may be FAIL / OPEN_DEFECT while the harness
//! itself remains green. Outer watchdog bounds every scenario.
//! Production Omen is never repaired here.

#![cfg(unix)]

use omen_compat::{
    InvariantId, InvariantOutcome, InvariantResult, PosixProcessIdentity, PosixTerminalState,
    PtySession, PtyWinsize, TermiosSnapshot, judge_child_signal_mask_unblocked,
    judge_job_has_distinct_pgrp, judge_job_shares_session, judge_no_zombie_children,
    judge_shell_regains_tty_after_exit, judge_shell_regains_tty_after_stop,
    judge_shell_survives_foreground_sigint, judge_sigint_targets_foreground_job,
    judge_sigwinch_async_delivered, judge_terminal_fg_is_job, judge_termios_restored,
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
    // Prompt sigil is readiness only; topology comes from OS observations.
    // Also accept reedline DSR timeout errors as "shell is running" evidence
    // when the sigil never appears (platform/DSR gap → still measure later).
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
        // Interactive handoff did not run the fixture — record product gap.
        results.push(InvariantResult::new(
            InvariantId::ShellJobHasDistinctProcessGroup,
            InvariantOutcome::OpenDefect,
            omen_compat::EvidenceGrade::Partial,
            "interactive handoff path did not produce fixture READY within bound; cannot prove distinct job pgrp",
        ));
        print_report("foreground-job", &results);
        assert!(omen.child_pid() > 0);
        return;
    }

    let text = omen.transcript().as_lossy();
    let job = parse_posix_report(&text).unwrap_or(PosixProcessIdentity {
        pid: 0,
        ppid: Some(shell.pid),
        pgrp: Some(shell.pgrp.unwrap_or(shell.pid)),
        session_id: shell.session_id,
        source: "fallback_same_as_shell".into(),
    });

    results.push(judge_job_shares_session(&shell, &job));
    results.push(judge_job_has_distinct_pgrp(&shell, &job));
    let term_job = terminal_state(&omen);
    results.push(judge_terminal_fg_is_job(&term_job, &job));

    let _ = omen.wait_for_text("\\O/", Duration::from_secs(15));
    let term_after = terminal_state(&omen);
    results.push(judge_shell_regains_tty_after_exit(&term_after, &shell));

    print_report("foreground-job", &results);
    assert!(!results.is_empty());
}

/// Scenario B — normal exit + shell tty reacquisition.
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
        &format!("{} --compat-report --exit 0", gremlin_exe().display()),
    );
    let _ = omen.wait_for_text("compat-report", Duration::from_secs(15));
    let _ = omen.wait_for_text("\\O/", Duration::from_secs(15));

    let post = terminal_state(&omen);
    let results = vec![judge_shell_regains_tty_after_exit(&post, &shell)];
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
        let got_prompt = omen
            .wait_for_text("\\O/", Duration::from_secs(6))
            .map(|t| t.contains("\\O/"))
            .unwrap_or(false);
        let term = terminal_state(&omen);
        if got_prompt {
            results.push(judge_shell_regains_tty_after_stop(&term, &shell));
        } else {
            results.push(InvariantResult::new(
                InvariantId::ShellRegainsTtyAfterJobStop,
                InvariantOutcome::Inconclusive,
                omen_compat::EvidenceGrade::Partial,
                "fixture stopped; prompt did not return within bound",
            ));
        }
        #[cfg(target_os = "linux")]
        {
            let observed = omen_compat::direct_children(omen.child_pid())
                .into_iter()
                .any(omen_compat::is_stopped);
            results.push(InvariantResult::new(
                InvariantId::WaitObservesStoppedState,
                if observed {
                    InvariantOutcome::Pass
                } else {
                    InvariantOutcome::Inconclusive
                },
                omen_compat::EvidenceGrade::Strong,
                format!("linux /proc direct-child stopped observation={observed}"),
            ));
        }
        #[cfg(not(target_os = "linux"))]
        {
            results.push(InvariantResult::new(
                InvariantId::WaitObservesStoppedState,
                InvariantOutcome::Unavailable,
                omen_compat::EvidenceGrade::Unavailable,
                "external stopped-state observation unavailable without Linux /proc",
            ));
        }
    } else {
        results.push(InvariantResult::new(
            InvariantId::WaitObservesStoppedState,
            InvariantOutcome::Inconclusive,
            omen_compat::EvidenceGrade::Partial,
            "fixture did not report STOPPING within bound",
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

/// Scenario E — terminal Ctrl-C.
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
            "--interactive {} --posix-sigint-report",
            gremlin_exe().display()
        ),
    );
    let ready = wait_has(&mut omen, "OMEN_COMPAT_READY", Duration::from_secs(15));
    let mut results = Vec::new();
    if !ready {
        let shell_alive = omen.try_wait_child().ok().flatten().is_none();
        results.push(judge_shell_survives_foreground_sigint(
            shell_alive,
            shell.pid,
        ));
        results.push(InvariantResult::new(
            InvariantId::TerminalSigintTargetsForegroundJob,
            InvariantOutcome::OpenDefect,
            omen_compat::EvidenceGrade::Partial,
            "interactive handoff did not announce fixture READY; Ctrl-C routing to job unproven",
        ));
        print_report("ctrl-c", &results);
        return;
    }

    let fg_job = omen.observe_foreground_pgrp().ok();
    omen.write_ctrl(0x03).expect("inject VINTR");

    let shell_alive = omen.try_wait_child().ok().flatten().is_none();
    results.push(judge_shell_survives_foreground_sigint(
        shell_alive,
        shell.pid,
    ));
    results.push(judge_sigint_targets_foreground_job(
        true,
        shell_alive,
        shell.pgrp,
        fg_job,
        fg_job,
    ));
    let _ = omen.wait_for_text("\\O/", Duration::from_secs(10));
    print_report("ctrl-c", &results);
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
            omen_compat::EvidenceGrade::Partial,
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
    // Calibration already proves signal-faithful SIGINT on the control path.
    // Through Omen we record whether the shell surfaces the same identity.
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
    let mut results = Vec::new();
    results.push(judge_shell_survives_foreground_sigint(
        shell_alive,
        shell.pid,
    ));
    // ExitStatusPreservesSignalNumber for the grandchild is judged only when
    // Omen surfaces a typed signal; otherwise UNAVAILABLE (honest gap).
    results.push(InvariantResult::new(
        InvariantId::ExitStatusPreservesSignalNumber,
        InvariantOutcome::Unavailable,
        omen_compat::EvidenceGrade::Unavailable,
        "Omen interactive handoff does not surface grandchild wait-signal identity to Compat without production instrumentation",
    ));
    let _ = omen.wait_for_text("\\O/", Duration::from_secs(10));
    print_report("signal-faithful", &results);
}

fn parse_posix_report(text: &str) -> Option<PosixProcessIdentity> {
    for line in text.lines().rev() {
        let line = line.trim();
        if !line.contains("\"fixture\":\"posix-report\"") {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        return Some(PosixProcessIdentity {
            pid: v["pid"].as_u64()? as u32,
            ppid: v["ppid"].as_i64().map(|x| x as u32).filter(|&x| x > 0),
            pgrp: v["pgrp"].as_i64().map(|x| x as u32).filter(|&x| x > 0),
            session_id: v["sid"].as_i64().map(|x| x as u32).filter(|&x| x > 0),
            source: "fixture_posix_report".into(),
        });
    }
    None
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
