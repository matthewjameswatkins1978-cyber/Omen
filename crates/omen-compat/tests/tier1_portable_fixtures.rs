//! Tier 1 — real fixture subprocess tests against omen-gremlin.
//!
//! Every test uses an outer hang guard in addition to the runner deadline so
//! a defect in timeout machinery cannot hang the suite that tests it.

use omen_compat::{
    CommandSpec, EnvPolicy, EvidenceGrade, InvariantId, InvariantOutcome, OutputBounds, StdinSpec,
    run,
};
use omen_test_fixtures::{GREMLIN_BIN, INTEGRATION_TIMEOUT, run_with_test_timeout};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
    EXE.get_or_init(resolve_gremlin_exe).clone()
}

fn gremlin_supports_compat_modes(exe: &Path) -> bool {
    std::process::Command::new(exe)
        .arg("--help")
        .output()
        .map(|output| {
            let text = String::from_utf8_lossy(&output.stdout);
            text.contains("stdin-report") && text.contains("exit-code")
        })
        .unwrap_or(false)
}

fn build_gremlin() {
    let build = std::process::Command::new("cargo")
        .args(["build", "-p", "omen-test-fixtures", "--bin", "omen-gremlin"])
        .current_dir(workspace_root())
        .status()
        .expect("on-demand gremlin build failed to start");
    assert!(build.success(), "on-demand gremlin build failed");
}

fn resolve_gremlin_exe() -> PathBuf {
    let mut path = std::env::current_exe().expect("failed to get current_exe");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    let name = if cfg!(windows) {
        "omen-gremlin.exe"
    } else {
        GREMLIN_BIN
    };
    let exe = path.join(name);
    let fallback = workspace_root().join("target").join("debug").join(name);

    for candidate in [&exe, &fallback] {
        if candidate.exists() && gremlin_supports_compat_modes(candidate) {
            return candidate.clone();
        }
    }

    // cargo check/clippy do not refresh bin artifacts; produce a real binary.
    build_gremlin();
    for candidate in [&exe, &fallback] {
        if candidate.exists() && gremlin_supports_compat_modes(candidate) {
            return candidate.clone();
        }
    }
    panic!("omen-gremlin with Compat M0 modes not found at {exe:?} or {fallback:?}");
}

fn assert_pass(result: &omen_compat::InvariantResult, label: &str) {
    assert_eq!(
        result.outcome,
        InvariantOutcome::Pass,
        "{label} expected PASS, got {:?}: {}",
        result.outcome,
        result.reason
    );
    assert_ne!(
        result.evidence_grade,
        EvidenceGrade::Unavailable,
        "{label} must not claim Unavailable evidence on PASS"
    );
}

async fn bound<T: Send + 'static>(name: &'static str, f: impl FnOnce() -> T + Send + 'static) -> T {
    run_with_test_timeout(name, INTEGRATION_TIMEOUT, |_ctx| async move {
        tokio::task::spawn_blocking(f)
            .await
            .expect("blocking worker panicked")
    })
    .await
}

fn base_spec(exe: &Path, args: &[&str], timeout: Duration) -> CommandSpec {
    let mut spec = CommandSpec::new(exe).timeout(timeout);
    for arg in args {
        spec = spec.arg(*arg);
    }
    // Inherit parent env then override a dedicated probe variable so PATH and
    // loader settings remain available while ENV_RECORDED stays secret-free.
    spec.cwd(workspace_root())
        .env(EnvPolicy::InheritWith(vec![(
            "OMEN_COMPAT_PROBE".into(),
            "m0-env".into(),
        )]))
        .stdin(StdinSpec::Closed)
}

#[tokio::test]
async fn demonstration_exit_7_preserves_cause() {
    let exe = gremlin_exe();
    let outcome = bound("demonstration_exit_7", move || {
        run(
            &base_spec(&exe, &["--exit-code", "7"], Duration::from_secs(5)),
            &OutputBounds::default(),
        )
    })
    .await;

    assert_pass(
        &outcome.check_bounded_wait_no_hang(),
        "BOUNDED_WAIT_NO_HANG",
    );
    let exit = outcome.check_exit_cause_preserved(7);
    assert_pass(&exit, "EXIT_CAUSE_PRESERVED");
    assert_eq!(
        exit.invariant,
        InvariantId::ExitCausePreserved,
        "observation: child exited code 7; result: PASS"
    );
    assert_eq!(outcome.exit_cause, Some(omen_compat::ExitCause::code(7)));
}

