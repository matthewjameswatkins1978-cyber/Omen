//! Exact physical execution binding proofs (H2 repair acceptance).
//!
//! Threat: Tethers authorises semantic action A while Omen executes
//! physical command B. These tests prove AUTHORISED A EXECUTES A:
//!
//! - the trusted resolver derives argv/cwd/exe-identity from the
//!   authorised semantics + trusted provision (never caller argv);
//! - `VerifiedExecutionBinding::bind` refuses ANY mismatch with zero
//!   spawn (the executor is never reached);
//! - `PhysicalExecutor::execute` accepts ONLY the verified binding, so
//!   post-COMMIT substitution is impossible by type;
//! - `SupervisorExecutor` re-verifies TARGET executable identity
//!   immediately before spawn (bind -> spawn window).
//!
//! Spawn truth: `CountingExecutor` records calls + exact argv/cwd and
//! writes the sentinel marker per execution. Marker absent + zero calls
//! == zero spawn; `last_argv`/`last_cwd` == the verified binding proves
//! the executed command IS the authorised one.

mod support;

use omen_authority::{
    AdmitExecute, AskPolicy, CountingExecutor, GateTransport, HumanOutcome, OutcomeJournal,
    SupervisorExecutor, dispatch::DispatchContext, resolve_execution, run::run_once,
    verify_dispatch,
};
use serde_json::json;
use support::*;

fn allow_harness(
    eval: &str,
) -> (
    tempfile::TempDir,
    omen_authority::AuthorityIntent,
    FakeGate,
    omen_authority::FixtureProvision,
    std::path::PathBuf,
    OutcomeJournal,
) {
    let dir = tempfile::tempdir().unwrap();
    let intent = test_intent(eval, dir.path());
    let mut gate = FakeGate::new(PrepareScript::Allow, CommitScript::Admit);
    register_intent(&mut gate, &intent);
    let provision = test_provision(dir.path());
    let marker = dir.path().join("spawn.marker");
    let journal = OutcomeJournal::open(dir.path().join("j.jsonl"));
    (dir, intent, gate, provision, marker, journal)
}

fn succeeding(marker: &std::path::Path) -> CountingExecutor {
    CountingExecutor::succeeding(Some(marker.to_path_buf()))
}

/// Drive PREPARE + COMMIT + dispatch verification at driver level and
/// return the verified dispatch (bind-level tests build on real
/// admission, never synthetic dispatches).
fn admitted_dispatch(
    driver: &mut AdmitExecute<FakeGate>,
    intent: &omen_authority::AuthorityIntent,
) -> omen_authority::VerifiedDispatch {
    let prepared_id = match driver.prepare(intent).expect("prepare works") {
        omen_authority::PrepareOutcome::AllowPrepared { prepared_id, .. } => prepared_id,
        other => panic!("want allow, got {other:?}"),
    };
    let dispatch = match driver.commit(&prepared_id, None).expect("commit works") {
        omen_authority::CommitOutcome::Admitted { dispatch, .. } => dispatch,
        omen_authority::CommitOutcome::Refused { code, message } => {
            panic!("want admit, got refused {code}: {message}")
        }
    };
    let ctx = DispatchContext {
        evaluation_id: intent.evaluation_id.clone(),
        action_id: intent.action_id.clone(),
        prepared_id: prepared_id.clone(),
        expected_capability: intent.expected_capability.clone(),
        expected_capability_version: intent.expected_capability_version,
        expected_manifest_digest: intent.expected_manifest_digest.clone(),
        expected_provider: intent.expected_provider.clone(),
        expected_arguments: intent.expected_arguments.clone(),
    };
    verify_dispatch(&dispatch, &ctx).expect("dispatch verifies")
}

// 1. EXECUTABLE SUBSTITUTION: authorise A, substitute a DIFFERENT
// executable under the bound path before bind. Expected: REFUSED, ZERO
// SPAWN.
#[test]
fn b01_executable_substitution_zero_spawn() {
    let (_dir, intent, gate, provision, marker, _journal) = allow_harness("eval_b01");
    let mut driver = AdmitExecute::new(gate);
    let verified = admitted_dispatch(&mut driver, &intent);
    let binding = resolve_execution(&intent, &provision).expect("resolve works");
    // Attacker substitutes a different executable at the bound path.
    std::fs::write(&provision.exe, b"attacker-controlled-executable").unwrap();
    let err = omen_authority::VerifiedExecutionBinding::bind(&verified, &binding)
        .expect_err("substituted executable must refuse");
    assert!(
        format!("{err:?}").contains("exe_identity_changed"),
        "got: {err:?}"
    );
    assert!(!marker.exists(), "substituted executable spawned");
    driver.transport_mut().shutdown();
}

