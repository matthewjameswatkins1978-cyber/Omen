//! Exact action binding: the dispatch Omen executes must be the action
//! Tethers admitted — field by field, no best-effort translation.
//!
//! Verified before spawn: schema, execution/evaluation/action/prepared
//! identities, capability name + version, manifest digest, provider
//! identity, the recomputed `argument_digest` over Omen's own expected
//! arguments, the authority protocol, and the ownership flags
//! (`authorizes_physical_execution_by_tethers` MUST be false — Tethers
//! did not and does not execute; `host_must_report_outcome` MUST be
//! true). Any mismatch is zero spawn.
//!
//! Scope note: `tethers.dispatch/1` carries no separate scope field.
//! Scope binds through the two digests — the manifest digest covers the
//! capability's `permission_scope` (prefixes, pointer), and the argument
//! digest covers the scoped argument values (e.g. `/path`). A scope
//! violation therefore cannot survive verification: either the manifest
//! differs (digest mismatch) or the scoped values differ (digest
//! mismatch).

use crate::AuthorityError;
use crate::protocol::DispatchRecord;

/// The intent context a dispatch is verified against (built from the
/// same [`crate::AuthorityIntent`] that produced the preparation).
#[derive(Debug, Clone)]
pub struct DispatchContext {
    pub evaluation_id: String,
    pub action_id: String,
    pub prepared_id: String,
    pub expected_capability: String,
    pub expected_capability_version: u32,
    pub expected_manifest_digest: String,
    pub expected_provider: String,
    pub expected_arguments: serde_json::Value,
}

/// A dispatch that survived every binding check. The ONLY value the
/// executor accepts.
#[derive(Debug, Clone)]
pub struct VerifiedDispatch {
    pub execution_id: String,
    pub action_id: String,
    pub capability_name: String,
    pub capability_version: u32,
    pub argument_digest: String,
    pub raw: DispatchRecord,
}

pub fn verify_dispatch(
    dispatch: &DispatchRecord,
    ctx: &DispatchContext,
) -> Result<VerifiedDispatch, AuthorityError> {
    let c = &dispatch.commit;
    let mismatch = |field: &str, got: &str, want: &str| {
        AuthorityError::Validate(format!("dispatch.{field}_mismatch: {got} != {want}"))
    };
    if c.evaluation_id != ctx.evaluation_id {
        return Err(mismatch(
            "evaluation_id",
            &c.evaluation_id,
            &ctx.evaluation_id,
        ));
    }
    if c.action_id != ctx.action_id {
        return Err(mismatch("action_id", &c.action_id, &ctx.action_id));
    }
    if c.prepared_id != ctx.prepared_id {
        return Err(mismatch("prepared_id", &c.prepared_id, &ctx.prepared_id));
    }
    if c.capability.name != ctx.expected_capability {
        return Err(mismatch(
            "capability",
            &c.capability.name,
            &ctx.expected_capability,
        ));
    }
    if c.capability.version != ctx.expected_capability_version {
        return Err(AuthorityError::Validate(format!(
            "dispatch.capability_version_mismatch: {} != {}",
            c.capability.version, ctx.expected_capability_version
        )));
    }
    if c.manifest_digest != ctx.expected_manifest_digest {
        return Err(mismatch(
            "manifest_digest",
            &c.manifest_digest,
            &ctx.expected_manifest_digest,
        ));
    }
    if c.provider_identity != ctx.expected_provider {
        return Err(mismatch(
            "provider_identity",
            &c.provider_identity,
            &ctx.expected_provider,
        ));
    }
    // The cryptographic bind: recompute over Omen's OWN expected
    // arguments. A Core plan that drifted from Omen's intent (wrong
    // event, tampered mapping) cannot survive this check.
    crate::verify_argument_digest(&c.argument_digest, &ctx.expected_arguments)?;
    if c.authorizes_physical_execution_by_tethers {
        return Err(AuthorityError::Validate(
            "dispatch.ownership_inverted: gate claims Tethers executes".to_string(),
        ));
    }
    if !c.host_must_report_outcome {
        return Err(AuthorityError::Validate(
            "dispatch.outcome_not_required: host_must_report_outcome != true".to_string(),
        ));
    }
    if c.execution_id.is_empty() {
        return Err(AuthorityError::Validate(
            "dispatch.missing_execution_id".to_string(),
        ));
    }
    Ok(VerifiedDispatch {
        execution_id: c.execution_id.clone(),
        action_id: c.action_id.clone(),
        capability_name: c.capability.name.clone(),
        capability_version: c.capability.version,
        argument_digest: c.argument_digest.clone(),
        raw: dispatch.clone(),
    })
}
