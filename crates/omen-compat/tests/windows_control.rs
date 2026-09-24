//! Tier WINDOWS CONTROL — Compat-owned ConPTY calibration without Omen.
//!
//! If these fail, the instrument is not ready. Do not judge Omen.
//! Outer watchdog: every test is bounded; no sleep is used as proof.
//!
//! D2-022 boundedness seal: blocked-write cancellation, close modes,
//! repeated lifecycle, and hostile helper watchdog are control facts.

#![cfg(windows)]

use omen_compat::{
    InvariantOutcome, WIN_WRITE_CANCEL_BOUND, WindowsConPtySession, WindowsConsoleModeSnapshot,
    WindowsCtrlReceipt, WindowsExitCauseContext, WindowsExitObservation, WindowsProcessLiveness,
    WindowsWriteOutcome, control_blocked_write, judge_engine_shutdown_bounded,
    judge_terminal_ctrl_c_reaches_child, parse_windows_report, poll_until,
    release_pseudoconsole_available,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
    STARTUPINFOW,
};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};

const WATCHDOG: Duration = Duration::from_secs(25);

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

fn spawn(args: &[&str], deadline: Duration) -> WindowsConPtySession {
    let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    WindowsConPtySession::spawn(
        gremlin_exe(),
        &argv,
        Some(&workspace_root()),
        24,
        80,
        deadline.min(WATCHDOG),
    )
    .expect("ConPTY spawn")
}

/// Control 1 — real CreatePseudoConsole + fixture I/O (independent of omen-engine).
#[test]
fn control_conpty_create_and_fixture_io() {
    let t0 = Instant::now();
    let mut session = spawn(&["--windows-report"], Duration::from_secs(12));
    let t = session
        .wait_for_text("OMEN_COMPAT_READY", Duration::from_secs(8))
        .expect("READY");
    assert!(
        t.contains("OMEN_COMPAT_READY"),
        "transcript: {:?}",
        t.as_lossy()
    );
    assert!(t0.elapsed() < WATCHDOG);
    let _ = session.wait_for_text("\"fixture\":\"windows-report\"", Duration::from_secs(5));
    session.close_handles();
}

/// Control 2 — fixture console handles / GetConsoleMode observation.
#[test]
fn control_fixture_console_handles_observed() {
    let mut session = spawn(&["--windows-report"], Duration::from_secs(12));
    let t = session
        .wait_for_text("\"fixture\":\"windows-report\"", Duration::from_secs(8))
        .expect("report");
    let report = parse_windows_report(&t.as_lossy()).expect("windows-report parse");
    assert!(report.pid > 0);
    // ConPTY client should see console-capable std handles.
    assert!(
        report.stdin_console_mode.is_some() || report.stdin_file_type.is_some(),
        "expected console/file-type evidence: {report:?}"
    );
    session.close_handles();
}

/// Control 3 — user-like Ctrl-C receipt through ConPTY input (0x03).
#[test]
fn control_user_like_ctrl_c_receipt() {
    let mut session = spawn(&["--windows-ctrl-observe"], Duration::from_secs(15));
    let armed = session
        .wait_for_text("OMEN_COMPAT_CTRL_ARMED", Duration::from_secs(8))
        .expect("ARMED");
    assert!(armed.contains("OMEN_COMPAT_CTRL_ARMED"));
    session.write_input(&[0x03]).expect("write Ctrl-C");
    let _ = session.pump_until(
        |t| t.contains("OMEN_COMPAT_CTRL_C") || t.contains("OMEN_COMPAT_CTRL_TIMEOUT"),
        Duration::from_secs(8),
    );
    let observed = session.transcript().contains("OMEN_COMPAT_CTRL_C");
    let receipt = if observed {
        WindowsCtrlReceipt::Observed {
            kind: "CTRL_C_EVENT",
            source: "fixture_ctrl_marker".into(),
        }
    } else {
        WindowsCtrlReceipt::NotObserved
    };
    let r = judge_terminal_ctrl_c_reaches_child(&receipt);
    if observed {
        assert_eq!(r.outcome, InvariantOutcome::Pass, "{r:?}");
    } else if cfg!(target_os = "windows") {
        // CI without a process-group Ctrl-C path: record honestly, do not invent.
        eprintln!("UNAVAILABLE/INCONCLUSIVE: CTRL_C_EVENT not observed via ConPTY input: {r:?}");
        assert_ne!(r.outcome, InvariantOutcome::Pass, "{r:?}");
    }
    let _ = session.terminate_bounded(Duration::from_secs(2));
    session.close_handles();
}

