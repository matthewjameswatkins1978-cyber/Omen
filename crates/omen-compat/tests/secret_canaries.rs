//! Secret canary tests — fake secrets must never appear in durable evidence
//! or Debug output.

use omen_compat::{
    CommandEvidence, CommandSpec, EnvPolicy, EvidenceGrade, FailureKind, InvariantId, Observation,
    Platform, ReplayDescriptor, ReplayFidelity, StdinSpec, StreamSummary, StructuredFailure,
};
use std::time::Duration;

const ENV_CANARY: &str = "OMEN_TEST_SECRET_DO_NOT_LEAK_9f3a_ENV";
const STDIN_CANARY: &str = "OMEN_TEST_SECRET_DO_NOT_LEAK_9f3a_STDIN";
const ARGV_CANARY: &str = "OMEN_TEST_SECRET_DO_NOT_LEAK_9f3a_ARGV";
const OUTPUT_CANARY: &str = "OMEN_TEST_SECRET_DO_NOT_LEAK_9f3a_OUT";

fn canary_spec() -> CommandSpec {
    CommandSpec::new("omen-gremlin")
        .arg("--print-env")
        .arg(ARGV_CANARY)
        .env(EnvPolicy::InheritWith(vec![(
            "OPENAI_API_KEY".into(),
            ENV_CANARY.into(),
        )]))
        .stdin(StdinSpec::Bytes(STDIN_CANARY.as_bytes().to_vec()))
        .timeout(Duration::from_millis(500))
}

fn assert_clean(label: &str, text: &str) {
    for canary in [ENV_CANARY, STDIN_CANARY, ARGV_CANARY, OUTPUT_CANARY] {
        assert!(
            !text.contains(canary),
            "{label} leaked canary {canary}: {text}"
        );
    }
}

#[test]
fn environment_secret_canary_absent_from_evidence_and_debug() {
    let spec = canary_spec();
    let evidence = CommandEvidence::from_execution(&spec);
    assert_clean(
        "CommandEvidence json",
        &serde_json::to_string(&evidence).unwrap(),
    );
    assert_clean("CommandSpec debug", &format!("{spec:?}"));
    assert_clean("EnvPolicy debug", &format!("{:?}", spec.env));
    assert_clean("StdinSpec debug", &format!("{:?}", spec.stdin));
}

#[test]
fn stdin_secret_canary_absent_from_evidence_and_replay() {
    let spec = canary_spec();
    assert_clean(
        "CommandEvidence json",
        &serde_json::to_string(&CommandEvidence::from_execution(&spec)).unwrap(),
    );
    let redacted = ReplayDescriptor::redacted(&spec, "mode", vec![]);
    assert_eq!(redacted.fidelity, ReplayFidelity::Redacted);
    assert!(redacted.argv.is_empty());
    assert_clean(
        "ReplayDescriptor redacted",
        &serde_json::to_string(&redacted).unwrap(),
    );
}

#[test]
fn argv_secret_canary_absent_unless_explicitly_safe() {
    let spec = canary_spec();
    let evidence = CommandEvidence::from_execution(&spec);
    assert!(evidence.argv_safe.is_none());
    assert_clean(
        "CommandEvidence default",
        &serde_json::to_string(&evidence).unwrap(),
    );

    // Exact fixture replay only when caller explicitly marks argv safe.
    // Here argv still contains canary, so we must NOT mark it safe.
    let redacted = ReplayDescriptor::redacted(&spec, "mode", vec![]);
    assert_clean(
        "redacted replay",
        &serde_json::to_string(&redacted).unwrap(),
    );
}

#[test]
fn output_preview_canary_absent_from_stream_summary() {
    // Durable summaries never retain textual output.
    let summary = StreamSummary::from_captured(64, 64, false, true, false);
    let json = serde_json::to_string(&summary).unwrap();
    assert!(!json.contains("preview"));
    assert_clean("StreamSummary", &json);
}

#[test]
fn structured_failure_with_canaries_stays_clean() {
    let spec = canary_spec();
    let failure = StructuredFailure::builder(
        InvariantId::BoundedWaitNoHang,
        Platform::current(),
        "secret_canary",
        FailureKind::HarnessError,
        EvidenceGrade::Strong,
        "synthetic failure with hostile execution inputs",
    )
    .command_evidence(&spec)
    .observations(vec![Observation::EnvApplied {
        key: "OPENAI_API_KEY".into(),
    }])
    .replay(ReplayDescriptor::redacted(
        &spec,
        "secret_canary",
        vec![InvariantId::BoundedWaitNoHang],
    ))
    .stdout(StreamSummary::from_captured(0, 0, false, false, false))
    .build();

    let json = serde_json::to_string(&failure).unwrap();
    assert_clean("StructuredFailure json", &json);
    assert_clean("StructuredFailure debug", &format!("{failure:?}"));
}

#[test]
fn env_observation_does_not_retain_values() {
    let obs = Observation::EnvApplied {
        key: "OPENAI_API_KEY".into(),
    };
    let json = serde_json::to_string(&obs).unwrap();
    assert_clean("EnvApplied", &json);
    assert!(!json.contains("value"));
}
