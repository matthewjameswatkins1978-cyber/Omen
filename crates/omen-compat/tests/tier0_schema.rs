//! Tier 0 — pure schema and state tests (no subprocesses).

use omen_compat::{
    Capability, CleanupOutcome, CommandEvidence, CommandSpec, Deadline, EnvPolicy, EvidenceGrade,
    ExitCause, FailureKind, InvariantId, InvariantOutcome, InvariantResult, Observation,
    OutputBounds, Platform, ReplayDescriptor, ReplayExactnessError, ReplayFidelity, RunOutcome,
    StdinSpec, StreamKind, StructuredFailure, declared_max_wall_ms,
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
fn command_evidence_is_redacted_by_default() {
    let secret = "OMEN_TEST_SECRET_DO_NOT_LEAK_9f3a";
    let spec = CommandSpec::new("omen-gremlin")
        .arg("--print-env")
        .arg("OPENAI_API_KEY")
        .env(EnvPolicy::InheritWith(vec![(
            "OPENAI_API_KEY".into(),
            secret.into(),
        )]))
        .stdin(StdinSpec::Bytes(secret.as_bytes().to_vec()))
        .timeout(Duration::from_millis(500));

    let evidence = CommandEvidence::from_execution(&spec);
    let json = serde_json::to_string(&evidence).unwrap();
    assert!(
        !json.contains(secret),
        "evidence must not retain env/stdin values"
    );
    assert_eq!(evidence.argv_count, 2);
    assert!(evidence.argv_safe.is_none());
    assert_eq!(evidence.stdin_byte_length, Some(secret.len()));
    assert_eq!(evidence.env_keys, vec!["OPENAI_API_KEY".to_string()]);
    assert_eq!(evidence.env_policy_kind, "inherit_with");
}

#[test]
fn replay_descriptor_redacted_and_exact_fixture() {
    let secret = "OMEN_TEST_SECRET_DO_NOT_LEAK_9f3a";
    let hostile = CommandSpec::new("omen-gremlin")
        .arg("--exit")
        .arg("7")
        .env(EnvPolicy::InheritWith(vec![(
            "OPENAI_API_KEY".into(),
            secret.into(),
        )]))
        .stdin(StdinSpec::Closed)
        .timeout(Duration::from_millis(500));

    let redacted = ReplayDescriptor::redacted(
        &hostile,
        "exit-code",
        vec![
            InvariantId::ExitCausePreserved,
            InvariantId::BoundedWaitNoHang,
        ],
    );
    assert_eq!(redacted.fidelity(), ReplayFidelity::Redacted);
    assert!(redacted.argv().is_empty());
    let json = serde_json::to_string(&redacted).unwrap();
    assert!(!json.contains(secret));
    assert_eq!(redacted.fixture_mode(), "exit-code");
    assert_eq!(redacted.expected_invariants().len(), 2);
    assert_eq!(redacted.stdin_mode(), "closed");

    // Secret-bearing env must block Exact.
    let blocked = ReplayDescriptor::try_exact_fixture(
        &hostile,
        "exit-code",
        vec![InvariantId::ExitCausePreserved],
        vec!["--exit".into(), "7".into()],
    );
    assert_eq!(
        blocked.expect_err("env values must block Exact"),
        ReplayExactnessError::EnvPolicyNotClear
    );

    // Safe controlled fixture: Clear env, Closed stdin, no cwd, safe argv.
    let safe = CommandSpec::new("omen-gremlin")
        .arg("--exit-code")
        .arg("7")
        .env(EnvPolicy::Clear)
        .stdin(StdinSpec::Closed)
        .timeout(Duration::from_millis(500));
    let exact = ReplayDescriptor::try_exact_fixture(
        &safe,
        "exit-code",
        vec![InvariantId::ExitCausePreserved],
        vec!["--exit-code".into(), "7".into()],
    )
    .expect("safe fixture must be Exact");
    assert_eq!(exact.fidelity(), ReplayFidelity::Exact);
    assert_eq!(
        exact.argv(),
        vec!["--exit-code".to_string(), "7".to_string()]
    );
    let json = serde_json::to_string(&exact).unwrap();
    assert!(!json.contains(secret));
    assert!(!json.contains("OPENAI_API_KEY="));
}

#[test]
fn exact_replay_blocked_by_env_stdin_cwd_or_unsafe_argv() {
    // B: env secret present
    let with_env = CommandSpec::new("omen-gremlin")
        .arg("--exit-code")
        .arg("7")
        .env(EnvPolicy::InheritWith(vec![(
            "OPENAI_API_KEY".into(),
            "s".into(),
        )]))
        .stdin(StdinSpec::Closed)
        .timeout(Duration::from_millis(200));
    assert_eq!(
        ReplayDescriptor::try_exact_fixture(
            &with_env,
            "m",
            vec![],
            vec!["--exit-code".into(), "7".into()]
        )
        .unwrap_err(),
        ReplayExactnessError::EnvPolicyNotClear
    );

    // C: stdin payload present
    let with_stdin = CommandSpec::new("omen-gremlin")
        .arg("--exit-code")
        .arg("7")
        .env(EnvPolicy::Clear)
        .stdin(StdinSpec::Bytes(b"payload".to_vec()))
        .timeout(Duration::from_millis(200));
    assert_eq!(
        ReplayDescriptor::try_exact_fixture(
            &with_stdin,
            "m",
            vec![],
            vec!["--exit-code".into(), "7".into()]
        )
        .unwrap_err(),
        ReplayExactnessError::StdinNotClosed
    );

    // D: cwd present but omitted from exact material
    let with_cwd = CommandSpec::new("omen-gremlin")
        .arg("--exit-code")
        .arg("7")
        .cwd("/tmp/compat-exact")
        .env(EnvPolicy::Clear)
        .stdin(StdinSpec::Closed)
        .timeout(Duration::from_millis(200));
    assert_eq!(
        ReplayDescriptor::try_exact_fixture(
            &with_cwd,
            "m",
            vec![],
            vec!["--exit-code".into(), "7".into()]
        )
        .unwrap_err(),
        ReplayExactnessError::CwdSet
    );

    // E: argv not explicitly safe / mismatched
    let safe_shape = CommandSpec::new("omen-gremlin")
        .arg("--exit-code")
        .arg("7")
        .env(EnvPolicy::Clear)
        .stdin(StdinSpec::Closed)
        .timeout(Duration::from_millis(200));
    assert_eq!(
        ReplayDescriptor::try_exact_fixture(&safe_shape, "m", vec![], vec![]).unwrap_err(),
        ReplayExactnessError::SafeArgvMismatch
    );
    assert_eq!(
        ReplayDescriptor::try_exact_fixture(
            &safe_shape,
            "m",
            vec![],
            vec!["--exit-code".into(), "8".into()]
        )
        .unwrap_err(),
        ReplayExactnessError::SafeArgvMismatch
    );

    // False Exact attack: same-length wrong argv via evidence projection.
    // CommandEvidence cannot prove argv equality against original execution.
    let original = CommandSpec::new("omen-gremlin")
        .arg("--exit-code")
        .arg("7")
        .env(EnvPolicy::Clear)
        .stdin(StdinSpec::Closed)
        .timeout(Duration::from_millis(200));
    let false_safe_argv = vec!["completely".to_string(), "different".to_string()];
    assert_eq!(false_safe_argv.len(), original.argv.len());
    let evidence =
        CommandEvidence::from_execution(&original).with_safe_argv(false_safe_argv.clone());
    // No CommandEvidence-based API can produce Exact.
    let projected = ReplayDescriptor::redacted_from_evidence(evidence, "m", vec![]);
    assert_eq!(projected.fidelity(), ReplayFidelity::Redacted);
    assert!(projected.argv().is_empty());
    // try_exact_fixture rejects same-length wrong argv against original spec.
    assert_eq!(
        ReplayDescriptor::try_exact_fixture(&original, "m", vec![], false_safe_argv).unwrap_err(),
        ReplayExactnessError::SafeArgvMismatch
    );
}

#[test]
fn replay_fidelity_serialization_roundtrip() {
    for fidelity in [
        ReplayFidelity::Exact,
        ReplayFidelity::Partial,
        ReplayFidelity::Redacted,
    ] {
        let json = serde_json::to_string(&fidelity).unwrap();
        let back: ReplayFidelity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, fidelity);
    }
    assert_eq!(
        serde_json::to_string(&ReplayFidelity::Exact).unwrap(),
        "\"exact\""
    );
    assert_eq!(
        serde_json::to_string(&ReplayFidelity::Partial).unwrap(),
        "\"partial\""
    );
    assert_eq!(
        serde_json::to_string(&ReplayFidelity::Redacted).unwrap(),
        "\"redacted\""
    );
}

