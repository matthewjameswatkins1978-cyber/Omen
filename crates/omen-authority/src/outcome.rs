//! OUTCOME handoff + durable delivery truth (Omen-owned, not authority).
//!
//! After physical execution Omen reports truthfully: execution id,
//! attempted, classification, result/error projection, evidence. Success
//! is never claimed from spawn alone; unknown stays unknown.
//!
//! If OUTCOME delivery fails after real execution, Omen does NOT
//! re-execute: the attempt is journaled (bounded file) so delivery can
//! be retried or reconciled safely. Idempotent repeats carry the same
//! classification. This journal records OMEN'S delivery truth — it
//! grants no authority and competes with nothing.

use crate::AuthorityError;
use crate::binding::VerifiedExecutionBinding;
use crate::executor::ExecAttempt;
use crate::protocol::OutcomeResult;
use crate::transport::{GateTransport, RoundtripError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Bounded journal capacity (entries; oldest dropped beyond).
pub const JOURNAL_CAP: usize = 256;

/// One journaled execution (delivery truth, not authority).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeRecord {
    pub execution_id: String,
    pub action_id: String,
    pub capability: String,
    pub attempted: bool,
    pub classification: String,
    pub outcome_sent: bool,
    pub terminal: bool,
    pub replay_terminal: Option<String>,
}

/// A queued delivery (response lost or gate unreachable after exec).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingOutcome {
    pub execution_id: String,
    pub payload: Value,
}

/// File-backed delivery journal. Append-only JSON lines; best-effort
/// (a journal fault never blocks execution truth — it surfaces).
pub struct OutcomeJournal {
    path: std::path::PathBuf,
}

impl OutcomeJournal {
    pub fn open(path: std::path::PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn read_all(&self) -> Vec<OutcomeRecord> {
        std::fs::read_to_string(&self.path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn write_all(&self, records: &[OutcomeRecord]) -> Result<(), AuthorityError> {
        let mut text = String::new();
        for r in records.iter().take(JOURNAL_CAP) {
            text.push_str(&serde_json::to_string(r).map_err(|e| {
                AuthorityError::OutcomeIncomplete(format!("journal.serialize.failed: {e}"))
            })?);
            text.push('\n');
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AuthorityError::OutcomeIncomplete(format!("journal.dir.failed: {e}"))
            })?;
        }
        std::fs::write(&self.path, text)
            .map_err(|e| AuthorityError::OutcomeIncomplete(format!("journal.write.failed: {e}")))
    }

    /// Already terminal (recorded outcome)? Then never execute again.
    pub fn is_terminal(&self, execution_id: &str) -> bool {
        self.read_all()
            .iter()
            .any(|r| r.execution_id == execution_id && r.terminal)
    }

    pub fn record(&self, record: OutcomeRecord) -> Result<(), AuthorityError> {
        let mut all = self.read_all();
        all.retain(|r| r.execution_id != record.execution_id);
        all.push(record);
        self.write_all(&all)
    }
}

/// Build the OUTCOME payload from a verified execution binding + real attempt.
/// `attempted == false` yields `None`: nothing is sent (the Gate would
/// refuse `not_attempted`); the caller journals the deferral instead.
pub fn outcome_payload(
    binding: &VerifiedExecutionBinding,
    attempt: &ExecAttempt,
    success_result: &Value,
    evidence: Option<String>,
) -> Option<Value> {
    if !attempt.attempted {
        return None;
    }
    let classification = attempt.classification().as_str().to_string();
    let mut payload = json!({
        "execution_id": binding.execution_id(),
        "classification": classification,
        "attempted": true,
        "external_execution_identity": attempt.omen_exec_id,
    });
    match attempt.classification() {
        crate::protocol::OutcomeClassification::Succeeded => {
            payload["result"] = success_result.clone();
        }
        crate::protocol::OutcomeClassification::Failed => {
            payload["error"] = Value::String(
                attempt
                    .error_text()
                    .unwrap_or_else(|| "omen.exec.failed".to_string()),
            );
        }
        crate::protocol::OutcomeClassification::Uncertain => {}
    }
    if let Some(ev) = evidence {
        payload["evidence"] = Value::String(ev);
    }
    Some(payload)
}

/// Deliver OUTCOME with one idempotent retry on transport timeout: the
/// retry carries the SAME classification for the SAME execution (never a
/// re-execution). A refused outcome (conflict/unknown) surfaces; a lost
/// response returns the pending payload for safe retry.
pub fn deliver_outcome<T: GateTransport>(
    transport: &mut T,
    payload: &Value,
) -> Result<OutcomeDelivered, AuthorityError> {
    match transport.roundtrip("outcome", payload.clone()) {
        Ok(raw) => {
            let parsed = OutcomeResult::parse(&raw)?;
            Ok(OutcomeDelivered::Recorded(parsed))
        }
        Err(RoundtripError::Refused { code, message, .. }) => {
            Ok(OutcomeDelivered::Refused { code, message })
        }
        Err(RoundtripError::Transport(_)) => {
            // Response possibly lost AFTER the Gate recorded: retry once
            // with identical classification (idempotent repeat path).
            match transport.roundtrip("outcome", payload.clone()) {
                Ok(raw) => {
                    let parsed = OutcomeResult::parse(&raw)?;
                    Ok(OutcomeDelivered::Recorded(parsed))
                }
                Err(RoundtripError::Refused { code, message, .. }) => {
                    Ok(OutcomeDelivered::Refused { code, message })
                }
                Err(RoundtripError::Transport(e)) => Err(AuthorityError::OutcomeIncomplete(
                    format!("outcome.delivery.lost: {e:?}; retry delivery, never re-execute"),
                )),
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum OutcomeDelivered {
    Recorded(OutcomeResult),
    Refused { code: String, message: String },
}
