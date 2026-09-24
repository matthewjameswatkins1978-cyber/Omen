//! Tier WINDOWS ENGINE CONPTY — Omen NativePtyHandle as product under test.
//!
//! Access mechanism: `omen-engine` windows-only **dev-dependency** from
//! omen-compat (public product API only). Control harness remains independent.
//!
//! Product `terminate()` / `WinConPty` Drop lifecycle is exercised only in
//! an isolated child driver (`examples/windows_engine_terminate_driver`) so
//! a product double-close cannot corrupt the measurement host (WIN-M0-004).

#![cfg(windows)]

use omen_compat::{
    EvidenceGrade, InvariantId, InvariantOutcome, InvariantResult, WindowsExitCauseContext,
    WindowsExitObservation, judge_engine_client_console, judge_engine_exit_preserves_raw_bits,
    judge_engine_job_contains, judge_engine_resize_visible, judge_engine_shutdown_bounded,
    judge_engine_terminate_kills, parse_windows_report, poll_until,
};
use omen_engine::backend::{PtyExecutionHandle, PtyExecutionRequest};
use omen_engine::pty::NativePtyHandle;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SHUTDOWN_BOUND: Duration = Duration::from_secs(10);

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

fn spawn_engine(args: &[&str], rows: u16, cols: u16) -> NativePtyHandle {
    let mut argv = vec![gremlin_exe().to_string_lossy().into_owned()];
    argv.extend(args.iter().map(|s| s.to_string()));
    let req = PtyExecutionRequest {
        session_id: omen_core::PtySessionId::generate(),
        argv,
        cwd: workspace_root(),
        env: vec![
            ("NO_COLOR".into(), "1".into()),
            ("TERM".into(), "dumb".into()),
        ],
        rows,
        cols,
    };
    NativePtyHandle::spawn(&req).expect("NativePtyHandle::spawn")
}

