//! Tier 0 — pure schema and state tests (no subprocesses).

use omen_compat::{
    Capability, CleanupOutcome, CommandSpec, Deadline, EnvPolicy, EvidenceGrade, ExitCause,
    FailureKind, InvariantId, InvariantOutcome, InvariantResult, Observation, OutputBounds,
    Platform, ReplayDescriptor, RunOutcome, StdinSpec, StreamKind, StructuredFailure,
};
use std::time::Duration;

#[test]
fn exit_cause_preserves_portable_code() {
    let cause = ExitCause::code(7);
    assert_eq!(cause.as_code(), Some(7));
    assert_eq!(cause.stable_id(), "code:7");

    let json = serde_json::to_string(&cause).unwrap();
    let back: ExitCause = serde_json::from_str(&json).unwrap();
    assert_eq!(back, cause);
    assert_eq!(back.as_code(), Some(7));
}

#[test]
fn exit_cause_timeout_and_unknown_roundtrip() {
    for cause in [
        ExitCause::TimeoutKill,
        ExitCause::OmenCancel,
        ExitCause::Unknown {
            evidence: "opaque".into(),
        },
        ExitCause::Signal { signal: 9 },
        ExitCause::WindowsStatus { code: 0xC000_013A },
    ] {
        let json = serde_json::to_string(&cause).unwrap();
        let back: ExitCause = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cause);
    }
}

#[test]
fn invariant_ids_are_stable_machine_ids() {
    assert_eq!(
        InvariantId::BoundedWaitNoHang.stable_id(),
        "BOUNDED_WAIT_NO_HANG"
    );
    assert_eq!(
        InvariantId::ExitCausePreserved.stable_id(),
        "EXIT_CAUSE_PRESERVED"
    );
    assert_eq!(
        InvariantId::DrainBothStreamsNoDeadlock.stable_id(),
        "DRAIN_BOTH_STREAMS_NO_DEADLOCK"
    );
    assert_eq!(
        InvariantId::DescriptorClosureEarlyExit.stable_id(),
        "DESCRIPTOR_CLOSURE_EARLY_EXIT"
    );
    assert_eq!(
        InvariantId::ZeroUnscriptedInputWrites.stable_id(),
        "ZERO_UNSCRIPTED_INPUT_WRITES"
    );
    assert_eq!(InvariantId::EnvRecorded.stable_id(), "ENV_RECORDED");

    for id in InvariantId::ALL {
        let json = serde_json::to_string(&id).unwrap();
        let back: InvariantId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
        assert!(InvariantId::from_stable_id(id.stable_id()).is_some());
    }
}

#[test]
fn invariant_outcomes_are_richer_than_bool() {
    assert_eq!(InvariantOutcome::Pass.stable_id(), "PASS");
    assert_eq!(InvariantOutcome::Fail.stable_id(), "FAIL");
    assert_eq!(InvariantOutcome::Unsupported.stable_id(), "UNSUPPORTED");
    assert_eq!(InvariantOutcome::Unavailable.stable_id(), "UNAVAILABLE");
    assert_eq!(InvariantOutcome::OpenDefect.stable_id(), "OPEN_DEFECT");
    assert_eq!(InvariantOutcome::Inconclusive.stable_id(), "INCONCLUSIVE");
    assert!(InvariantOutcome::Fail.is_failure());
    assert!(!InvariantOutcome::Pass.is_failure());
}

#[test]
fn evidence_grade_never_silently_upgrades() {
    assert!(EvidenceGrade::Strong > EvidenceGrade::Partial);
    assert_eq!(
        EvidenceGrade::Strong.weaker(EvidenceGrade::Partial),
        EvidenceGrade::Partial
    );
    assert_eq!(
        EvidenceGrade::Weak.weaker(EvidenceGrade::Unavailable),
        EvidenceGrade::Unavailable
    );
    assert_eq!(
        EvidenceGrade::Partial.weaker(EvidenceGrade::Strong),
        EvidenceGrade::Partial
    );
}

#[test]
fn observation_and_invariant_result_serialization() {
    let obs = Observation::ExitObserved {
        cause: ExitCause::code(7),
    };
    let json = serde_json::to_string(&obs).unwrap();
    let back: Observation = serde_json::from_str(&json).unwrap();
    assert_eq!(back, obs);

    let result = InvariantResult::pass(
        InvariantId::ExitCausePreserved,
        EvidenceGrade::Strong,
        "exact code 7",
    )
    .with_observations(vec![obs.clone()]);
    let json = serde_json::to_string(&result).unwrap();
    let back: InvariantResult = serde_json::from_str(&json).unwrap();
    assert_eq!(back, result);
    assert_eq!(back.invariant, InvariantId::ExitCausePreserved);
    assert_eq!(back.outcome, InvariantOutcome::Pass);
}

#[test]
fn command_spec_has_no_shell_string_field() {
    let spec = CommandSpec::new("/bin/echo")
        .arg("hello")
        .arg("world")
        .cwd("/tmp")
        .env(EnvPolicy::AllowList(vec![(
            "OMEN_COMPAT_PROBE".into(),
            "m0".into(),
        )]))
        .stdin(StdinSpec::Closed)
        .timeout(Duration::from_millis(250));
    assert_eq!(spec.argv, vec!["hello".to_string(), "world".to_string()]);
    assert_eq!(spec.deadline.timeout_ms, 250);
    let json = serde_json::to_string(&spec).unwrap();
    assert!(!json.contains("shell"));
    let back: CommandSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(back, spec);
}