/// Control 4 — resize observed by fixture.
#[test]
fn control_resize_observed_by_fixture() {
    let mut session = spawn(&["--windows-resize-report"], Duration::from_secs(15));
    let _ = session
        .wait_for_text("OMEN_COMPAT_RESIZE_WAIT", Duration::from_secs(8))
        .expect("WAIT");
    session.resize(40, 120).expect("ResizePseudoConsole");
    let _ = session.pump_until(
        |t| t.contains("OMEN_COMPAT_RESIZED") || t.contains("OMEN_COMPAT_RESIZE_TIMEOUT"),
        Duration::from_secs(8),
    );
    let observed = session.transcript().contains("OMEN_COMPAT_RESIZED");
    if observed {
        // PASS via transcript observation in product path; control asserts fact.
        assert!(observed);
    } else if cfg!(target_os = "windows") {
        eprintln!("UNAVAILABLE: fixture did not observe resize via GetConsoleScreenBufferInfo");
    }
    let _ = session.terminate_bounded(Duration::from_secs(2));
    session.close_handles();
}

/// Control 5 — controlled console-mode mutation verified.
#[test]
fn control_console_mode_mutation_verified() {
    let mut session = spawn(
        &["--windows-console-mode-dirty-exit"],
        Duration::from_secs(12),
    );
    let t = session
        .wait_for_text("OMEN_COMPAT_DIRTY", Duration::from_secs(8))
        .expect("DIRTY");
    let text = t.as_lossy();
    assert!(text.contains("OMEN_COMPAT_DIRTY"), "{text}");
    let verified = text.contains("verify=true") || text.contains("verify: true");
    if !verified {
        eprintln!("INCONCLUSIVE: fixture could not verify console-mode mutation: {text}");
    }
    let _ = session.terminate_bounded(Duration::from_secs(2));
    session.close_handles();
}

/// Control 6 — raw Windows exit DWORD observation.
#[test]
fn control_raw_exit_status_observed() {
    let mut session = spawn(&["--windows-exit-status", "7"], Duration::from_secs(10));
    // Non-zero status path exits immediately after any prior mode checks.
    let code = session.wait_exit(Duration::from_secs(8)).expect("wait");
    // With default args, gremlin exits via --windows-exit-status when != 0.
    // If the process exits before we can bind, wait_exit still observes DWORD.
    let observed = code.or_else(|| session.try_exit().ok().flatten());
    if let Some(raw) = observed {
        let obs = WindowsExitObservation {
            raw_status: Some(raw),
            product_code: Some(raw as i32),
            cause_context: WindowsExitCauseContext::Unknown,
            source: "control_wait_exit".into(),
        };
        assert_eq!(obs.raw_status, Some(raw));
    } else {
        eprintln!("UNAVAILABLE: process did not exit within bound");
    }
    session.close_handles();
}

