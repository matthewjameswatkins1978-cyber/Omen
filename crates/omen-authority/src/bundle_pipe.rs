//! Authorised byte-pipeline runner over `tethers.authority/2` bundles.
//!
//! Division of responsibility (absolute, per the canonical architecture):
//!
//! - **Omen owns** physical pipeline composition: stage executables,
//!   argv, cwd, OS pipes, redirect targets, and the raw stdin/stdout/
//!   stderr bytes. The composition identity below is computed over exactly
//!   that material — Tethers treats it as an opaque handle.
//! - **Tethers owns** authority/replay/outcome truth: sequential PREPARE
//!   per stage, one atomic `commit_bundle`, and terminal outcome records.
//!
//! Law: every required Action is PREPAREd before physical execution; the
//! bundle COMMIT succeeds before the first spawn; any failure before the
//! committed dispatch yields ZERO process spawns. `together` is never used
//! for pipelines and there is no `process.pipeline` capability — members
//! are ordinary single Actions bound by one Omen-owned composition.
//!
//! The runner is split in two so tests (and operators) can hold the
//! authorised bundle across the physical boundary: [`authorise_pipe`]
//! performs steps 1–5 with zero spawn; [`spawn_authorised`] re-verifies
//! live material and spawns. Composition drift between the two fails
//! closed with zero new spawn.

use crate::AuthorityError;
use crate::admission::{AdmitExecute, AuthorityIntent, BundleCommitOutcome, PrepareOutcome};
use crate::protocol::{AUTHORITY_PROTOCOL_V2, BUNDLE_DISPATCH_SCHEMA, BundleDispatchRecord};
use crate::transport::GateTransport;
use omen_engine::{PipelineExecutionOutput, ProcessSupervisor};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One physical pipe stage Omen will spawn after COMMIT.
#[derive(Debug, Clone)]
pub struct PipeStage {
    /// Absolute path to the stage executable.
    pub exe: PathBuf,
    /// Full argv (argv[0] is the executable path as spawned).
    pub argv: Vec<String>,
    /// Working directory for the spawn.
    pub cwd: PathBuf,
    /// Semantic intent PREPAREd for this stage (arguments carry the
    /// composition identity; see [`compose_identity`]).
    pub intent: AuthorityIntent,
}

/// Optional stdout redirect for the LAST stage. The target path is part
/// of the authorised composition: redirect authority is included before
/// execution, and a drifted target fails closed with zero spawn.
#[derive(Debug, Clone)]
pub struct PipeRedirect {
    pub path: PathBuf,
}

/// Omen-owned composition material: the exact physical truth the bundle
/// authorises.
#[derive(Debug, Clone)]
pub struct PipeComposition {
    pub stages: Vec<StageMaterial>,
    pub redirect: Option<PathBuf>,
}

/// Per-stage physical material bound into the composition.
#[derive(Debug, Clone)]
pub struct StageMaterial {
    /// SHA256 of the stage executable bytes at authorise time.
    pub exe_sha256: String,
    /// Exact argv as spawned.
    pub argv: Vec<String>,
    /// Working directory as spelled.
    pub cwd: PathBuf,
}

