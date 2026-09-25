//! Phased authority errors. Every failure names the phase it stopped in
//! (`discover`, `spawn`, `initialize`, `request`, `receive`, `validate`,
//! `commit`, `shutdown`) so an agent never reconstructs the path from prose.

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AuthorityError {
    /// Gate companion discovery/resolution failed (missing, hash mismatch,
    /// no managed companion). Zero spawn.
    #[error("authority.discover.failed: {0}")]
    Discover(String),
    /// Gate process would not start or the hello handshake failed/timed
    /// out. Zero spawn.
    #[error("authority.initialize.failed: {0}")]
    Initialize(String),
    /// A protocol frame could not be sent (broken pipe, oversized
    /// payload). Zero spawn unless a COMMIT already completed.
    #[error("authority.request.failed: {0}")]
    Request(String),
    /// No/invalid response: timeout, malformed NDJSON, oversized frame,
    /// wrong schema, duplicate request identity. Fail closed.
    #[error("authority.receive.failed: {0}")]
    Receive(String),
    /// Response understood but binding/identity check failed: wrong
    /// protocol/product, capability/scope/digest mismatch, stale or
    /// revoked admission, recovery required. Zero spawn.
    #[error("authority.validate.failed: {0}")]
    Validate(String),
    /// Tethers refused admission (DENY, ASK-unapproved, revoked, replay
    /// refusal, stale preparation). Zero spawn. Carries the Gate code.
    #[error("authority.commit.refused: {0}")]
    CommitRefused(String),
    /// Physical execution failed AFTER a valid COMMIT (exactly one spawn
    /// was attempted; truthful outcome still reported).
    #[error("authority.execute.failed: {0}")]
    Execute(String),
    /// OUTCOME delivery failed after real execution. No re-execution;
    /// retry/reconcile delivery only. Carries the pending execution id.
    #[error("authority.outcome.incomplete: {0}")]
    OutcomeIncomplete(String),
    /// Gate supervision failure after start (crash, stderr flood,
    /// shutdown fault). Fail closed.
    #[error("authority.supervision.failed: {0}")]
    Supervision(String),
}

impl AuthorityError {
    pub fn phase(&self) -> &'static str {
        match self {
            AuthorityError::Discover(_) => "discover",
            AuthorityError::Initialize(_) => "initialize",
            AuthorityError::Request(_) => "request",
            AuthorityError::Receive(_) => "receive",
            AuthorityError::Validate(_) => "validate",
            AuthorityError::CommitRefused(_) => "commit",
            AuthorityError::Execute(_) => "execute",
            AuthorityError::OutcomeIncomplete(_) => "outcome",
            AuthorityError::Supervision(_) => "supervision",
        }
    }
}
