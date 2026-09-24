//! Shared H2 matrix harness: scripted fake Gate, fixed intent, sentinels.
//!
//! The fake speaks `GateTransport` with deterministic scripts. The
//! admission seam cannot distinguish it from a real Gate process —
//! which is exactly what makes the zero-spawn matrix conclusive.
//! Real-process supervision (crash, malformed, timeout, secrets) lives
//! in `gate_process_tests.rs` with a scripted stub executable.

use omen_authority::{AuthorityIntent, GateTransport, RoundtripError, canonical_digest};
use serde_json::{Value, json};
use std::collections::HashMap;

pub const CAPABILITY: &str = "fixture.ping";
pub const CAPABILITY_VERSION: u32 = 1;
pub const MANIFEST_DIGEST: &str =
    "sha256:eb61b62bde489e00a4d15c37c83e6cdb1e9e378b8f13b910d4b68bd6d68c19da";
pub const PROVIDER: &str = "tethers-stdio-fixture";

pub fn test_intent(evaluation_id: &str, dir: &std::path::Path) -> AuthorityIntent {
    let args = json!({"message": "LK-39", "path": "projects/r2-stdio"});
    AuthorityIntent {
        tether_id: "r2-complete".to_string(),
        tether_version: "1".to_string(),
        evaluation_id: evaluation_id.to_string(),
        action_id: "action_1".to_string(),
        event_id: "evt_r2_001".to_string(),
        event_name: "coding.task_completed".to_string(),
        event_data: json!({"project": "lantern-keeper", "task": "LK-39", "path": "projects/r2-stdio"}),
        facts: json!({"project.type": "software", "task.changed_files": 3}),
        expected_arguments: args,
        expected_capability: CAPABILITY.to_string(),
        expected_capability_version: CAPABILITY_VERSION,
        expected_manifest_digest: MANIFEST_DIGEST.to_string(),
        expected_provider: PROVIDER.to_string(),
        argv: vec!["fixture-exe".to_string(), "--write-marker".to_string()],
        cwd: dir.to_path_buf(),
        timeout_ms: 10_000,
        success_result: json!({"echo": "marker-ok"}),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareScript {
    Allow,
    Ask,
    Deny,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitScript {
    Admit,
    /// Mutate one dispatch field before returning.
    AdmitMutated(DispatchMutation),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchMutation {
    PreparedId,
    ArgumentDigest,
    Capability,
    CapabilityVersion,
    ManifestDigest,
    Provider,
    OwnershipFlag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomeScript {
    Record,
    RefuseConflict,
    /// First delivery attempt times out (response lost); retry records.
    DropFirstThenRecord,
    /// Every delivery attempt times out.
    LoseAll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusScript {
    Clean,
    RecoveryRequired,
}

/// Revocation modelling: real Gates re-read policy at COMMIT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeMode {
    None,
    /// Revoked between PREPARE and COMMIT: prepare allows, commit denies.
    CommitOnly,
    /// Revoked before PREPARE: fresh evaluation denies.
    PrepareAndCommit,
}

pub struct FakeGate {
    pub prepare: PrepareScript,
    pub commit: CommitScript,
    pub outcome: OutcomeScript,
    pub status: StatusScript,
    pub revoke: RevokeMode,
    /// Sleep before answering `commit` (timeout simulation).
    pub commit_sleep_ms: u64,
    pub prepare_calls: u64,
    pub approval_calls: u64,
    pub commit_calls: u64,
    pub outcome_calls: u64,
    pub status_calls: u64,
    pub request_log: Vec<String>,
    intent_args: HashMap<String, Value>,
    /// Session truth: prepared_id -> (evaluation_id, action_id).
    prepared: HashMap<String, (String, String)>,
}

impl FakeGate {
    pub fn new(prepare: PrepareScript, commit: CommitScript) -> Self {
        Self {
            prepare,
            commit,
            outcome: OutcomeScript::Record,
            status: StatusScript::Clean,
            revoke: RevokeMode::None,
            commit_sleep_ms: 0,
            prepare_calls: 0,
            approval_calls: 0,
            commit_calls: 0,
            outcome_calls: 0,
            status_calls: 0,
            request_log: Vec::new(),
            intent_args: HashMap::new(),
            prepared: HashMap::new(),
        }
    }

    fn dispatch_for(
        &self,
        prepared_id: &str,
        approval_consumed: bool,
        intent: &AuthorityIntent,
    ) -> Value {
        let digest = canonical_digest(&intent.expected_arguments).unwrap();
        let mut d = json!({
            "schema": "tethers.dispatch/1",
            "execution_id": format!("exec_{prepared_id}"),
            "evaluation_id": intent.evaluation_id,
            "action_id": intent.action_id,
            "event_id": intent.event_id,
            "capability": {"name": CAPABILITY, "version": CAPABILITY_VERSION},
            "argument_digest": digest,
            "manifest_digest": MANIFEST_DIGEST,
            "provider_identity": PROVIDER,
            "intent": {"trail": "recorded", "replay": "armed"},
            "authority_protocol": "tethers.authority/1",
            "prepared_id": prepared_id,
            "approval_consumed": approval_consumed,
            "authorizes_physical_execution_by_tethers": false,
            "host_must_report_outcome": true,
            "provider_invocations": 0
        });
        if let CommitScript::AdmitMutated(m) = &self.commit {
            match m {
                DispatchMutation::PreparedId => d["prepared_id"] = json!("prep_forged"),
                DispatchMutation::ArgumentDigest => {
                    d["argument_digest"] = json!(
                        "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    )
                }
                DispatchMutation::Capability => d["capability"]["name"] = json!("evil.other"),
                DispatchMutation::CapabilityVersion => d["capability"]["version"] = json!(99),
                DispatchMutation::ManifestDigest => {
                    d["manifest_digest"] = json!(
                        "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                    )
                }
                DispatchMutation::Provider => d["provider_identity"] = json!("evil-provider"),
                DispatchMutation::OwnershipFlag => {
                    d["authorizes_physical_execution_by_tethers"] = json!(true)
                }
            }
        }
        d
    }
}

impl GateTransport for FakeGate {
    fn label(&self) -> String {
        "fake:matrix".to_string()
    }

    fn roundtrip(&mut self, operation: &str, payload: Value) -> Result<Value, RoundtripError> {
        self.request_log.push(operation.to_string());
        match operation {
            "status" => {
                self.status_calls += 1;
                Ok(match self.status {
                    StatusScript::Clean => json!({
                        "protocol": "tethers.authority/1",
                        "product_version": "0.8.0",
                        "gate_instance_id": "gate_fake",
                        "healthy": true,
                        "durable_reconciliation": {"state": "healthy"},
                        "shutdown_requested": false,
                        "provider_invocations": 0,
                        "pending_approvals": [],
                        "unresolved_commits": [],
                        "prepared": [],
                        "terminal_outcomes": [],
                        "recovery_required": [],
                        "truncated": false,
                        "prepared_count": 0,
                        "committed_count": 0
                    }),
                    StatusScript::RecoveryRequired => json!({
                        "protocol": "tethers.authority/1",
                        "product_version": "0.8.0",
                        "gate_instance_id": "gate_fake",
                        "healthy": true,
                        "durable_reconciliation": {"state": "recovery_required"},
                        "shutdown_requested": false,
                        "provider_invocations": 0,
                        "pending_approvals": [],
                        "unresolved_commits": [],
                        "prepared": [],
                        "terminal_outcomes": [],
                        "recovery_required": [{"execution_id": "exec_old", "state": "terminal_replay_incomplete"}],
                        "truncated": false,
                        "prepared_count": 0,
                        "committed_count": 0
                    }),
                })
            }
            "prepare" => {
                self.prepare_calls += 1;
                let action_id = payload
                    .get("action_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("action_1")
                    .to_string();
                let evaluation_id = payload
                    .get("evaluation_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("eval_1")
                    .to_string();
                // Learn the intent's arguments for digest-faithful dispatch.
                // (The fake stands in for Core planning deterministically.)
                let prepared_id = format!("prep_{}_{}", self.prepare_calls, action_id);
                // Session truth: this preparation is ABOUT this evaluation.
                self.prepared.insert(
                    prepared_id.clone(),
                    (evaluation_id.clone(), action_id.clone()),
                );
                if self.revoke == RevokeMode::PrepareAndCommit {
                    return Ok(json!({
                        "decision": "deny", "reason": "host_policy_deny",
                        "authorizes_dispatch": false, "core_planned": true,
                        "evaluation_id": evaluation_id, "action_id": action_id,
                        "tether_id": "r2-complete", "tether_version": "1",
                        "provider_invocations": 0,
                        "execution": {"performed": false, "provider_invocations": 0, "replay_mutated": false}
                    }));
                }
                let base = || {
                    json!({
                        "prepared_id": prepared_id,
                        "authorizes_dispatch": false, "core_planned": true,
                        "evaluation_id": evaluation_id, "action_id": action_id,
                        "tether_id": "r2-complete", "tether_version": "1",
                        "provider_invocations": 0,
                        "execution": {"performed": false, "provider_invocations": 0, "replay_mutated": false}
                    })
                };
                match self.prepare {
                    PrepareScript::Allow => {
                        let mut v = base();
                        v["decision"] = json!("allow_prepared");
                        v["reason"] = json!("current_policy_allow");
                        Ok(v)
                    }
                    PrepareScript::Ask => {
                        let mut v = base();
                        v["decision"] = json!("ask");
                        v["reason"] = json!("host_policy_ask");
                        // Digest-faithful approval: recompute from the
                        // intent args the test registered (see note below).
                        let digest = self
                            .intent_args
                            .get(&evaluation_id)
                            .map(|a| canonical_digest(a).unwrap())
                            .unwrap_or_else(|| "sha256:missing".to_string());
                        v["approval"] = json!({
                            "approval_id": "approval-1",
                            "action_id": action_id,
                            "capability": {"name": CAPABILITY, "version": CAPABILITY_VERSION},
                            "reason": "host_policy_ask",
                            "argument_digest": digest,
                            "effect_summary": "test approval",
                            "state": "pending"
                        });
                        Ok(v)
                    }
                    PrepareScript::Deny => {
                        let mut v = base();
                        v.as_object_mut().unwrap().remove("prepared_id");
                        v["decision"] = json!("deny");
                        v["reason"] = json!("host_policy_deny");
                        Ok(v)
                    }
                    PrepareScript::Unavailable => {
                        let mut v = base();
                        v.as_object_mut().unwrap().remove("prepared_id");
                        v["decision"] = json!("unavailable");
                        v["reason"] = json!("unsupported_policy_configuration");
                        Ok(v)
                    }
                }
            }
            "approval_decision" => {
                self.approval_calls += 1;
                let decision = payload
                    .get("decision")
                    .and_then(|v| v.as_str())
                    .unwrap_or("deny");
                let state = match decision {
                    "approve" => "approved",
                    "deny" => "denied",
                    _ => "cancelled",
                };
                Ok(json!({
                    "approval_id": payload.get("approval_id").cloned().unwrap_or(json!("approval-1")),
                    "state": state,
                    "action_id": "action_1",
                    "capability": {"name": CAPABILITY, "version": CAPABILITY_VERSION},
                    "authorizes_dispatch": false,
                    "provider_invocations": 0
                }))
            }
            "commit" => {
                self.commit_calls += 1;
                if self.commit_sleep_ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(self.commit_sleep_ms));
                }
                if self.revoke != RevokeMode::None {
                    return Err(RoundtripError::Refused {
                        code: "commit.deny".to_string(),
                        message: "fresh current authority denies (revoked)".to_string(),
                        data: None,
                    });
                }
                let prepared_id = payload
                    .get("prepared_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("prep_unknown")
                    .to_string();
                let approval_consumed = payload.get("approval_id").is_some();
                // Session binding: unknown preparations refuse (replay
                // hygiene mirrors the real Gate).
                let Some((evaluation_id, action_id)) = self.prepared.get(&prepared_id).cloned()
                else {
                    return Err(RoundtripError::Refused {
                        code: "commit.unknown_prepared".to_string(),
                        message: "prepared_id not known to this session".to_string(),
                        data: None,
                    });
                };
                // Reconstruct the intent carrying the registered args.
                let args = self
                    .intent_args
                    .get(&evaluation_id)
                    .cloned()
                    .unwrap_or(json!({}));
                let mut stub = stub_intent(&prepared_id, args);
                stub.evaluation_id = evaluation_id;
                stub.action_id = action_id;
                Ok(self.dispatch_for(&prepared_id, approval_consumed, &stub))
            }
            "outcome" => {
                self.outcome_calls += 1;
                match self.outcome {
                    OutcomeScript::Record => {
                        let exec = payload
                            .get("execution_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("exec_unknown");
                        Ok(json!({
                            "execution_id": exec,
                            "status": payload.get("classification").cloned().unwrap_or(json!("succeeded")),
                            "attempted": true,
                            "trail_outcome_recorded": true,
                            "idempotent": false,
                            "replay_terminal": "recorded",
                            "provider_invocations": 0
                        }))
                    }
                    OutcomeScript::RefuseConflict => Err(RoundtripError::Refused {
                        code: "outcome.conflict".to_string(),
                        message: "different classification already recorded".to_string(),
                        data: None,
                    }),
                    OutcomeScript::DropFirstThenRecord => {
                        if self.outcome_calls == 1 {
                            Err(RoundtripError::Transport(
                                omen_authority::AuthorityError::Receive(
                                    "fake.response.lost".to_string(),
                                ),
                            ))
                        } else {
                            let exec = payload
                                .get("execution_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("exec_unknown");
                            Ok(json!({
                                "execution_id": exec,
                                "status": payload.get("classification").cloned().unwrap_or(json!("succeeded")),
                                "attempted": true,
                                "trail_outcome_recorded": true,
                                "idempotent": true,
                                "replay_terminal": "recorded",
                                "provider_invocations": 0
                            }))
                        }
                    }
                    OutcomeScript::LoseAll => Err(RoundtripError::Transport(
                        omen_authority::AuthorityError::Receive("fake.response.lost".to_string()),
                    )),
                }
            }
            other => Err(RoundtripError::Refused {
                code: "frame.unknown_operation".to_string(),
                message: format!("fake gate has no script for {other}"),
                data: None,
            }),
        }
    }

    fn shutdown(&mut self) {}
}

/// Register the intent's expected args so ask/commit digests are
/// faithful (stands in for deterministic Core planning).
pub fn register_intent(gate: &mut FakeGate, intent: &AuthorityIntent) {
    gate.intent_args.insert(
        intent.evaluation_id.clone(),
        intent.expected_arguments.clone(),
    );
}

fn stub_intent(prepared_id: &str, args: Value) -> AuthorityIntent {
    // Session truth supplies evaluation/action at commit time (see the
    // `prepared` map); the stub only needs the registered arguments.
    let _ = prepared_id;
    AuthorityIntent {
        tether_id: "r2-complete".to_string(),
        tether_version: "1".to_string(),
        evaluation_id: String::new(),
        action_id: "action_1".to_string(),
        event_id: "evt_r2_001".to_string(),
        event_name: "coding.task_completed".to_string(),
        event_data: Value::Null,
        facts: Value::Null,
        expected_arguments: args,
        expected_capability: CAPABILITY.to_string(),
        expected_capability_version: CAPABILITY_VERSION,
        expected_manifest_digest: MANIFEST_DIGEST.to_string(),
        expected_provider: PROVIDER.to_string(),
        argv: vec![],
        cwd: std::path::PathBuf::from("."),
        timeout_ms: 10_000,
        success_result: json!({"echo": "x"}),
    }
}