/// Type seal: no public fidelity field; Exact assigned only in try_exact_fixture.
#[test]
fn exact_fidelity_has_single_source_authority() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/failure.rs"),
    )
    .expect("read failure.rs");

    assert!(
        !src.contains("pub fidelity"),
        "fidelity field must not be public"
    );
    assert!(
        !src.contains("pub fn set_fidelity")
            && !src.contains("pub fn with_fidelity")
            && !src.contains("pub fn make_exact")
            && !src.contains("pub fn unchecked_exact"),
        "no public Exact setter/constructor may exist"
    );
    assert!(
        !src.contains("pub fn from_evidence("),
        "caller-supplied fidelity constructor must not exist"
    );
    assert!(src.contains("pub fn try_exact_fixture"));
    assert!(src.contains("pub fn redacted_from_evidence"));
    assert!(
        src.contains("Exact replay requires validation against original CommandSpec"),
        "custom Deserialize must reject Exact with explicit reason"
    );

    let exact_assign = "fidelity: ReplayFidelity::Exact";
    let assignments = src.matches(exact_assign).count();
    assert_eq!(
        assignments, 1,
        "ReplayFidelity::Exact must be assigned exactly once (try_exact_fixture)"
    );

    let try_pos = src
        .find("pub fn try_exact_fixture")
        .expect("try_exact_fixture must exist");
    let assign_pos = src.find(exact_assign).expect("Exact assignment must exist");
    let after_sig = try_pos + "pub fn try_exact_fixture".len();
    let next_pub_fn = src[after_sig..]
        .find("    pub fn ")
        .map(|i| after_sig + i)
        .unwrap_or(src.len());
    assert!(
        assign_pos > try_pos && assign_pos < next_pub_fn,
        "Exact must only be assigned inside try_exact_fixture (try={try_pos} assign={assign_pos} next={next_pub_fn})"
    );
}