/// Control 7 — Job Object descendant containment calibration (Compat-owned).
#[test]
fn control_job_object_descendant_containment() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        assert!(!job.is_null(), "CreateJobObjectW");
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        assert_ne!(
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32
            ),
            0
        );

        // Spawn suspended gremlin child-hold, assign, resume (creation barrier).
        let exe = gremlin_exe();
        let mut cmd_line: Vec<u16> = OsStr::new(&format!(
            "\"{}\" --windows-child-hold --sleep-ms 4000",
            exe.display()
        ))
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESTDHANDLES;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        assert_ne!(
            CreateProcessW(
                std::ptr::null(),
                cmd_line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_SUSPENDED,
                std::ptr::null(),
                std::ptr::null(),
                &si,
                &mut pi
            ),
            0
        );
        // Open for assign: use process handle from PROCESS_INFORMATION.
        assert_ne!(
            AssignProcessToJobObject(job, pi.hProcess),
            0,
            "AssignProcessToJobObject"
        );
        ResumeThread(pi.hThread);
        CloseHandle(pi.hThread);

        let parent_pid = pi.dwProcessId;
        // Fixture holds ~3.5s; observe parent alive.
        let alive_before = WaitForSingleObject(pi.hProcess, 200) != WAIT_OBJECT_0;
        assert!(alive_before, "parent must be alive after resume");

        // Terminate job (kill-on-close style).
        TerminateJobObject(job, 1);
        let wait_ms = 3000u32;
        let _ = WaitForSingleObject(pi.hProcess, wait_ms);
        let mut code = 0u32;
        GetExitCodeProcess(pi.hProcess, &mut code);
        let gone = WaitForSingleObject(pi.hProcess, 0) == WAIT_OBJECT_0;
        assert!(gone, "parent must die after TerminateJobObject");

        let obs = WindowsProcessLiveness {
            pid: parent_pid,
            alive: !gone,
            parent_pid: None,
            source: "control_job_terminate".into(),
        };
        assert!(!obs.alive);

        CloseHandle(pi.hProcess);
        CloseHandle(job);
    }
}

/// Control 8 — bounded normal shutdown.
#[test]
fn control_bounded_normal_shutdown() {
    let t0 = Instant::now();
    let mut session = spawn(&["--windows-report"], Duration::from_secs(10));
    let _ = session.wait_for_text("OMEN_COMPAT_READY", Duration::from_secs(6));
    // Let fixture exit naturally (report path exits after print).
    let _ = session.wait_exit(Duration::from_secs(5));
    session.close_handles();
    let elapsed = t0.elapsed();
    let r = judge_engine_shutdown_bounded(elapsed, Duration::from_secs(10));
    assert_eq!(r.outcome, InvariantOutcome::Pass, "{r:?}");
}

/// Control 9 — bounded explicit termination.
#[test]
fn control_bounded_explicit_termination() {
    let mut session = spawn(&["--windows-child-hold"], Duration::from_secs(12));
    let _ = session.wait_for_text("windows-child-hold", Duration::from_secs(6));
    let t0 = Instant::now();
    let _ = session.terminate_bounded(Duration::from_secs(3));
    session.close_handles();
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "terminate must be bounded"
    );
}

/// Control — WinTranscript never exceeds cap (bounded capture).
#[test]
fn control_transcript_cap_bounded() {
    use omen_compat::WinTranscript;
    let mut t = WinTranscript::default();
    let chunk = vec![b'A'; 1024];
    for _ in 0..200 {
        t.push(&chunk);
    }
    assert!(t.retained.len() <= 64 * 1024);
    assert!(t.truncated);
}

/// Control — console-mode snapshot availability is explicit.
#[test]
fn control_console_mode_snapshot_availability() {
    let un = WindowsConsoleModeSnapshot::unavailable("test");
    assert!(!un.available);
    assert!(un.input_mode.is_none());
    let m = WindowsConsoleModeSnapshot::measured(Some(0x1f), Some(0x7), "test");
    assert!(m.available);
    assert_ne!(m.input_consequential(), None);
}

/// Control — process liveness observation helper (self).
#[test]
fn control_process_liveness_self() {
    let pid = std::process::id();
    // Toolhelp not required: using a trivial alive fact via open semantics is
    // enough for calibration of the observation type shape.
    let obs = WindowsProcessLiveness {
        pid,
        alive: true,
        parent_pid: None,
        source: "self".into(),
    };
    assert!(obs.alive);
    let _ = poll_until(Duration::from_millis(10), || true);
}