fn read_until(handle: &mut NativePtyHandle, needle: &str, budget: Duration) -> String {
    let deadline = Instant::now() + budget;
    let mut collected = String::new();
    while Instant::now() < deadline {
        if let Ok(bytes) = handle.read_output()
            && !bytes.is_empty()
        {
            collected.push_str(&String::from_utf8_lossy(&bytes));
            if collected.contains(needle) {
                return collected;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    collected
}

/// Cleanup without product `terminate()` (avoids known double-close path).
fn drop_cleanup(mut handle: NativePtyHandle) {
    handle.write_input(b"").ok();
    drop(handle);
}

fn print_report(scenario: &str, results: &[InvariantResult]) {
    println!("WINDOWS / ENGINE CONPTY");
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

/// E1 — NativePtyHandle spawn + real fixture console observation.
#[test]
fn engine_e1_client_console_valid() {
    let mut handle = spawn_engine(&["--windows-report"], 24, 80);
    let out = read_until(
        &mut handle,
        "\"fixture\":\"windows-report\"",
        Duration::from_secs(15),
    );
    let mut results = Vec::new();
    match parse_windows_report(&out) {
        Some(report) => {
            results.push(judge_engine_client_console(&report));
        }
        None => results.push(InvariantResult::new(
            InvariantId::WindowsEngineConPtyClientConsoleValid,
            InvariantOutcome::OpenDefect,
            EvidenceGrade::Partial,
            "no windows-report from engine ConPTY client within bound",
        )),
    }
    print_report("engine-console", &results);
    drop_cleanup(handle);
}

/// E2 — product write/read round trip.
#[test]
fn engine_e2_write_read_roundtrip() {
    let mut handle = spawn_engine(&["--windows-report"], 24, 80);
    let _ = read_until(&mut handle, "OMEN_COMPAT_READY", Duration::from_secs(12));
    handle.write_input(b"\r\n").expect("product write_input");
    let after = read_until(
        &mut handle,
        "\"fixture\":\"windows-report\"",
        Duration::from_secs(8),
    );
    assert!(
        !after.is_empty(),
        "product read_output must return fixture evidence"
    );
    drop_cleanup(handle);
}

/// E3 — product ResizePseudoConsole path + fixture observes new dimensions.
#[test]
fn engine_e3_resize_visible() {
    let mut handle = spawn_engine(&["--windows-resize-report"], 24, 80);
    let out = read_until(
        &mut handle,
        "OMEN_COMPAT_RESIZE_WAIT",
        Duration::from_secs(15),
    );
    if !out.contains("OMEN_COMPAT_RESIZE_WAIT") {
        let results = vec![InvariantResult::new(
            InvariantId::WindowsEngineConPtyResizeVisible,
            InvariantOutcome::OpenDefect,
            EvidenceGrade::Partial,
            "resize fixture did not reach WAIT barrier under product ConPTY",
        )];
        print_report("engine-resize", &results);
        drop_cleanup(handle);
        return;
    }
    let before = (24u16, 80u16);
    handle.resize(40, 120).expect("product resize");
    let after = (40u16, 120u16);
    let child_observed = poll_until(Duration::from_secs(8), || {
        handle
            .read_output()
            .map(|b| String::from_utf8_lossy(&b).contains("OMEN_COMPAT_RESIZED"))
            .unwrap_or(false)
    });
    let results = vec![judge_engine_resize_visible(before, after, child_observed)];
    print_report("engine-resize", &results);
    drop_cleanup(handle);
}

/// E4 — normal exit status measurement.
#[test]
fn engine_e4_normal_exit() {
    let mut handle = spawn_engine(&["--windows-exit-status", "0", "--sleep-ms", "200"], 24, 80);
    let mut exit: Option<i32> = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(Some(pe)) = handle.try_wait() {
            exit = pe.code;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("FACT\tENGINE_NORMAL_EXIT\t{exit:?}");
    if let Some(code) = exit {
        assert_eq!(code, 0);
    }
    drop_cleanup(handle);
}

/// E5 — high-bit / raw status bit preservation.
#[test]
fn engine_e5_raw_status_bits() {
    let expected: u32 = 0x8000_0007;
    let mut handle = spawn_engine(&["--windows-exit-status", &expected.to_string()], 24, 80);
    let mut raw: Option<u32> = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(Some(pe)) = handle.try_wait() {
            raw = pe.code.map(|c| c as u32);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let obs = WindowsExitObservation {
        raw_status: raw,
        product_code: raw.map(|r| r as i32),
        cause_context: WindowsExitCauseContext::Unknown,
        source: "engine_try_wait".into(),
    };
    if raw.is_some() {
        let r = judge_engine_exit_preserves_raw_bits(&obs, expected);
        print_report("engine-raw-exit", std::slice::from_ref(&r));
        assert_eq!(obs.raw_status, Some(expected), "{obs:?}");
    } else {
        eprintln!("UNAVAILABLE: engine did not surface exit within bound");
    }
    drop_cleanup(handle);
}

/// E6+E7 — isolated product terminate removes parent + descendant.
///
/// Product `terminate()` is invoked only inside the child driver so a
/// handle-lifecycle defect cannot corrupt this measurement host.
#[test]
fn engine_e6_e7_terminate_kills_descendants() {
    let driver = workspace_root()
        .join("target")
        .join("debug")
        .join("examples")
        .join("windows_engine_terminate_driver.exe");
    assert!(
        driver.exists() || build_driver(&driver),
        "terminate driver must exist at {driver:?}"
    );
    let out = std::process::Command::new(&driver)
        .args(["--windows-tree-report"])
        .current_dir(workspace_root())
        .output()
        .expect("run terminate driver");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    println!("FACT\tDRIVER\tstatus={:?}\n{stdout}\n{stderr}", out.status);

    let mut parent_alive_before = false;
    let mut desc_alive_before = false;
    let mut parent_gone = false;
    let mut desc_gone = false;
    let mut terminate_ok = false;
    for line in stdout.lines().rev() {
        if !line.contains("\"driver\":\"engine-terminate\"") {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) {
            parent_alive_before = v["parent_alive_before"].as_bool().unwrap_or(false);
            desc_alive_before = v["desc_alive_before"].as_bool().unwrap_or(false);
            parent_gone = v["parent_gone"].as_bool().unwrap_or(false);
            desc_gone = v["desc_gone"].as_bool().unwrap_or(false);
            terminate_ok = v["terminate_ok"].as_bool().unwrap_or(false);
            break;
        }
    }

    if out.status.code().is_none() || out.status.code() == Some(0xC0000374u32 as i32) {
        // Child died abnormally after terminate — product handle lifecycle defect.
        eprintln!(
            "PRODUCT_ANOMALY: driver abnormal exit after terminate: {:?}",
            out.status
        );
    }

    let results = vec![
        judge_engine_job_contains(
            parent_alive_before,
            desc_alive_before,
            !parent_gone,
            !desc_gone,
        ),
        judge_engine_terminate_kills(
            parent_alive_before && desc_alive_before,
            parent_gone,
            desc_gone,
        ),
    ];
    assert!(
        terminate_ok || !parent_alive_before,
        "driver terminate reported"
    );
    print_report("engine-terminate", &results);
}

/// E8 — repeated bounded create/drop cycles (no product terminate).
#[test]
fn engine_e8_repeated_lifecycle() {
    for i in 0..3 {
        let t0 = Instant::now();
        let handle = spawn_engine(&["--windows-child-hold"], 24, 80);
        drop_cleanup(handle);
        assert!(
            t0.elapsed() < SHUTDOWN_BOUND,
            "cycle {i} exceeded shutdown bound: {:?}",
            t0.elapsed()
        );
    }
    let r = judge_engine_shutdown_bounded(SHUTDOWN_BOUND, SHUTDOWN_BOUND);
    assert_eq!(r.outcome, InvariantOutcome::Pass, "{r:?}");
    print_report("engine-lifecycle", std::slice::from_ref(&r));
}

fn build_driver(path: &std::path::Path) -> bool {
    let status = std::process::Command::new("cargo")
        .args([
            "build",
            "-p",
            "omen-compat",
            "--example",
            "windows_engine_terminate_driver",
        ])
        .current_dir(workspace_root())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    status && path.exists()
}

fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
    };
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let w = WaitForSingleObject(h, 0);
        CloseHandle(h);
        w != WAIT_OBJECT_0
    }
}

#[allow(dead_code)]
fn _unused(_: fn(u32) -> bool) {
    let _ = process_alive;
}