/// Raw JSON claiming Exact is rejected by generic deserialization.
#[test]
fn raw_json_exact_is_rejected() {
    let spec = CommandSpec::new("omen-gremlin")
        .arg("--exit-code")
        .arg("7")
        .env(EnvPolicy::Clear)
        .stdin(StdinSpec::Closed)
        .timeout(Duration::from_millis(200));
    let redacted = ReplayDescriptor::redacted(&spec, "exit-code", vec![]);
    let mut json: serde_json::Value = serde_json::to_value(&redacted).expect("serialize redacted");
    json["fidelity"] = serde_json::json!("exact");
    let text = serde_json::to_string(&json).unwrap();
    let err = serde_json::from_str::<ReplayDescriptor>(&text)
        .expect_err("raw Exact JSON must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("Exact replay requires validation against original CommandSpec"),
        "error must explain Exact requires validation: {msg}"
    );
}

/// Trusted in-memory Exact can serialize; generic Deserialize cannot restore it.
#[test]
fn serialized_valid_exact_cannot_generic_deserialize() {
    let spec = CommandSpec::new("omen-gremlin")
        .arg("--exit-code")
        .arg("7")
        .env(EnvPolicy::Clear)
        .stdin(StdinSpec::Closed)
        .timeout(Duration::from_millis(200));
    let exact = ReplayDescriptor::try_exact_fixture(
        &spec,
        "exit-code",
        vec![InvariantId::ExitCausePreserved],
        vec!["--exit-code".into(), "7".into()],
    )
    .expect("safe Exact");
    assert_eq!(exact.fidelity(), ReplayFidelity::Exact);
    let json = serde_json::to_string(&exact).unwrap();
    assert!(json.contains("\"fidelity\":\"exact\""));
    let err = serde_json::from_str::<ReplayDescriptor>(&json)
        .expect_err("serialized Exact must not generic-deserialize");
    assert!(err.to_string().contains("Exact replay requires validation"));
}