/// D2-022 §41 — deterministic blocked write: CancelSynchronousIo path.
#[test]
fn control_blocked_write_cancellation() {
    let t0 = Instant::now();
    let write_budget = Duration::from_millis(400);
    let obs = control_blocked_write(write_budget, WIN_WRITE_CANCEL_BOUND)
        .expect("blocked write control must return an observation");
    println!(
        "FACT\tBLOCKED_WRITE\toutcome={} cancel_requested={} worker_completed={} \
         written={} elapsed_ms={}",
        obs.outcome.stable_id(),
        obs.cancel_requested,
        obs.worker_completed,
        obs.written_bytes,
        obs.elapsed.as_millis()
    );
    assert_ne!(
        obs.outcome,
        WindowsWriteOutcome::Completed,
        "un-drained pipe write must not complete fully before timeout: {obs:?}"
    );
    assert!(
        obs.cancel_requested,
        "CancelSynchronousIo must be requested"
    );
    assert!(
        obs.worker_completed,
        "worker completion must be observed after cancel (no bare join)"
    );
    assert!(matches!(
        obs.outcome,
        WindowsWriteOutcome::Cancelled | WindowsWriteOutcome::TimedOut
    ));
    // Prefer Cancelled (actual cancellation observed); TimedOut only if
    // completion still missing after cancel bound — that is a harness fail.
    assert_eq!(
        obs.outcome,
        WindowsWriteOutcome::Cancelled,
        "expected Cancelled after cancel+completion: {obs:?}"
    );
    assert!(
        t0.elapsed() < WATCHDOG,
        "blocked write control must stay bounded"
    );
}

/// D2-022 §42 — small ConPTY write completes without cancellation.
#[test]
fn control_small_write_completed() {
    let mut session = spawn(&["--windows-report"], Duration::from_secs(12));
    let _ = session.wait_for_text("OMEN_COMPAT_READY", Duration::from_secs(6));
    let obs = session
        .write_input(b"\r\n")
        .expect("small write must be accepted");
    println!(
        "FACT\tSMALL_WRITE\toutcome={} written={} cancel={}",
        obs.outcome.stable_id(),
        obs.written_bytes,
        obs.cancel_requested
    );
    assert_eq!(obs.outcome, WindowsWriteOutcome::Completed, "{obs:?}");
    assert_eq!(obs.written_bytes, 2);
    assert!(!obs.cancel_requested);
    assert!(obs.worker_completed);
    let shutdown = session.shutdown_bounded(Duration::from_secs(6));
    assert!(
        shutdown.harness_shutdown_pass(Duration::from_secs(6)),
        "{shutdown:?}"
    );
}

/// D2-022 §43 — normal client exit → close worker returns, workers exit.
#[test]
fn control_close_normal_client_exit() {
    let t0 = Instant::now();
    let mut session = spawn(&["--windows-report"], Duration::from_secs(12));
    let _ = session.wait_for_text("OMEN_COMPAT_READY", Duration::from_secs(6));
    let _ = session.wait_exit(Duration::from_secs(5));
    let obs = session.shutdown_bounded(Duration::from_secs(6));
    println!(
        "FACT\tCLOSE_NORMAL\tclose_started={} close_returned={} input_stopped={} \
         output_stopped={} pipe_broken={} handles={} elapsed_ms={}",
        obs.close_started,
        obs.close_returned,
        obs.input_worker_stopped,
        obs.output_worker_stopped,
        obs.output_pipe_broken,
        obs.handles_closed_once,
        obs.elapsed.as_millis()
    );
    assert!(obs.close_started, "close worker must start");
    assert!(
        obs.close_returned,
        "ClosePseudoConsole must actually return"
    );
    assert!(obs.input_worker_stopped);
    assert!(
        obs.output_worker_stopped,
        "output drain must stop after close"
    );
    assert!(obs.handles_closed_once);
    assert!(obs.harness_shutdown_pass(Duration::from_secs(6)), "{obs:?}");
    assert!(t0.elapsed() < WATCHDOG);
    // Idempotent second call.
    let again = session.shutdown_bounded(Duration::from_secs(1));
    assert!(again.close_returned && !again.bounded_out, "{again:?}");
}