// 2. ARGV SUBSTITUTION (semantic mutation after PREPARE): the physical
// command derives from the semantic arguments, so mutating arguments
// after PREPARE must break the COMMIT-time digest bind. Expected:
// REFUSED, ZERO SPAWN.
#[tokio::test]
async fn b02_argv_substitution_after_prepare_zero_spawn() {
    let (_dir, mut intent, gate, provision, marker, journal) = allow_harness("eval_b02");
    let mut driver = AdmitExecute::new(gate);
    let prepared_id = match driver.prepare(&intent).expect("prepare works") {
        omen_authority::PrepareOutcome::AllowPrepared { prepared_id, .. } => prepared_id,
        other => panic!("want allow, got {other:?}"),
    };
    // Attacker mutates the semantic arguments after PREPARE (the only
    // channel that could steer the derived physical command).
    intent.expected_arguments = json!({"message": "EVIL-1", "path": "projects/r2-stdio"});
    let dispatch = match driver
        .commit(&prepared_id, None)
        .expect("commit roundtrips")
    {
        omen_authority::CommitOutcome::Admitted { dispatch, .. } => dispatch,
        omen_authority::CommitOutcome::Refused { code, message } => {
            panic!("fake admits, got refused {code}: {message}")
        }
    };
    let ctx = DispatchContext {
        evaluation_id: intent.evaluation_id.clone(),
        action_id: intent.action_id.clone(),
        prepared_id: prepared_id.clone(),
        expected_capability: intent.expected_capability.clone(),
        expected_capability_version: intent.expected_capability_version,
        expected_manifest_digest: intent.expected_manifest_digest.clone(),
        expected_provider: intent.expected_provider.clone(),
        expected_arguments: intent.expected_arguments.clone(),
    };
    let err = verify_dispatch(&dispatch, &ctx).expect_err("mutated args must refuse");
    assert!(
        format!("{err:?}").contains("argument_digest"),
        "got: {err:?}"
    );
    // End-to-end: the mutated intent can never reach execution either
    // (fresh admission recomputes the digest from the MUTATED args, the
    // fake plans the ORIGINAL args — mismatch fails closed).
    let mut exec = succeeding(&marker);
    let out = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        &provision,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await;
    assert!(out.is_err(), "mutated intent must fail closed, got {out:?}");
    assert_eq!(exec.calls, 0);
    assert!(!marker.exists(), "mutated arguments executed");
}

// 3. POST-COMMIT SUBSTITUTION: a binding verified for one authorised
// action cannot cross to another COMMIT, and the verified handle
// exposes no mutation channel (getters are read-only; mutating a clone
// leaves the bound values intact). Expected: REFUSED / impossible by
// type, ZERO SPAWN.
#[test]
fn b03_post_commit_binding_cannot_cross_commits() {
    let (dir, intent, gate, provision, _marker, _journal) = allow_harness("eval_b03a");
    let mut driver = AdmitExecute::new(gate);
    let verified_a = admitted_dispatch(&mut driver, &intent);
    let binding_a = resolve_execution(&intent, &provision).expect("resolve works");
    let bound_a = omen_authority::VerifiedExecutionBinding::bind(&verified_a, &binding_a)
        .expect("bind works");

    // Second authorised action with DIFFERENT semantic arguments.
    let mut intent_b = test_intent("eval_b03b", dir.path());
    intent_b.expected_arguments = json!({"message": "LK-99", "path": "projects/r2-stdio"});
    register_intent(driver.transport_mut(), &intent_b);
    let verified_b = admitted_dispatch(&mut driver, &intent_b);

    // The first binding cannot ride the second COMMIT.
    let err = omen_authority::VerifiedExecutionBinding::bind(&verified_b, &binding_a)
        .expect_err("cross-commit binding must refuse");
    assert!(
        format!("{err:?}").contains("argument_digest"),
        "got: {err:?}"
    );

    // Type-level: the bound handle offers argv/cwd read-only. There is
    // no setter, no public field, and no executor overload accepting
    // caller argv — the only physical path is `execute(&bound)`.
    let snapshot = bound_a.argv().to_vec();
    let mut forged = snapshot.clone();
    forged.push("attacker-flag".to_string());
    assert_eq!(
        bound_a.argv(),
        snapshot.as_slice(),
        "verified binding mutated through its handle"
    );
    assert_eq!(bound_a.argv().len(), 2, "resolver argv shape changed");
    driver.transport_mut().shutdown();
}

