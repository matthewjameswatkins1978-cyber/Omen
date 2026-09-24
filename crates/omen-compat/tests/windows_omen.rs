//! Tier WINDOWS OMEN INTERACTIVE — real Omen shell under Compat ConPTY.
//!
//! Product results may be FAIL / INCONCLUSIVE / OPEN_DEFECT while the control
//! harness remains green. Production Omen is never repaired here.

#![cfg(windows)]

use omen_compat::{
    EvidenceGrade, InvariantId, InvariantOutcome, InvariantResult, WindowsConPtySession,
    WindowsConsoleModeSnapshot, WindowsCtrlReceipt, judge_console_mode_restored,
    judge_interactive_child_console_handles, judge_outer_resize_visible,
    judge_shell_survives_child_ctrl_c, judge_targetable_control_group,
    judge_terminal_ctrl_c_reaches_child, parse_windows_report,
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
        let mut path = std::env::current_exe().expect("current_exe");
        path.pop();
        if path.ends_with("deps") {
            path.pop();
        }
        let name = "omen-gremlin.exe";
        let candidates = [
            path.join(name),
            workspace_root().join("target").join("debug").join(name),
        ];
        for c in &candidates {
            if c.exists() {
                return c.clone();
            }
        }
        let build = std::process::Command::new("cargo")
            .args(["build", "-p", "omen-test-fixtures", "--bin", "omen-gremlin"])
            .current_dir(workspace_root())
            .status()
            .expect("gremlin build");
        assert!(build.success());
        for c in &candidates {
            if c.exists() {
                return c.clone();
            }
        }
        panic!("omen-gremlin not found");
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
        let name = "omen.exe";
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

fn seed_human_settings(home: &std::path::Path) {
    let cfg = home.join(".config").join("omen");
    let _ = std::fs::create_dir_all(&cfg);
    let _ = std::fs::write(
        cfg.join("config.toml"),
        "theme = \"omen\"\ndensity = \"normal\"\nepigraph = true\n",
    );
}

fn spawn_omen(cwd: &std::path::Path) -> Option<WindowsConPtySession> {
    let exe = omen_exe()?;
    seed_human_settings(cwd);
    let session = WindowsConPtySession::spawn(exe, &[], Some(cwd), 24, 80, SCENARIO_BUDGET).ok()?;
    Some(session)
}

fn wait_prompt(session: &mut WindowsConPtySession) -> bool {
    match session.wait_for_text("\\O/", Duration::from_secs(15)) {
        Ok(t) => t.contains("\\O/"),
        Err(_) => false,
    }
}

fn wait_has(session: &mut WindowsConPtySession, needle: &str, budget: Duration) -> bool {
    match session.wait_for_text(needle, budget) {
        Ok(t) => t.contains(needle),
        Err(_) => false,
    }
}

fn write_line(session: &mut WindowsConPtySession, line: &str) {
    let mut s = line.to_string();
    s.push_str("\r\n");
    let _ = session.write_input(s.as_bytes());
}

fn print_report(scenario: &str, results: &[InvariantResult]) {
    println!("WINDOWS / INTERACTIVE HANDOFF");
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

fn transcript_has_line(text: &str, needle: &str) -> bool {
    text.lines().any(|l| l.trim() == needle)
}

/// A — prompt/readiness (`\O/` is readiness only, not topology truth).
#[test]
fn windows_a_prompt_readiness() {
    let Some(mut omen) = spawn_omen(&workspace_root()) else {
        eprintln!("SKIP: omen binary not built");
        return;
    };
    assert!(wait_prompt(&mut omen), "prompt readiness");
    assert!(omen.child_pid() > 0);
    omen.close_handles();
}

/// B — interactive child Windows report + console handle invariant.
#[test]
fn windows_b_interactive_child_console_handles() {
    let Some(mut omen) = spawn_omen(&workspace_root()) else {
        eprintln!("SKIP: omen binary not built");
        return;
    };
    assert!(wait_prompt(&mut omen));
    write_line(
        &mut omen,
        &format!("--interactive {} --windows-report", gremlin_exe().display()),
    );
    let ready = wait_has(&mut omen, "OMEN_COMPAT_READY", Duration::from_secs(15))
        && wait_has(
            &mut omen,
            "\"fixture\":\"windows-report\"",
            Duration::from_secs(10),
        );
    let mut results = Vec::new();
    if !ready {
        results.push(InvariantResult::new(
            InvariantId::WindowsInteractiveChildConsoleHandlesValid,
            InvariantOutcome::OpenDefect,
            EvidenceGrade::Partial,
            "interactive handoff did not produce windows-report within bound",
        ));
        print_report("child-handles", &results);
        omen.close_handles();
        return;
    }
    let text = omen.transcript().as_lossy();
    match parse_windows_report(&text) {
        Some(report) => {
            results.push(judge_interactive_child_console_handles(&report));
            // Targetable control group: no behavioural targeting probe in this
            // scenario → honest INCONCLUSIVE.
            results.push(judge_targetable_control_group(None, None));
        }
        None => results.push(InvariantResult::inconclusive(
            InvariantId::WindowsInteractiveChildConsoleHandlesValid,
            EvidenceGrade::Partial,
            "windows-report present but unparseable",
        )),
    }
    print_report("child-handles", &results);
    omen.close_handles();
}

/// C+D — Ctrl-C through outer ConPTY + shell survival (independent results).
#[test]
fn windows_c_d_ctrl_c_and_shell_survival() {
    let Some(mut omen) = spawn_omen(&workspace_root()) else {
        eprintln!("SKIP: omen binary not built");
        return;
    };
    assert!(wait_prompt(&mut omen));
    write_line(
        &mut omen,
        &format!(
            "--interactive {} --windows-ctrl-observe",
            gremlin_exe().display()
        ),
    );
    let armed = wait_has(&mut omen, "OMEN_COMPAT_CTRL_ARMED", Duration::from_secs(15));
    let mut results = Vec::new();
    let shell_pid = omen.child_pid();
    if !armed {
        results.push(InvariantResult::inconclusive(
            InvariantId::WindowsTerminalCtrlCReachesInteractiveChild,
            EvidenceGrade::Partial,
            "ctrl-observe fixture did not announce ARMED",
        ));
        let shell_alive = omen.is_alive();
        results.push(judge_shell_survives_child_ctrl_c(shell_alive, shell_pid));
        print_report("ctrl-c", &results);
        omen.close_handles();
        return;
    }

    // User-like Ctrl-C through OUTER Compat ConPTY input only.
    let _ = omen.write_input(&[0x03]);
    let _ = omen.pump_until(
        |t| {
            t.contains("OMEN_COMPAT_CTRL_C")
                || t.contains("OMEN_COMPAT_CTRL_TIMEOUT")
                || t.contains("\\O/")
        },
        Duration::from_secs(8),
    );
    let receipt = if transcript_has_line(&omen.transcript().as_lossy(), "OMEN_COMPAT_CTRL_C")
        || omen.transcript().contains("OMEN_COMPAT_CTRL_C")
    {
        WindowsCtrlReceipt::Observed {
            kind: "CTRL_C_EVENT",
            source: "fixture_ctrl_marker".into(),
        }
    } else {
        WindowsCtrlReceipt::NotObserved
    };
    println!(
        "FACT\tCTRL_C_RECEIPT\t{}",
        matches!(receipt, WindowsCtrlReceipt::Observed { .. })
    );
    results.push(judge_terminal_ctrl_c_reaches_child(&receipt));

    // Shell survival is independent of child receipt.
    let shell_alive = omen.is_alive();
    results.push(judge_shell_survives_child_ctrl_c(shell_alive, shell_pid));

    // Targetable control group: without a separate GenerateConsoleCtrlEvent
    // experiment, creation-only → INCONCLUSIVE (configured ≠ observed).
    results.push(judge_targetable_control_group(Some(&receipt), None));

    let _ = omen.wait_for_text("\\O/", Duration::from_secs(8));
    print_report("ctrl-c", &results);
    omen.close_handles();
}

/// E — dirty console mode + restoration after abnormal child exit.
#[test]
fn windows_e_mode_restoration_after_abnormal_exit() {
    let Some(mut omen) = spawn_omen(&workspace_root()) else {
        eprintln!("SKIP: omen binary not built");
        return;
    };
    assert!(wait_prompt(&mut omen));

    // Baseline report.
    write_line(
        &mut omen,
        &format!("--interactive {} --windows-report", gremlin_exe().display()),
    );
    let _ = wait_has(
        &mut omen,
        "\"fixture\":\"windows-report\"",
        Duration::from_secs(12),
    );
    let pre_text = omen.transcript().as_lossy();
    let pre_report = parse_windows_report(&pre_text);
    let pre = pre_report
        .as_ref()
        .map(|r| r.console_mode_snapshot())
        .unwrap_or_else(|| WindowsConsoleModeSnapshot::unavailable("pre report missing"));

    // Dirty fixture (abnormal exit).
    write_line(
        &mut omen,
        &format!(
            "--interactive {} --windows-console-mode-dirty-exit --exit 9",
            gremlin_exe().display()
        ),
    );
    let dirty_seen = wait_has(&mut omen, "OMEN_COMPAT_DIRTY", Duration::from_secs(12));
    let dirty_text = omen.transcript().as_lossy();
    let dirty_verified =
        dirty_seen && (dirty_text.contains("verify=true") || dirty_text.contains("verify: true"));

    let _ = omen.wait_for_text("\\O/", Duration::from_secs(12));

    // Second report after prompt returns.
    write_line(
        &mut omen,
        &format!("--interactive {} --windows-report", gremlin_exe().display()),
    );
    let _ = wait_has(
        &mut omen,
        "\"fixture\":\"windows-report\"",
        Duration::from_secs(12),
    );
    // Re-parse the latest report (use reverse transcript).
    let post_text = omen.transcript().as_lossy();
    let post_report = parse_windows_report(&post_text);
    let post = post_report
        .as_ref()
        .map(|r| r.console_mode_snapshot())
        .unwrap_or_else(|| WindowsConsoleModeSnapshot::unavailable("post report missing"));

    let results = vec![judge_console_mode_restored(&pre, dirty_verified, &post)];
    print_report("mode-restore", &results);
    omen.close_handles();
}

/// F — outer Compty resize while interactive child active.
#[test]
fn windows_f_outer_resize_visible() {
    let Some(mut omen) = spawn_omen(&workspace_root()) else {
        eprintln!("SKIP: omen binary not built");
        return;
    };
    assert!(wait_prompt(&mut omen));
    write_line(
        &mut omen,
        &format!(
            "--interactive {} --windows-resize-report",
            gremlin_exe().display()
        ),
    );
    let waiting = wait_has(
        &mut omen,
        "OMEN_COMPAT_RESIZE_WAIT",
        Duration::from_secs(15),
    );
    if !waiting {
        let results = vec![InvariantResult::new(
            InvariantId::WindowsOuterConPtyResizeVisibleToInteractiveChild,
            InvariantOutcome::OpenDefect,
            EvidenceGrade::Partial,
            "resize fixture did not reach WAIT barrier",
        )];
        print_report("outer-resize", &results);
        omen.close_handles();
        return;
    }
    let before = omen.dimensions();
    omen.resize(40, 120).expect("outer ResizePseudoConsole");
    let after = omen.dimensions();
    let child_observed = wait_has(&mut omen, "OMEN_COMPAT_RESIZED", Duration::from_secs(8))
        || omen.transcript().contains("OMEN_COMPAT_RESIZED");
    let results = vec![judge_outer_resize_visible(before, after, child_observed)];
    print_report("outer-resize", &results);
    omen.close_handles();
}

/// G — targetable control-group probe (creation-only path → honest result).
#[test]
fn windows_g_control_group_probe() {
    let results = vec![judge_targetable_control_group(None, None)];
    assert_eq!(results[0].outcome, InvariantOutcome::Inconclusive);
    print_report("control-group", &results);
}