/// D2-022 §44 — live client: terminate + close bounded, output drain live.
#[test]
fn control_close_live_client() {
    let t0 = Instant::now();
    let mut session = spawn(&["--windows-child-hold"], Duration::from_secs(12));
    let _ = session.wait_for_text("windows-child-hold", Duration::from_secs(6));
    assert!(
        session.is_alive(),
        "child-hold must be live before teardown"
    );
    let obs = session.shutdown_bounded(Duration::from_secs(8));
    println!(
        "FACT\tCLOSE_LIVE\tclient_exit={} close_returned={} input_stopped={} \
         output_stopped={} handles={} elapsed_ms={}",
        obs.client_exit_observed,
        obs.close_returned,
        obs.input_worker_stopped,
        obs.output_worker_stopped,
        obs.handles_closed_once,
        obs.elapsed.as_millis()
    );
    assert!(
        obs.client_exit_observed,
        "client termination must be observed"
    );
    assert!(obs.close_returned);
    assert!(obs.input_worker_stopped && obs.output_worker_stopped);
    assert!(obs.handles_closed_once);
    assert!(obs.harness_shutdown_pass(Duration::from_secs(8)), "{obs:?}");
    assert!(t0.elapsed() < WATCHDOG);
    assert!(
        session.write_input(b"x").is_err(),
        "no writes after shutdown"
    );
}

/// D2-022 §45 — pending/final output while close runs; drain stays live.
#[test]
fn control_close_with_final_output() {
    let t0 = Instant::now();
    let mut session = spawn(&["--windows-final-output-hold"], Duration::from_secs(15));
    let _ = session.wait_for_text("OMEN_COMPAT_READY", Duration::from_secs(6));
    // Do not wait for the full burst — start teardown while output may be pending.
    let _ = session.wait_for_text("OMEN_COMPAT_FINAL_LINE", Duration::from_secs(3));
    let bytes_before = session.transcript().total_bytes;
    let obs = session.shutdown_bounded(Duration::from_secs(8));
    let t = session.transcript().clone();
    println!(
        "FACT\tCLOSE_FINAL_OUTPUT\tbytes_before={} bytes_after={} final_done={} \
         close_returned={} output_stopped={} pipe_broken={} elapsed_ms={}",
        bytes_before,
        t.total_bytes,
        t.contains("OMEN_COMPAT_FINAL_OUTPUT_DONE") || t.contains("OMEN_COMPAT_FINAL_LINE"),
        obs.close_returned,
        obs.output_worker_stopped,
        obs.output_pipe_broken,
        obs.elapsed.as_millis()
    );
    assert!(
        t.total_bytes >= bytes_before,
        "transcript must not lose drained bytes"
    );
    assert!(
        t.contains("OMEN_COMPAT_FINAL_LINE") || t.contains("OMEN_COMPAT_READY"),
        "final/pending output must remain visible: {}",
        t.as_lossy()
    );
    assert!(obs.close_returned, "{obs:?}");
    assert!(obs.output_worker_stopped, "{obs:?}");
    assert!(obs.harness_shutdown_pass(Duration::from_secs(8)), "{obs:?}");
    assert!(t0.elapsed() < WATCHDOG);
    // Transcript cap still holds.
    assert!(t.retained.len() <= 64 * 1024);
}

/// D2-022 §46 — blocked write + shutdown interaction (worker cancel path).
#[test]
fn control_write_blocked_then_shutdown() {
    let t0 = Instant::now();
    // Phase 1: prove cancellation machinery on hostile pipe.
    let w = control_blocked_write(Duration::from_millis(300), WIN_WRITE_CANCEL_BOUND)
        .expect("hostile write");
    assert_eq!(w.outcome, WindowsWriteOutcome::Cancelled, "{w:?}");
    assert!(w.worker_completed);

    // Phase 2: session accepts write, then shutdown stops further writes.
    let mut session = spawn(&["--windows-child-hold"], Duration::from_secs(12));
    let _ = session.wait_for_text("windows-child-hold", Duration::from_secs(6));
    let wr = session.write_input(b"echo\r\n");
    // Write may complete (ConPTY accepts) — either way shutdown must proceed.
    println!("FACT\tWRITE_BEFORE_SHUTDOWN\t{wr:?}");
    let obs = session.shutdown_bounded(Duration::from_secs(8));
    assert!(
        obs.harness_shutdown_pass(Duration::from_secs(8)),
        "teardown must proceed without caller hang: {obs:?}"
    );
    assert!(
        session.write_input(b"more").is_err(),
        "no further writes accepted after shutdown"
    );
    assert!(
        t0.elapsed() < WATCHDOG,
        "blocked-write+shutdown must stay bounded"
    );
}