// 4. CWD SUBSTITUTION: authorise/bind cwd A, destroy it before bind
// (attempt cwd B has no channel — cwd derives from the provision).
// Expected: REFUSED, ZERO SPAWN.
#[test]
fn b04_cwd_substitution_zero_spawn() {
    let (dir, intent, gate, provision, marker, _journal) = allow_harness("eval_b04");
    let mut driver = AdmitExecute::new(gate);
    let verified = admitted_dispatch(&mut driver, &intent);
    let binding = resolve_execution(&intent, &provision).expect("resolve works");
    assert_eq!(
        binding.cwd(),
        dir.path(),
        "cwd must be the provisioned sandbox"
    );
    // Attacker removes the bound scope between resolve and bind.
    std::fs::remove_dir_all(dir.path()).unwrap();
    let err = omen_authority::VerifiedExecutionBinding::bind(&verified, &binding)
        .expect_err("lost cwd must refuse");
    assert!(
        format!("{err:?}").contains("cwd_unresolvable"),
        "got: {err:?}"
    );
    assert!(
        !marker.exists(),
        "unscoped execution happened: marker exists"
    );
    driver.transport_mut().shutdown();
}

// 5. EXECUTABLE IDENTITY CHANGE (in-place tamper): bind records the
// TARGET identity at resolve; flipping bytes before bind refuses.
// Expected: REFUSED, ZERO SPAWN.
#[test]
fn b05_executable_identity_change_zero_spawn() {
    let (_dir, intent, gate, provision, marker, _journal) = allow_harness("eval_b05");
    let mut driver = AdmitExecute::new(gate);
    let verified = admitted_dispatch(&mut driver, &intent);
    let binding = resolve_execution(&intent, &provision).expect("resolve works");
    // In-place tamper: same path, different bytes.
    let mut bytes = std::fs::read(&provision.exe).unwrap();
    bytes.extend_from_slice(b".tampered");
    std::fs::write(&provision.exe, bytes).unwrap();
    let err = omen_authority::VerifiedExecutionBinding::bind(&verified, &binding)
        .expect_err("tampered executable must refuse");
    assert!(
        format!("{err:?}").contains("exe_identity_changed"),
        "got: {err:?}"
    );
    assert!(!marker.exists(), "tampered executable spawned");
    driver.transport_mut().shutdown();
}

// 6. PROVIDER SUBSTITUTION: authorise provider A, attempt resolution
// from provider B. The registry knows exactly one provider mapping;
// anything else refuses before any dispatch can bind it. Expected:
// REFUSED, ZERO SPAWN.
#[tokio::test]
async fn b06_provider_substitution_zero_spawn() {
    let (_dir, mut intent, gate, provision, marker, journal) = allow_harness("eval_b06");
    intent.expected_provider = "evil-provider".to_string();
    // Resolver: unknown provider refuses with no dispatch involved.
    let err = resolve_execution(&intent, &provision).expect_err("evil provider must refuse");
    assert!(
        format!("{err:?}").contains("unknown_provider"),
        "got: {err:?}"
    );
    // End-to-end: a Gate dispatch for the authorised provider can never
    // verify against the substituted intent either.
    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let out = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        &provision,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await;
    assert!(
        out.is_err(),
        "substituted provider must fail closed, got {out:?}"
    );
    assert_eq!(exec.calls, 0);
    assert!(!marker.exists(), "substituted provider executed");
}

// 6b. Unknown capability refuses at the resolver (semantic capabilities
// are never weakened into generic exec).
#[test]
fn b06b_unknown_capability_zero_spawn() {
    let (_dir, mut intent, _gate, provision, _marker, _journal) = allow_harness("eval_b06b");
    intent.expected_capability = "os.exec".to_string();
    let err = resolve_execution(&intent, &provision).expect_err("generic exec must refuse");
    assert!(
        format!("{err:?}").contains("unknown_capability"),
        "got: {err:?}"
    );
}