#[tokio::test]
async fn demonstration_closed_stdin_eof_zero_bytes() {
    let exe = gremlin_exe();
    let outcome = bound("demonstration_stdin_eof", move || {
        run(
            &base_spec(&exe, &["--stdin-report"], Duration::from_secs(5)),
            &OutputBounds::default(),
        )
    })
    .await;

    assert_pass(
        &outcome.check_bounded_wait_no_hang(),
        "BOUNDED_WAIT_NO_HANG",
    );
    let report = outcome
        .fixture_report()
        .expect("stdin-report must emit single-line JSON");
    assert_eq!(report["fixture"], "stdin-report");
    let bytes = report["bytes_read"].as_u64().expect("bytes_read");
    let eof = report["stdin_eof"].as_bool().expect("stdin_eof");
    assert_eq!(bytes, 0, "closed stdin must yield zero bytes");
    assert!(eof, "fixture must observe EOF");

    let zero =
        outcome.check_zero_unscripted_input_writes(&StdinSpec::Closed, Some(bytes), Some(eof));
    assert_pass(&zero, "ZERO_UNSCRIPTED_INPUT_WRITES");
    assert_pass(
        &outcome.check_descriptor_closure_early_exit(),
        "DESCRIPTOR_CLOSURE_EARLY_EXIT",
    );
}

#[tokio::test]
async fn demonstration_scripted_stdin_bytes_delivered_exactly() {
    let payload = b"compat-m0-bytes".to_vec();
    let exe = gremlin_exe();
    let spec = base_spec(&exe, &["--stdin-report"], Duration::from_secs(5))
        .stdin(StdinSpec::Bytes(payload.clone()));
    let expected_len = payload.len() as u64;
    let outcome = bound("demonstration_stdin_bytes", move || {
        run(&spec, &OutputBounds::default())
    })
    .await;

    let report = outcome.fixture_report().expect("stdin-report JSON");
    let bytes = report["bytes_read"].as_u64().unwrap();
    let eof = report["stdin_eof"].as_bool().unwrap();
    let result = outcome.check_zero_unscripted_input_writes(
        &StdinSpec::Bytes(payload),
        Some(bytes),
        Some(eof),
    );
    assert_pass(&result, "ZERO_UNSCRIPTED_INPUT_WRITES bytes path");
    assert_eq!(bytes, expected_len);
    assert!(eof);
}

#[tokio::test]
async fn demonstration_dual_stream_drains_both_without_deadlock() {
    let exe = gremlin_exe();
    let outcome = bound("demonstration_dual_stream", move || {
        run(
            &base_spec(&exe, &["--dual-stream"], Duration::from_secs(10)),
            &OutputBounds {
                max_stdout_bytes: 512 * 1024,
                max_stderr_bytes: 512 * 1024,
            },
        )
    })
    .await;

    assert_pass(
        &outcome.check_bounded_wait_no_hang(),
        "BOUNDED_WAIT_NO_HANG",
    );
    let drain = outcome.check_drain_both_streams_no_deadlock();
    assert_pass(&drain, "DRAIN_BOTH_STREAMS_NO_DEADLOCK");

    assert!(
        outcome.stdout.total_bytes >= 300 * 1024,
        "stdout_total={}",
        outcome.stdout.total_bytes
    );
    assert!(
        outcome.stderr.total_bytes >= 300 * 1024,
        "stderr_total={}",
        outcome.stderr.total_bytes
    );
    assert!(
        outcome.stdout_lossy().contains("DUAL_TICK"),
        "stdout must contain interleaved ticks"
    );
    assert!(
        outcome.stderr_lossy().contains("DUAL_TICK"),
        "stderr must contain interleaved ticks"
    );
    assert!(!outcome.timed_out);
}

#[tokio::test]
async fn demonstration_large_output_completes_with_explicit_truncation() {
    let bounds = OutputBounds {
        max_stdout_bytes: 64 * 1024,
        max_stderr_bytes: 64 * 1024,
    };
    let exe = gremlin_exe();
    let outcome = bound("demonstration_large_output", move || {
        run(
            &base_spec(&exe, &["--large-output"], Duration::from_secs(5)),
            &bounds,
        )
    })
    .await;

    assert_pass(
        &outcome.check_bounded_wait_no_hang(),
        "BOUNDED_WAIT_NO_HANG",
    );
    assert!(!outcome.timed_out);
    assert!(
        outcome.stdout.total_bytes >= 256 * 1024,
        "fixture should emit >=256KiB, got {}",
        outcome.stdout.total_bytes
    );
    assert!(
        outcome.stdout.truncated,
        "truncation must be explicitly recorded"
    );
    assert!(
        outcome.stdout.bytes.len() <= bounds.max_stdout_bytes,
        "kept bytes must respect bound"
    );
    assert!(
        outcome.observations.iter().any(|o| matches!(
            o,
            omen_compat::Observation::OutputTruncated {
                stream: omen_compat::StreamKind::Stdout,
                ..
            }
        )),
        "OutputTruncated observation required"
    );
    assert_pass(
        &outcome.check_drain_both_streams_no_deadlock(),
        "DRAIN_BOTH_STREAMS_NO_DEADLOCK",
    );
}