#[test]
fn replay_descriptor_roundtrip() {
    let spec = CommandSpec::new("omen-gremlin")
        .arg("--exit")
        .arg("7")
        .stdin(StdinSpec::Closed)
        .timeout(Duration::from_millis(500));
    let replay = ReplayDescriptor::from_command(
        &spec,
        "exit-code",
        vec![
            InvariantId::ExitCausePreserved,
            InvariantId::BoundedWaitNoHang,
        ],
    );
    let json = serde_json::to_string(&replay).unwrap();
    let back: ReplayDescriptor = serde_json::from_str(&json).unwrap();
    assert_eq!(back, replay);
    assert_eq!(back.fixture_mode, "exit-code");
    assert_eq!(back.expected_invariants.len(), 2);
    assert!(matches!(back.stdin, StdinSpec::Closed));
}

#[test]
fn capability_and_platform_schema() {
    assert_eq!(Capability::ProcessSpawn.stable_id(), "PROCESS_SPAWN");
    assert_eq!(
        Capability::PosixControllingTerminal.stable_id(),
        "POSIX_CONTROLLING_TERMINAL"
    );
    assert_eq!(Capability::Win32ConPty.stable_id(), "WIN32_CONPTY");
    assert_eq!(Capability::portable_initial_set().len(), 5);

    let platform = Platform::current();
    let json = serde_json::to_string(&platform).unwrap();
    let back: Platform = serde_json::from_str(&json).unwrap();
    assert_eq!(back, platform);
    assert_eq!(Platform::Portable.stable_id(), "PORTABLE");
}

#[test]
fn deadline_is_explicit_and_serialized() {
    let d = Deadline::from_duration(Duration::from_secs(2));
    assert_eq!(d.timeout_ms, 2000);
    assert!(!d.is_zero());
    let json = serde_json::to_string(&d).unwrap();
    let back: Deadline = serde_json::from_str(&json).unwrap();
    assert_eq!(back, d);
}

#[test]
fn structured_failure_intentional_mismatch_is_replayable() {
    // Deliberately judge a wrong expected code to prove the failure record.
    let outcome = RunOutcome {
        observations: vec![
            Observation::ProcessSpawned { pid: 4242 },
            Observation::ExitObserved {
                cause: ExitCause::code(7),
            },
            Observation::OutputComplete {
                stream: StreamKind::Stdout,
                total_bytes: 0,
            },
        ],
        exit_cause: Some(ExitCause::code(7)),
        stdout: omen_compat::CapturedStream::default(),
        stderr: omen_compat::CapturedStream::default(),
        timed_out: false,
        cleanup: CleanupOutcome::default(),
        elapsed_ms: 12,
        deadline_ms: 1000,
        bounds: OutputBounds::default(),
        harness_failed: false,
        pid: Some(4242),
    };

    let result = outcome.check_exit_cause_preserved(1); // expected 1, got 7
    assert_eq!(result.outcome, InvariantOutcome::Fail);

    let spec = CommandSpec::new("omen-gremlin")
        .arg("--exit")
        .arg("7")
        .timeout(Duration::from_millis(1000));

    let failure = StructuredFailure::builder(
        InvariantId::ExitCausePreserved,
        Platform::current(),
        "tier0_intentional_exit_mismatch",
        FailureKind::Contradiction,
        result.evidence_grade,
        result.reason.clone(),
    )
    .seed(7)
    .command(spec.clone())
    .exit_cause(ExitCause::code(7))
    .stdout(RunOutcome::capture_summary(&outcome.stdout))
    .stderr(RunOutcome::capture_summary(&outcome.stderr))
    .observations(outcome.observations.clone())
    .result(result.clone())
    .replay(ReplayDescriptor::from_command(
        &spec,
        "exit-code",
        vec![InvariantId::ExitCausePreserved],
    ))
    .minimal_reproducer("omen-gremlin --exit-code 7")
    .likely_subsystem("compat.runner.exit_cause")
    .output_bounds(outcome.bounds)
    .build();

    assert_eq!(failure.invariant, InvariantId::ExitCausePreserved);
    assert_eq!(failure.failure_kind, FailureKind::Contradiction);
    assert_eq!(failure.seed, Some(7));
    assert!(failure.reason.contains("expected exit code 1"));
    assert!(failure.reason.contains("observed 7"));
    assert_eq!(failure.exit_cause, Some(ExitCause::code(7)));
    assert!(failure.replay.is_some());
    assert_eq!(failure.replay.as_ref().unwrap().fixture_mode, "exit-code");
    assert!(!failure.observations.is_empty());
    assert!(failure.minimal_reproducer.as_deref().unwrap().contains("7"));

    let json = serde_json::to_string(&failure).unwrap();
    let back: StructuredFailure = serde_json::from_str(&json).unwrap();
    assert_eq!(back.invariant, failure.invariant);
    assert_eq!(back.reason, failure.reason);

    // This unit test asserts on the failure record; it does not remain a
    // failing test in the normal suite.
    assert_eq!(result.outcome, InvariantOutcome::Fail);
}