// 7. FAITHFUL BINDING: authorise semantic action A; the trusted
// resolver produces physical binding A; COMMIT verifies; EXACTLY ONE
// execution occurs with EXACTLY the resolver argv/cwd; outcome reaches
// the Gate.
#[tokio::test]
async fn b07_faithful_binding_exactly_one_spawn() {
    let (_dir, intent, gate, provision, marker, journal) = allow_harness("eval_b07");
    // Independent resolution: what the trusted mapping says A IS.
    let expected = resolve_execution(&intent, &provision).expect("resolve works");
    assert_eq!(expected.argv().len(), 2);
    assert_eq!(
        std::path::Path::new(&expected.argv()[0]),
        expected.exe(),
        "argv[0] must be the bound executable"
    );
    assert!(
        expected.argv()[1].ends_with("LK-39.marker"),
        "marker must derive from the authorised message, got {:?}",
        expected.argv()[1]
    );

    let mut driver = AdmitExecute::new(gate);
    let mut exec = succeeding(&marker);
    let report = run_once(
        &mut driver,
        Some("gate_fake".to_string()),
        &intent,
        &provision,
        AskPolicy::Defer,
        &mut exec,
        &journal,
    )
    .await
    .expect("faithful path drives cleanly");
    assert!(matches!(report.human, HumanOutcome::Admitted));
    assert_eq!(report.spawn_count, 1);
    assert_eq!(exec.calls, 1, "exactly one physical execution");
    // The executed command IS the verified binding: byte-for-byte.
    assert_eq!(
        exec.last_argv,
        expected.argv(),
        "executed argv != bound argv"
    );
    assert_eq!(
        exec.last_cwd.as_deref(),
        Some(expected.cwd()),
        "executed cwd != bound cwd"
    );
    assert_eq!(
        std::fs::read_to_string(&marker).ok().as_deref(),
        Some("spawn-1\n")
    );
    assert!(journal.is_terminal("exec_prep_1_action_1"));
    assert_eq!(driver.transport().outcome_calls, 1);
    assert!(
        expected.env().is_empty(),
        "fixture env projection must stay empty"
    );
}

const MARKER_RS: &str = r#"
fn main() {
    let marker = std::env::args().nth(1).expect("marker path");
    std::fs::write(&marker, std::process::id().to_string()).expect("marker");
    println!("marker-ok");
}
"#;

fn real_fixture(dir: &std::path::Path) -> std::path::PathBuf {
    let src = dir.join("bind_fixture.rs");
    std::fs::write(&src, MARKER_RS).unwrap();
    let exe = dir.join("bind-fixture.exe");
    let out = std::process::Command::new("rustc")
        .arg("--edition=2021")
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("rustc builds the binding fixture");
    assert!(out.status.success(), "fixture build failed");
    exe
}

// 8. REAL SUPERVISOR FAITHFUL: the verified binding spawns the exact
// bound command through production supervision (marker proves it).
#[tokio::test]
async fn b08_real_supervisor_spawns_exact_bound_command() {
    let (dir, intent, gate, _prov, _marker, _journal) = allow_harness("eval_b08");
    let exe = real_fixture(dir.path());
    let provision = omen_authority::FixtureProvision {
        exe,
        workdir: dir.path().to_path_buf(),
    };
    let mut driver = AdmitExecute::new(gate);
    let verified = admitted_dispatch(&mut driver, &intent);
    let binding = resolve_execution(&intent, &provision).expect("resolve works");
    let bound =
        omen_authority::VerifiedExecutionBinding::bind(&verified, &binding).expect("bind works");
    let mut exec = SupervisorExecutor::new();
    let attempt = omen_authority::PhysicalExecutor::execute(&mut exec, &bound)
        .await
        .expect("faithful execution runs");
    assert!(attempt.attempted);
    assert!(attempt.exit.is_zero(), "fixture failed: {attempt:?}");
    let marker_path = binding.marker_path().to_path_buf();
    assert!(marker_path.is_file(), "bound marker was not written");
    assert_eq!(String::from_utf8_lossy(&attempt.stdout).trim(), "marker-ok");
    driver.transport_mut().shutdown();
}

// 9. PRE-SPAWN IDENTITY (bind -> spawn window): swapping the TARGET
// binary after bind refuses inside production supervision with ZERO
// SPAWN — even though COMMIT, resolve, and bind all succeeded.
#[tokio::test]
async fn b09_pre_spawn_identity_change_zero_spawn() {
    let (dir, intent, gate, _prov, _marker, _journal) = allow_harness("eval_b09");
    let exe = real_fixture(dir.path());
    let provision = omen_authority::FixtureProvision {
        exe,
        workdir: dir.path().to_path_buf(),
    };
    let mut driver = AdmitExecute::new(gate);
    let verified = admitted_dispatch(&mut driver, &intent);
    let binding = resolve_execution(&intent, &provision).expect("resolve works");
    let bound =
        omen_authority::VerifiedExecutionBinding::bind(&verified, &binding).expect("bind works");
    let marker_path = binding.marker_path().to_path_buf();
    // Attacker swaps the TARGET binary after bind, before spawn.
    std::fs::write(&provision.exe, b"not-an-executable-anymore").unwrap();
    let mut exec = SupervisorExecutor::new();
    let err = omen_authority::PhysicalExecutor::execute(&mut exec, &bound)
        .await
        .expect_err("swapped TARGET must refuse before spawn");
    assert!(
        format!("{err:?}").contains("exe_identity_changed"),
        "got: {err:?}"
    );
    assert!(
        !marker_path.exists(),
        "swapped TARGET executed: marker exists"
    );
    driver.transport_mut().shutdown();
}