#[tokio::test]
async fn demonstration_timeout_returns_bounded_with_cleanup() {
    // Resolve fixture path before starting the elapsed clock so on-demand
    // builds cannot skew the outer bound assertion.
    let exe = gremlin_exe();
    let started = Instant::now();
    let outcome = bound("demonstration_timeout", move || {
        run(
            &base_spec(
                &exe,
                &["--sleep-bounded", "10000"],
                Duration::from_millis(400),
            ),
            &OutputBounds::default(),
        )
    })
    .await;

    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(8),
        "outer suite must not hang; elapsed={elapsed:?}"
    );
    assert!(outcome.timed_out, "fixture exceeded deadline => timed_out");
    assert_eq!(
        outcome.exit_cause,
        Some(omen_compat::ExitCause::TimeoutKill)
    );
    assert!(
        outcome.cleanup.attempted,
        "cleanup attempt must be recorded"
    );
    assert!(
        outcome
            .observations
            .iter()
            .any(|o| matches!(o, omen_compat::Observation::TimeoutObserved { .. })),
        "TimeoutObserved required"
    );
    assert!(
        outcome
            .observations
            .iter()
            .any(|o| matches!(o, omen_compat::Observation::CleanupAttempted { .. })),
        "CleanupAttempted required"
    );

    assert_pass(
        &outcome.check_bounded_wait_no_hang(),
        "BOUNDED_WAIT_NO_HANG",
    );
    let exit = outcome.check_exit_cause_preserved(7);
    assert_eq!(exit.outcome, InvariantOutcome::Fail);
    let drain = outcome.check_drain_both_streams_no_deadlock();
    assert_ne!(drain.outcome, InvariantOutcome::Pass);
}

#[tokio::test]
async fn demonstration_env_recorded_dedicated_test_variable() {
    let exe = gremlin_exe();
    let outcome = bound("demonstration_env", move || {
        run(
            &base_spec(
                &exe,
                &["--print-env", "OMEN_COMPAT_PROBE"],
                Duration::from_secs(5),
            ),
            &OutputBounds::default(),
        )
    })
    .await;

    assert_pass(
        &outcome.check_bounded_wait_no_hang(),
        "BOUNDED_WAIT_NO_HANG",
    );
    let line = outcome.stdout_lossy();
    assert!(
        line.contains("OMEN_COMPAT_PROBE=m0-env"),
        "fixture must observe dedicated value, got: {line}"
    );
    let fixture_value = line
        .lines()
        .find_map(|l| l.strip_prefix("OMEN_COMPAT_PROBE="))
        .map(|s| s.trim().to_string());
    let result =
        outcome.check_env_recorded("OMEN_COMPAT_PROBE", "m0-env", fixture_value.as_deref());
    assert_pass(&result, "ENV_RECORDED");

    for obs in &outcome.observations {
        if let omen_compat::Observation::EnvRecorded { key, .. } = obs {
            assert_eq!(key, "OMEN_COMPAT_PROBE");
        }
    }
}

#[tokio::test]
async fn demonstration_portable_child_identity_reported() {
    let exe = gremlin_exe();
    let outcome = bound("demonstration_portable_child", move || {
        run(
            &base_spec(&exe, &["--spawn-child-portable"], Duration::from_secs(5)),
            &OutputBounds::default(),
        )
    })
    .await;

    assert_pass(
        &outcome.check_bounded_wait_no_hang(),
        "BOUNDED_WAIT_NO_HANG",
    );
    let report = outcome
        .fixture_report()
        .expect("spawn-child-portable JSON report");
    assert_eq!(report["fixture"], "spawn-child-portable");
    let child_pid = report["child_pid"].as_u64().expect("child_pid");
    assert!(child_pid > 0);
    assert!(
        outcome
            .observations
            .iter()
            .any(|o| matches!(o, omen_compat::Observation::ProcessSpawned { .. })),
        "root spawn observation required"
    );
}

#[tokio::test]
async fn demonstration_exit_0_and_1_codes() {
    let exe = gremlin_exe();
    for code in [0_i32, 1_i32] {
        let expected = code;
        let spec_exe = exe.clone();
        let outcome = bound("demonstration_exit_codes", move || {
            run(
                &base_spec(
                    &spec_exe,
                    &["--exit-code", &expected.to_string()],
                    Duration::from_secs(5),
                ),
                &OutputBounds::default(),
            )
        })
        .await;
        assert_pass(
            &outcome.check_exit_cause_preserved(expected),
            "EXIT_CAUSE_PRESERVED",
        );
    }
}

#[test]
fn gremlin_binary_is_present_for_tier1() {
    let exe = gremlin_exe();
    assert!(exe.exists(), "omen-gremlin missing at {exe:?}");
}