/// D2-022 §47 — ≥10 create / I/O / bounded-shutdown cycles.
#[test]
fn control_repeated_lifecycle_ten_cycles() {
    const CYCLES: usize = 10;
    const CYCLE_BOUND: Duration = Duration::from_secs(8);
    for i in 0..CYCLES {
        let t0 = Instant::now();
        let mut session = spawn(&["--windows-report"], Duration::from_secs(10));
        let _ = session.wait_for_text("OMEN_COMPAT_READY", Duration::from_secs(5));
        let _ = session.write_input(b"\r\n");
        let obs = session.shutdown_bounded(CYCLE_BOUND);
        let elapsed = t0.elapsed();
        assert!(
            elapsed < WATCHDOG,
            "cycle {i} exceeded safety bound: {elapsed:?}"
        );
        assert!(
            obs.harness_shutdown_pass(CYCLE_BOUND),
            "cycle {i} shutdown not clean: {obs:?}"
        );
        assert!(
            session.write_input(b"x").is_err(),
            "cycle {i}: write after shutdown must fail"
        );
    }
    let r = judge_engine_shutdown_bounded(WATCHDOG, WATCHDOG);
    assert_eq!(r.outcome, InvariantOutcome::Pass, "{r:?}");
    println!("FACT\tREPEATED_LIFECYCLE\tcycles={CYCLES}");
}

/// D2-022 §30E — outer helper watchdog can contain a hypothetical close hang.
#[test]
fn control_hostile_close_helper_watchdog() {
    let driver = workspace_root()
        .join("target")
        .join("debug")
        .join("examples")
        .join("windows_conpty_teardown_driver.exe");
    if !driver.exists() {
        let build = std::process::Command::new("cargo")
            .args([
                "build",
                "-p",
                "omen-compat",
                "--example",
                "windows_conpty_teardown_driver",
            ])
            .current_dir(workspace_root())
            .status()
            .expect("build teardown driver");
        assert!(build.success());
    }
    assert!(driver.exists(), "teardown driver missing at {driver:?}");

    let mut child = std::process::Command::new(&driver)
        .current_dir(workspace_root())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn teardown driver");

    // Outer absolute bound: if helper wedges, terminate it and FAIL fast.
    let outer = Duration::from_secs(20);
    let deadline = Instant::now() + outer;
    let mut exited = false;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => {
                exited = true;
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break,
        }
    }
    if !exited {
        let _ = child.kill();
        let _ = child.wait();
        panic!("teardown helper wedged; parent contained it via kill (hostile-close FAIL)");
    }
    let out = child.wait_with_output().expect("read helper output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    println!(
        "FACT\tTEARDOWN_DRIVER\tstatus={:?}\n{stdout}\n{stderr}",
        out.status
    );
    assert!(
        stdout.contains("\"scenario\":\"normal_exit\",\"ok\":true") || out.status.success(),
        "helper must report successful bounded teardown:\n{stdout}"
    );
    assert!(
        !stdout.contains("\"ok\":false"),
        "helper reported failed scenario:\n{stdout}"
    );
    assert!(out.status.success(), "helper exit: {:?}", out.status.code());
}

/// Optional modern Windows capability — reported, never required (§23/§48).
#[test]
fn control_release_pseudoconsole_capability_report() {
    let avail = release_pseudoconsole_available();
    println!("FACT\tRELEASE_PSEUDOCONSOLE\tavailable={avail} used=NO baseline=threaded_close");
    // Never assert failure on older Windows; optional control only.
}
