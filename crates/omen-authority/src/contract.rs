//! Machine + human projection of authority truth.
//!
//! Machine-facing output exposes authority structurally (additive — no
//! contract bump): `authority_required`, `authority_state`,
//! `tethers_protocol`, execution/admission identities, `attempted`,
//! `outcome_state`, `recovery_required`. Agents never scrape prose.
//! Humans get small closed-vocabulary outcomes.

use serde::{Deserialize, Serialize};

/// Machine projection of one authority-required step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityProjection {
    pub authority_required: bool,
    pub authority_state: AuthorityState,
    pub tethers_protocol: String,
    pub tethers_product: String,
    pub gate_instance_id: Option<String>,
    pub execution_id: Option<String>,
    pub prepared_id: Option<String>,
    pub approval_id: Option<String>,
    pub approval_consumed: bool,
    pub attempted: bool,
    pub outcome_state: Option<String>,
    pub recovery_required: bool,
    pub spawn_count: u64,
}

/// Closed authority-state vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityState {
    Admitted,
    Denied,
    ApprovalRequired,
    AuthorityUnavailable,
    StaleRevoked,
    MalformedAuthority,
    ExecutionFailed,
    OutcomeIncomplete,
}

impl AuthorityState {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthorityState::Admitted => "admitted",
            AuthorityState::Denied => "denied",
            AuthorityState::ApprovalRequired => "approval_required",
            AuthorityState::AuthorityUnavailable => "authority_unavailable",
            AuthorityState::StaleRevoked => "stale_revoked",
            AuthorityState::MalformedAuthority => "malformed_authority",
            AuthorityState::ExecutionFailed => "execution_failed",
            AuthorityState::OutcomeIncomplete => "outcome_incomplete",
        }
    }
}

/// Small human outcome (Lens owns richer interaction; not here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HumanOutcome {
    Admitted,
    Denied(String),
    ApprovalRequired(String),
    AuthorityUnavailable(String),
    StaleRevoked(String),
    MalformedAuthority(String),
    ExecutionFailed(String),
    OutcomeIncomplete(String),
}

impl HumanOutcome {
    pub fn line(&self) -> String {
        match self {
            HumanOutcome::Admitted => "admitted: executing under current commit".to_string(),
            HumanOutcome::Denied(r) => format!("denied: {r} (zero spawn)"),
            HumanOutcome::ApprovalRequired(id) => {
                format!("approval required: {id} (zero spawn until approved + fresh commit)")
            }
            HumanOutcome::AuthorityUnavailable(r) => format!("authority unavailable: {r}"),
            HumanOutcome::StaleRevoked(r) => format!("stale/revoked: {r} (zero spawn)"),
            HumanOutcome::MalformedAuthority(r) => format!("malformed authority: {r} (zero spawn)"),
            HumanOutcome::ExecutionFailed(r) => format!("execution failed: {r}"),
            HumanOutcome::OutcomeIncomplete(r) => format!("outcome reconciliation incomplete: {r}"),
        }
    }

    pub fn state(&self) -> AuthorityState {
        match self {
            HumanOutcome::Admitted => AuthorityState::Admitted,
            HumanOutcome::Denied(_) => AuthorityState::Denied,
            HumanOutcome::ApprovalRequired(_) => AuthorityState::ApprovalRequired,
            HumanOutcome::AuthorityUnavailable(_) => AuthorityState::AuthorityUnavailable,
            HumanOutcome::StaleRevoked(_) => AuthorityState::StaleRevoked,
            HumanOutcome::MalformedAuthority(_) => AuthorityState::MalformedAuthority,
            HumanOutcome::ExecutionFailed(_) => AuthorityState::ExecutionFailed,
            HumanOutcome::OutcomeIncomplete(_) => AuthorityState::OutcomeIncomplete,
        }
    }
}