/// Nested Exact inside StructuredFailure fails closed on generic deserialize.
#[test]
fn structured_failure_nested_exact_deserialize_fails_closed() {
    let spec = CommandSpec::new("omen-gremlin")
        .arg("--exit-code")
        .arg("7")
        .env(EnvPolicy::Clear)
        .stdin(StdinSpec::Closed)
        .timeout(Duration::from_millis(200));
    let exact = ReplayDescriptor::try_exact_fixture(
        &spec,
        "exit-code",
        vec![InvariantId::ExitCausePreserved],
        vec!["--exit-code".into(), "7".into()],
    )
    .expect("safe Exact");
    let failure = StructuredFailure::builder(
        InvariantId::ExitCausePreserved,
        Platform::current(),
        "nested_exact",
        FailureKind::Contradiction,
        EvidenceGrade::Strong,
        "nested exact must fail closed",
    )
    .replay(exact)
    .build();
    let json = serde_json::to_string(&failure).unwrap();
    assert!(json.contains("\"fidelity\":\"exact\""));
    let err = serde_json::from_str::<StructuredFailure>(&json)
        .expect_err("nested Exact must fail closed");
    assert!(err.to_string().contains("Exact replay requires validation"));
}

/// Redacted StructuredFailure still round-trips generically.
#[test]
fn structured_failure_redacted_replay_roundtrip() {
    let spec = CommandSpec::new("omen-gremlin")
        .arg("--exit-code")
        .arg("7")
        .env(EnvPolicy::Clear)
        .stdin(StdinSpec::Closed)
        .timeout(Duration::from_millis(200));
    let failure = StructuredFailure::builder(
        InvariantId::ExitCausePreserved,
        Platform::current(),
        "redacted_roundtrip",
        FailureKind::Contradiction,
        EvidenceGrade::Strong,
        "redacted replay round-trip",
    )
    .replay(ReplayDescriptor::redacted(
        &spec,
        "exit-code",
        vec![InvariantId::ExitCausePreserved],
    ))
    .build();
    let json = serde_json::to_string(&failure).unwrap();
    let back: StructuredFailure = serde_json::from_str(&json).unwrap();
    assert_eq!(back.invariant, failure.invariant);
    let replay = back.replay.as_ref().expect("replay present");
    assert_eq!(replay.fidelity(), ReplayFidelity::Redacted);
    assert_eq!(replay.fixture_mode(), "exit-code");
    assert_eq!(back, failure);
}