fn sha256_file(path: &Path) -> Result<String, AuthorityError> {
    let bytes = std::fs::read(path).map_err(|e| {
        AuthorityError::Validate(format!(
            "composition.exe.unreadable: {}: {e}",
            path.display()
        ))
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Compute the composition over live physical material. Every byte that
/// influences what runs (executable bytes, argv, cwd, redirect target)
/// is inside the digest; anything outside it is unauthorised by
/// construction. Drift after authority (changed exe/argv/cwd/redirect)
/// yields a different digest and fails closed before first spawn.
pub fn compose_identity(
    stages: &[PipeStage],
    redirect: Option<&PipeRedirect>,
) -> Result<(PipeComposition, String), AuthorityError> {
    if stages.len() < 2 {
        return Err(AuthorityError::Validate(
            "composition.member_count: a pipe has at least two stages".to_string(),
        ));
    }
    let mut material = Vec::with_capacity(stages.len());
    for (index, stage) in stages.iter().enumerate() {
        if stage.argv.is_empty() {
            return Err(AuthorityError::Validate(format!(
                "composition.stage_{index}_empty_argv"
            )));
        }
        material.push(StageMaterial {
            exe_sha256: sha256_file(&stage.exe)?,
            argv: stage.argv.clone(),
            cwd: stage.cwd.clone(),
        });
    }
    let composition = PipeComposition {
        stages: material,
        redirect: redirect.map(|r| r.path.clone()),
    };
    let canonical = serde_json::json!({
        "format": "omen-pipe-v1",
        "stages": composition.stages.iter().map(|s| serde_json::json!({
            "exe_sha256": s.exe_sha256,
            "argv": s.argv,
            "cwd": s.cwd.to_string_lossy(),
        })).collect::<Vec<_>>(),
        "redirect": composition.redirect.as_ref().map(|p| p.to_string_lossy().to_string()),
    });
    let bytes = serde_json_canonicalizer::to_vec(&canonical)
        .map_err(|e| AuthorityError::Validate(format!("composition.encode.failed: {e}")))?;
    Ok((composition, format!("{:x}", Sha256::digest(bytes))))
}

/// A bundle dispatch that survived every Omen-side binding check.
#[derive(Debug, Clone)]
pub struct VerifiedBundle {
    pub bundle_id: String,
    pub composition_digest: String,
    pub members: Vec<VerifiedBundleMember>,
    pub raw: BundleDispatchRecord,
}

/// One verified bundle member: Gate-admitted execution identity bound to
/// Omen's stage order.
#[derive(Debug, Clone)]
pub struct VerifiedBundleMember {
    pub stage_index: usize,
    pub execution_id: String,
    pub action_id: String,
    pub prepared_id: String,
}

/// Verify a `commit_bundle` dispatch against Omen's own truth: schema,
/// /2 authority protocol, member count/order/identities, per-member
/// capability/version/manifest/provider/argument digests recomputed from
/// Omen's intents, the echoed composition identity, and the ownership
/// flags (Tethers never executes; Omen must report outcomes). Any
/// mismatch is zero spawn.
pub fn verify_bundle_dispatch(
    dispatch: &BundleDispatchRecord,
    stages: &[PipeStage],
    composition_digest: &str,
) -> Result<VerifiedBundle, AuthorityError> {
    let mismatch = |field: &str, got: &str, want: &str| {
        AuthorityError::Validate(format!("bundle.dispatch.{field}_mismatch: {got} != {want}"))
    };
    if dispatch.raw["schema"] != serde_json::Value::String(BUNDLE_DISPATCH_SCHEMA.to_string()) {
        // (Schema already enforced by parse; belt and braces on the raw.)
        return Err(mismatch(
            "schema",
            dispatch.raw["schema"].as_str().unwrap_or("?"),
            BUNDLE_DISPATCH_SCHEMA,
        ));
    }
    if dispatch.authority_protocol != AUTHORITY_PROTOCOL_V2 {
        return Err(mismatch(
            "authority_protocol",
            &dispatch.authority_protocol,
            AUTHORITY_PROTOCOL_V2,
        ));
    }
    if dispatch.composition_digest != composition_digest {
        return Err(mismatch(
            "composition_digest",
            &dispatch.composition_digest,
            composition_digest,
        ));
    }
    if dispatch.members.len() != stages.len() {
        return Err(AuthorityError::Validate(format!(
            "bundle.dispatch.member_count_mismatch: {} != {}",
            dispatch.members.len(),
            stages.len()
        )));
    }
    if dispatch.bundle_id.is_empty() {
        return Err(AuthorityError::Validate(
            "bundle.dispatch.missing_bundle_id".to_string(),
        ));
    }
    let mut members = Vec::with_capacity(stages.len());
    for (index, (member, stage)) in dispatch.members.iter().zip(stages.iter()).enumerate() {
        let ctx = |f: &str| format!("member[{index}].{f}");
        if member.evaluation_id != stage.intent.evaluation_id {
            return Err(mismatch(
                &ctx("evaluation_id"),
                &member.evaluation_id,
                &stage.intent.evaluation_id,
            ));
        }
        if member.action_id != stage.intent.action_id {
            return Err(mismatch(
                &ctx("action_id"),
                &member.action_id,
                &stage.intent.action_id,
            ));
        }
        if member.capability.name != stage.intent.expected_capability {
            return Err(mismatch(
                &ctx("capability"),
                &member.capability.name,
                &stage.intent.expected_capability,
            ));
        }
        if member.capability.version != stage.intent.expected_capability_version {
            return Err(mismatch(
                &ctx("capability_version"),
                &member.capability.version.to_string(),
                &stage.intent.expected_capability_version.to_string(),
            ));
        }
        if member.manifest_digest != stage.intent.expected_manifest_digest {
            return Err(mismatch(
                &ctx("manifest_digest"),
                &member.manifest_digest,
                &stage.intent.expected_manifest_digest,
            ));
        }
        if member.provider_identity != stage.intent.expected_provider {
            return Err(mismatch(
                &ctx("provider_identity"),
                &member.provider_identity,
                &stage.intent.expected_provider,
            ));
        }
        crate::verify_argument_digest(&member.argument_digest, &stage.intent.expected_arguments)
            .map_err(|e| AuthorityError::Validate(format!("{}: {e:?}", ctx("argument_digest"))))?;
        if member.execution_id.is_empty() {
            return Err(AuthorityError::Validate(ctx("missing_execution_id")));
        }
        members.push(VerifiedBundleMember {
            stage_index: index,
            execution_id: member.execution_id.clone(),
            action_id: member.action_id.clone(),
            prepared_id: member.prepared_id.clone(),
        });
    }
    if dispatch.authorizes_physical_execution_by_tethers {
        return Err(AuthorityError::Validate(
            "bundle.dispatch.ownership_inverted: gate claims Tethers executes".to_string(),
        ));
    }
    if !dispatch.host_must_report_outcome {
        return Err(AuthorityError::Validate(
            "bundle.dispatch.outcome_not_required: host_must_report_outcome != true".to_string(),
        ));
    }
    Ok(VerifiedBundle {
        bundle_id: dispatch.bundle_id.clone(),
        composition_digest: dispatch.composition_digest.clone(),
        members,
        raw: dispatch.clone(),
    })
}

/// An authorised pipe held across the physical boundary: the verified
/// bundle plus the exact stages and redirect it authorises. Spawn only
/// through [`spawn_authorised`], which re-verifies live material first.
#[derive(Debug, Clone)]
pub struct AuthorisedBundle {
    pub bundle: VerifiedBundle,
    pub composition_digest: String,
    pub stages: Vec<PipeStage>,
    pub redirect: Option<PipeRedirect>,
}

/// Steps 1–5 with zero spawn: compose over live material, sequential
/// PREPARE for every stage (any non-allow stops the world), one atomic
/// `commit_bundle`, dispatch verification against Omen's own truth, and
/// a drift re-check. Returns the authorised bundle; nothing is spawned.
pub fn authorise_pipe<T: GateTransport>(
    driver: &mut AdmitExecute<T>,
    stages: Vec<PipeStage>,
    redirect: Option<PipeRedirect>,
) -> Result<AuthorisedBundle, AuthorityError> {
    // 1. Composition over LIVE material, before any authority contact.
    let (_composition, composition_digest) = compose_identity(&stages, redirect.as_ref())?;
    // 2. Sequential PREPARE for every stage. Any non-allow stops the
    //    world with zero spawns (no commit, no dispatch, no execution).
    //    Pipes require standing allow: ask/defer has no pipeline meaning.
    let mut prepared_ids: Vec<String> = Vec::with_capacity(stages.len());
    let approvals: HashMap<String, String> = HashMap::new();
    for (index, stage) in stages.iter().enumerate() {
        match driver.prepare(&stage.intent)? {
            PrepareOutcome::AllowPrepared { prepared_id, .. } => {
                prepared_ids.push(prepared_id);
            }
            PrepareOutcome::Ask { .. } => {
                return Err(AuthorityError::Validate(format!(
                    "pipe.stage_{index}_approval_required: pipes require standing allow"
                )));
            }
            PrepareOutcome::Deny { reason } => {
                return Err(AuthorityError::CommitRefused(format!(
                    "pipe.stage_{index}_deny: {reason}"
                )));
            }
            PrepareOutcome::Unavailable { reason } => {
                return Err(AuthorityError::CommitRefused(format!(
                    "pipe.stage_{index}_unavailable: {reason}"
                )));
            }
        }
    }
    // 3. Atomic COMMIT before the first spawn.
    let dispatch = match driver.commit_bundle(&prepared_ids, &approvals)? {
        BundleCommitOutcome::Admitted { dispatch } => dispatch,
        BundleCommitOutcome::Refused { code, message } => {
            return Err(AuthorityError::CommitRefused(format!(
                "pipe.commit_bundle.{code}: {message}"
            )));
        }
    };
    // 4. Verify the dispatch against Omen's own truth. Mismatch: zero spawn.
    let bundle = verify_bundle_dispatch(&dispatch, &stages, &composition_digest)?;
    // 5. Drift re-verification against LIVE material: executable/CWD/argv
    //    (and redirect) changes after authority fail closed here.
    let (_live, live_digest) = compose_identity(&stages, redirect.as_ref())?;
    if live_digest != composition_digest {
        return Err(AuthorityError::Validate(
            "pipe.composition_drift: physical material changed after authority".to_string(),
        ));
    }
    Ok(AuthorisedBundle {
        bundle,
        composition_digest,
        stages,
        redirect,
    })
}

/// Outcome of the authorise-then-spawn pipeline run.
#[derive(Debug)]
pub struct AuthorisedPipeReport {
    pub bundle: VerifiedBundle,
    /// Per-stage physical truth in stage order. `attempted == false`
    /// entries are unstarted bundle members (partial start): reported via
    /// the bounded `not_attempted` outcome, never executed, never
    /// retried as execution.
    pub attempts: Vec<StageAttempt>,
    pub pipeline: Option<PipelineExecutionOutput>,
    /// Number of physical processes Omen spawned (observability for the
    /// zero-spawn laws; tests assert this directly).
    pub spawn_count: usize,
}

/// One stage's physical truth.
#[derive(Debug, Clone)]
pub struct StageAttempt {
    pub stage_index: usize,
    pub execution_id: String,
    /// True only when Omen actually spawned this stage.
    pub attempted: bool,
    pub classification: &'static str,
    pub error: Option<String>,
}

/// Spawn an authorised bundle: re-verify live material against the
/// authorised digest (drift fails closed with zero new spawn), run the OS
/// pipe, apply the authorised redirect, and build truthful per-stage
/// attempts. Omen owns the bytes from here; Tethers owns the outcome
/// truth the caller reports next.
pub async fn spawn_authorised(
    supervisor: &ProcessSupervisor,
    authorised: AuthorisedBundle,
) -> Result<AuthorisedPipeReport, AuthorityError> {
    let AuthorisedBundle {
        bundle,
        composition_digest,
        stages,
        redirect,
    } = authorised;
    // 6. Live drift gate: the material must still be exactly what the
    //    bundle authorised. Anything else fails closed before first spawn.
    let (_live, live_digest) = compose_identity(&stages, redirect.as_ref())?;
    if live_digest != composition_digest {
        return Err(AuthorityError::Validate(
            "pipe.composition_drift: physical material changed after authority".to_string(),
        ));
    }
    // 7. Spawn the OS pipe. Omen owns the bytes from here.
    let requests: Vec<omen_engine::supervisor::ExecutionRequest> = stages
        .iter()
        .map(|s| omen_engine::supervisor::ExecutionRequest::simple(s.argv.clone(), s.cwd.clone()))
        .collect();
    let output = supervisor
        .execute_pipeline(requests)
        .await
        .map_err(|e| AuthorityError::Validate(format!("pipe.spawn.failed: {e:?}")))?;
    // 8. Redirect (authorised target only): applied after a successful
    //    pipe run, before outcome reporting.
    if let Some(target) = redirect.as_ref() {
        std::fs::write(&target.path, &output.execution.stdout_all)
            .map_err(|e| AuthorityError::Validate(format!("pipe.redirect.failed: {e}")))?;
    }
    // 9. Truthful per-stage attempts: every spawned stage was attempted
    //    exactly once; exits classify succeeded/failed.
    let attempts = bundle
        .members
        .iter()
        .enumerate()
        .map(|(index, m)| {
            let exit = output.stage_exits.get(index);
            let ok = exit.map(|x| x.code == Some(0)).unwrap_or(false);
            StageAttempt {
                stage_index: index,
                execution_id: m.execution_id.clone(),
                attempted: true,
                classification: if ok { "succeeded" } else { "failed" },
                error: if ok {
                    None
                } else {
                    Some(format!("pipe.stage_{index}_exit: {exit:?}"))
                },
            }
        })
        .collect::<Vec<_>>();
    let spawn_count = attempts.iter().filter(|a| a.attempted).count();
    Ok(AuthorisedPipeReport {
        bundle,
        attempts,
        pipeline: Some(output),
        spawn_count,
    })
}

/// Convenience: authorise then spawn in one call (steps 1–9).
pub async fn authorise_and_spawn_pipe<T: GateTransport>(
    driver: &mut AdmitExecute<T>,
    supervisor: &ProcessSupervisor,
    stages: Vec<PipeStage>,
    redirect: Option<PipeRedirect>,
) -> Result<AuthorisedPipeReport, AuthorityError> {
    let authorised = authorise_pipe(driver, stages, redirect)?;
    spawn_authorised(supervisor, authorised).await
}

/// Report one member outcome on /2: attempted members with their real
/// classification; unstarted members (partial start) via the bounded
/// `not_attempted` outcome with attempted=false and neither result nor
/// error.
pub fn member_outcome_payload(
    execution_id: &str,
    attempt: &StageAttempt,
    success_result: Option<serde_json::Value>,
) -> serde_json::Value {
    if attempt.attempted {
        let (result, error) = match attempt.classification {
            "succeeded" => (success_result, None),
            _ => (
                None,
                Some(
                    attempt
                        .error
                        .clone()
                        .unwrap_or_else(|| "omen.pipe.stage_failed".to_string()),
                ),
            ),
        };
        crate::outcome::bundle_member_outcome_payload(
            execution_id,
            attempt.classification,
            true,
            result,
            error,
        )
    } else {
        crate::outcome::bundle_member_outcome_payload(
            execution_id,
            crate::protocol::OUTCOME_NOT_ATTEMPTED,
            false,
            None,
            None,
        )
    }
}

/// Mark every member at and after `first_unstarted` as unattempted
/// (partial physical start): earlier members keep their real attempts,
/// later members are never executed and reported `not_attempted`.
pub fn mark_unstarted_members(report: &mut AuthorisedPipeReport, first_unstarted: usize) {
    for attempt in report.attempts.iter_mut() {
        if attempt.stage_index >= first_unstarted {
            attempt.attempted = false;
            attempt.classification = crate::protocol::OUTCOME_NOT_ATTEMPTED;
            attempt.error = Some("omen.pipe.not_attempted: stage never started".to_string());
        }
    }
    report.spawn_count = report.attempts.iter().filter(|a| a.attempted).count();
    report.pipeline = None;
}

/// Approvals map helper: pipes require standing allow, so the approvals
/// map is always empty. The parameter exists so the Gate payload shape
/// stays exact (no `approvals` key when empty).
pub fn empty_approvals() -> HashMap<String, String> {
    HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(exe: &str, argv: &[&str]) -> PipeStage {
        PipeStage {
            exe: PathBuf::from(exe),
            argv: argv.iter().map(|s| s.to_string()).collect(),
            cwd: PathBuf::from("/tmp"),
            intent: crate::admission::AuthorityIntent {
                tether_id: "t".to_string(),
                tether_version: "1".to_string(),
                evaluation_id: "e".to_string(),
                action_id: "a".to_string(),
                event_id: "v".to_string(),
                event_name: "n".to_string(),
                event_data: serde_json::json!({}),
                facts: serde_json::json!({}),
                expected_arguments: serde_json::json!({}),
                expected_capability: "c".to_string(),
                expected_capability_version: 1,
                expected_manifest_digest: "d".to_string(),
                expected_provider: "p".to_string(),
                timeout_ms: 1000,
                success_result: serde_json::json!({}),
            },
        }
    }

    #[test]
    fn composition_rejects_short_pipes() {
        let one = vec![stage("/bin/a", &["/bin/a"])];
        assert!(compose_identity(&one, None).is_err());
    }

    #[test]
    fn not_attempted_payload_shape_is_exact() {
        let attempt = StageAttempt {
            stage_index: 1,
            execution_id: "exec_1".to_string(),
            attempted: false,
            classification: crate::protocol::OUTCOME_NOT_ATTEMPTED,
            error: None,
        };
        let payload = member_outcome_payload("exec_1", &attempt, None);
        assert_eq!(
            payload["classification"],
            serde_json::json!("not_attempted")
        );
        assert_eq!(payload["attempted"], serde_json::json!(false));
        assert!(payload.get("result").is_none());
        assert!(payload.get("error").is_none());
    }

    #[test]
    fn attempted_payload_carries_result_or_error() {
        let ok = StageAttempt {
            stage_index: 0,
            execution_id: "exec_0".to_string(),
            attempted: true,
            classification: "succeeded",
            error: None,
        };
        let payload = member_outcome_payload("exec_0", &ok, Some(serde_json::json!({"echo": "x"})));
        assert_eq!(payload["attempted"], serde_json::json!(true));
        assert_eq!(payload["result"], serde_json::json!({"echo": "x"}));
        let failed = StageAttempt {
            stage_index: 1,
            execution_id: "exec_1".to_string(),
            attempted: true,
            classification: "failed",
            error: Some("boom".to_string()),
        };
        let payload = member_outcome_payload("exec_1", &failed, None);
        assert_eq!(payload["error"], serde_json::json!("boom"));
    }
}