/// Safe exact fixture serializes cleanly with validated argv.
#[test]
fn exact_fixture_serialization_preserves_validated_argv_without_secrets() {
    let canary = "OMEN_TEST_SECRET_DO_NOT_LEAK_9f3a_ENV";
    let spec = CommandSpec::new("omen-gremlin")
        .arg("--exit-code")
        .arg("7")
        .env(EnvPolicy::Clear)
        .stdin(StdinSpec::Closed)
        .timeout(Duration::from_millis(200));
    let exact = ReplayDescriptor::try_exact_fixture(
        &spec,
        "exit-code",
        vec![InvariantId::ExitCausePreserved],
        vec!["--exit-code".into(), "7".into()],
    )
    .expect("safe Exact");
    let json = serde_json::to_string(&exact).unwrap();
    assert!(json.contains("\"exact\""));
    assert!(json.contains("--exit-code"));
    assert!(!json.contains(canary));
    assert!(!json.contains("payload"));
    assert!(!json.contains("OPENAI_API_KEY"));

    // Evidence projection of hostile env remains redacted and Exact-free.
    let hostile = CommandSpec::new("omen-gremlin")
        .arg("--exit-code")
        .arg("7")
        .env(EnvPolicy::InheritWith(vec![(
            "OPENAI_API_KEY".into(),
            canary.into(),
        )]))
        .stdin(StdinSpec::Closed)
        .timeout(Duration::from_millis(200));
    let projected = ReplayDescriptor::redacted_from_evidence(
        CommandEvidence::from_execution(&hostile)
            .with_safe_argv(vec!["completely".into(), "different".into()]),
        "exit-code",
        vec![],
    );
    assert_eq!(projected.fidelity(), ReplayFidelity::Redacted);
    assert!(projected.argv().is_empty());
    let json = serde_json::to_string(&projected).unwrap();
    assert!(!json.contains(canary));
    assert!(!json.contains("completely"));
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
fn stream_summary_has_no_raw_preview() {
    let summary = omen_compat::StreamSummary::from_captured(10, 10, false, true, false);
    let json = serde_json::to_string(&summary).unwrap();
    assert!(!json.contains("preview"));
    assert!(json.contains("eof_observed"));
    assert!(json.contains("drain_bounded_out"));
}

#[test]
fn bounded_wait_formula_and_elapsed_check() {
    assert_eq!(declared_max_wall_ms(400), 400 + 500 + 300 + 75);

    let mut outcome = RunOutcome {
        observations: vec![],
        exit_cause: Some(ExitCause::code(0)),
        stdout: Default::default(),
        stderr: Default::default(),
        timed_out: false,
        cleanup: CleanupOutcome::default(),
        elapsed_ms: 400,
        deadline_ms: 400,
        declared_max_wall_ms: declared_max_wall_ms(400),
        bounds: OutputBounds::default(),
        harness_failed: false,
        pid: Some(1),
    };
    assert_eq!(
        outcome.check_bounded_wait_no_hang().outcome,
        InvariantOutcome::Pass
    );
    outcome.elapsed_ms = outcome.declared_max_wall_ms + 1;
    assert_eq!(
        outcome.check_bounded_wait_no_hang().outcome,
        InvariantOutcome::Fail
    );
}

#[test]
fn structured_failure_intentional_mismatch_is_replayable() {
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
        declared_max_wall_ms: declared_max_wall_ms(1000),
        bounds: OutputBounds::default(),
        harness_failed: false,
        pid: Some(4242),
    };

    let result = outcome.check_exit_cause_preserved(1);
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
    .command_evidence(&spec)
    .exit_cause(ExitCause::code(7))
    .stdout(RunOutcome::capture_summary(&outcome.stdout))
    .stderr(RunOutcome::capture_summary(&outcome.stderr))
    .observations(outcome.observations.clone())
    .result(result.clone())
    .replay(ReplayDescriptor::redacted(
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
    assert_eq!(failure.replay.as_ref().unwrap().fixture_mode(), "exit-code");
    assert!(!failure.observations.is_empty());

    // Generic round-trip uses Redacted replay (Exact cannot re-enter trusted type).
    let json = serde_json::to_string(&failure).unwrap();
    let back: StructuredFailure = serde_json::from_str(&json).unwrap();
    assert_eq!(back.invariant, failure.invariant);
    assert_eq!(back.reason, failure.reason);
    assert_eq!(
        back.replay.as_ref().unwrap().fidelity(),
        ReplayFidelity::Redacted
    );
    assert_eq!(result.outcome, InvariantOutcome::Fail);
}
