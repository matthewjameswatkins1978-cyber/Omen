use chrono::Utc;
use omen_core::composition::StateChange;
use omen_core::{CoreError, ExecutionId, FactId, InteractiveSessionId, ResourceUri};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_HISTORY_LIMIT: usize = 20;
pub const MAX_HISTORY_LIMIT: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryQuery {
    pub all_sessions: bool,
    pub session_id: Option<InteractiveSessionId>,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HistoryStatus {
    Completed,
    Failed,
    Refused,
    TimedOut,
    Partial,
    Cancelled,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub sequence: i64,
    pub execution_id: ExecutionId,
    pub session_id: InteractiveSessionId,
    pub command: String,
    pub status: HistoryStatus,
    pub recorded_at: String,
    pub duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_changed: Option<StateChange>,
    pub evidence: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryResult {
    pub schema_version: u32,
    pub entries: Vec<HistoryEntry>,
    pub limit: usize,
    pub all_sessions: bool,
    pub ordering: String,
    pub pagination: String,
}

const HISTORY_SCHEMA_VERSION: u32 = 1;

pub fn query_history(
    db: &crate::db::Database,
    query: &HistoryQuery,
) -> Result<HistoryResult, CoreError> {
    if query.limit == 0 || query.limit > MAX_HISTORY_LIMIT {
        return Err(CoreError::ExecutionFailedCode {
            code: omen_core::ErrorCode::SchemaViolation,
            message: format!("history limit must be between 1 and {MAX_HISTORY_LIMIT}"),
        });
    }
    if !query.all_sessions && query.session_id.is_none() {
        return Err(CoreError::ExecutionFailedCode {
            code: omen_core::ErrorCode::SchemaViolation,
            message: "current-session history requires a session_id".into(),
        });
    }

    let sql = if query.all_sessions {
        "SELECT rowid, execution_id, session_id, command, exit_code, duration_ms, stdout_artifact, stderr_artifact, envelope_json, created_at FROM execution_history ORDER BY rowid DESC LIMIT ?1"
    } else {
        "SELECT rowid, execution_id, session_id, command, exit_code, duration_ms, stdout_artifact, stderr_artifact, envelope_json, created_at FROM execution_history WHERE session_id = ?1 ORDER BY rowid DESC LIMIT ?2"
    };
    let mut stmt = db
        .conn()
        .prepare(sql)
        .map_err(|e| CoreError::ExecutionFailedCode {
            code: omen_core::ErrorCode::PersistenceFailure,
            message: format!("failed to prepare history query: {e}"),
        })?;

    let rows = if query.all_sessions {
        stmt.query_map(params![query.limit as i64], history_entry_from_row)
            .map_err(|e| CoreError::ExecutionFailedCode {
                code: omen_core::ErrorCode::PersistenceFailure,
                message: format!("failed to query history: {e}"),
            })?
            .collect::<Result<Vec<_>, _>>()
    } else {
        stmt.query_map(
            params![
                query.session_id.as_ref().unwrap().as_str(),
                query.limit as i64
            ],
            history_entry_from_row,
        )
        .map_err(|e| CoreError::ExecutionFailedCode {
            code: omen_core::ErrorCode::PersistenceFailure,
            message: format!("failed to query history: {e}"),
        })?
        .collect::<Result<Vec<_>, _>>()
    };
    let entries = rows.map_err(|e| CoreError::ExecutionFailedCode {
        code: omen_core::ErrorCode::PersistenceFailure,
        message: format!("failed to decode history: {e}"),
    })?;

    Ok(HistoryResult {
        schema_version: HISTORY_SCHEMA_VERSION,
        entries,
        limit: query.limit,
        all_sessions: query.all_sessions,
        ordering: "sqlite_rowid_desc".into(),
        pagination: "none_bounded_limit".into(),
    })
}

fn history_entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    let envelope_json: Option<String> = row.get(8)?;
    let envelope = envelope_json
        .as_deref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    let exit_code: Option<i32> = row.get(4)?;
    let status = envelope
        .as_ref()
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        .and_then(parse_history_status)
        .unwrap_or(match exit_code {
            Some(0) => HistoryStatus::Completed,
            Some(_) => HistoryStatus::Failed,
            None => HistoryStatus::Unknown,
        });
    let state_changed = envelope
        .as_ref()
        .and_then(|value| value.get("state_changed"))
        .and_then(Value::as_str)
        .and_then(parse_state_change);
    let mut evidence = Vec::new();
    for index in [6, 7] {
        if let Some(uri) = row.get::<_, Option<String>>(index)? {
            evidence.push(uri);
        }
    }
    if let Some(uri) = envelope
        .as_ref()
        .and_then(|value| value.get("evidence_artifact"))
        .and_then(Value::as_str)
    {
        evidence.push(uri.into());
    }
    if let Some(artifacts) = envelope
        .as_ref()
        .and_then(|value| value.get("artifacts"))
        .and_then(Value::as_array)
    {
        evidence.extend(
            artifacts
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
    evidence.sort();
    evidence.dedup();

    Ok(HistoryEntry {
        sequence: row.get(0)?,
        execution_id: ExecutionId::new(row.get::<_, String>(1)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        session_id: InteractiveSessionId::new(row.get::<_, String>(2)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        command: row.get(3)?,
        status,
        recorded_at: row.get(9)?,
        duration_ms: row.get(5)?,
        state_changed,
        evidence,
        error: envelope.and_then(|value| value.get("error").cloned()),
    })
}

fn parse_history_status(value: &str) -> Option<HistoryStatus> {
    match value {
        "COMPLETED" | "Completed" => Some(HistoryStatus::Completed),
        "FAILED" | "Failed" => Some(HistoryStatus::Failed),
        "SPAWN_FAILED" | "CONTAINMENT_FAILED" | "IO_FAILED" => Some(HistoryStatus::Failed),
        "REFUSED" | "Refused" => Some(HistoryStatus::Refused),
        "TIMED_OUT" | "TimedOut" => Some(HistoryStatus::TimedOut),
        "CANCELLED" | "Cancelled" => Some(HistoryStatus::Cancelled),
        "PARTIAL" | "Partial" => Some(HistoryStatus::Partial),
        "OUTCOME_UNKNOWN" | "UNKNOWN" | "Unknown" => Some(HistoryStatus::Unknown),
        _ => None,
    }
}

fn parse_state_change(value: &str) -> Option<StateChange> {
    match value {
        "NO" | "No" => Some(StateChange::No),
        "YES" | "Yes" => Some(StateChange::Yes),
        "POSSIBLE" | "Possible" => Some(StateChange::Possible),
        _ => None,
    }
}

/// Project a history result for surfaces that must communicate unjournaled
/// local execution truthfully.
///
/// Direct local execution writes a `local-execution-status.json` marker
/// (`UNJOURNALED_LOCAL_EXECUTION`) beside the workspace state. When that
/// marker exists, the returned value wraps the durable result as
/// `{history, history_status, local_execution}`; without a marker the
/// durable result is returned unchanged. CLI (`omen history --machine`)
/// and MCP (`omen_history_query`) share this constructor so both surfaces
/// project identical truth instead of one of them hiding the marker.
pub fn history_view_with_unjournaled_marker(
    workspace_root: &std::path::Path,
    history: &HistoryResult,
) -> Value {
    let history_value = serde_json::to_value(history).expect("history result is serializable");
    let marker = std::fs::read_to_string(crate::workspace::local_execution_status_path(
        workspace_root,
    ))
    .ok()
    .and_then(|contents| serde_json::from_str::<Value>(&contents).ok());
    match marker {
        Some(local_execution) => serde_json::json!({
            "history": history_value,
            "history_status": "UNJOURNALED_LOCAL_EXECUTION",
            "local_execution": local_execution,
        }),
        None => history_value,
    }
}

/// Read the unjournaled local-execution marker for a workspace, if present.
pub fn read_unjournaled_marker(workspace_root: &std::path::Path) -> Option<Value> {
    std::fs::read_to_string(crate::workspace::local_execution_status_path(
        workspace_root,
    ))
    .ok()
    .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractiveSessionRecord {
    pub session_id: InteractiveSessionId,
    pub actor: String,
    pub created_at: String,
    pub last_active: String,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    pub execution_id: ExecutionId,
    pub session_id: InteractiveSessionId,
    pub command: String,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
    pub stdout_artifact: Option<String>,
    pub stderr_artifact: Option<String>,
    pub envelope_json: Option<String>,
    pub created_at: String,
}

pub struct ExecutionHistory;

impl ExecutionHistory {
    pub fn register_session(
        db: &mut crate::db::Database,
        session_id: &InteractiveSessionId,
        actor: &str,
        cwd: &str,
    ) -> Result<(), CoreError> {
        let now = Utc::now().to_rfc3339();
        db.conn_mut()
            .execute(
                r#"
                INSERT INTO interactive_sessions (session_id, actor, created_at, last_active, cwd)
                VALUES (?1, ?2, ?3, ?3, ?4)
                ON CONFLICT(session_id) DO UPDATE SET last_active = ?3, cwd = ?4
                "#,
                params![session_id.as_str(), actor, now, cwd],
            )
            .map_err(|e| CoreError::Internal(format!("Failed to register session: {e}")))?;
        Ok(())
    }

    pub fn record_execution(
        db: &mut crate::db::Database,
        record: &ExecutionRecord,
        resources: &[(ResourceUri, String)],
        facts: &[FactId],
        artifacts: &[ResourceUri],
    ) -> Result<(), CoreError> {
        let tx = db
            .conn_mut()
            .transaction()
            .map_err(|e| CoreError::Internal(format!("Failed to begin execution tx: {e}")))?;

        // Ensure session exists
        let now = Utc::now().to_rfc3339();
        tx.execute(
            r#"
            INSERT INTO interactive_sessions (session_id, actor, created_at, last_active, cwd)
            VALUES (?1, 'human', ?2, ?2, '.')
            ON CONFLICT(session_id) DO UPDATE SET last_active = ?2
            "#,
            params![record.session_id.as_str(), now],
        )
        .map_err(|e| CoreError::Internal(format!("Failed to touch session: {e}")))?;

        tx.execute(
            r#"
            INSERT INTO execution_history (
                execution_id, session_id, command, exit_code, duration_ms,
                stdout_artifact, stderr_artifact, envelope_json, created_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                record.execution_id.as_str(),
                record.session_id.as_str(),
                record.command,
                record.exit_code,
                record.duration_ms,
                record.stdout_artifact,
                record.stderr_artifact,
                record.envelope_json,
                record.created_at,
            ],
        )
        .map_err(|e| CoreError::Internal(format!("Failed to insert execution record: {e}")))?;

        for (res, role) in resources {
            tx.execute(
                r#"
                INSERT INTO execution_resources (execution_id, resource_uri, role)
                VALUES (?1, ?2, ?3)
                "#,
                params![record.execution_id.as_str(), res.as_str(), role],
            )
            .map_err(|e| {
                CoreError::Internal(format!("Failed to insert execution resource: {e}"))
            })?;
        }

        for fact in facts {
            tx.execute(
                r#"
                INSERT INTO execution_facts (execution_id, fact_id)
                VALUES (?1, ?2)
                "#,
                params![record.execution_id.as_str(), fact.as_str()],
            )
            .map_err(|e| CoreError::Internal(format!("Failed to insert execution fact: {e}")))?;
        }

        for art in artifacts {
            tx.execute(
                r#"
                INSERT INTO execution_artifacts (execution_id, artifact_uri)
                VALUES (?1, ?2)
                "#,
                params![record.execution_id.as_str(), art.as_str()],
            )
            .map_err(|e| {
                CoreError::Internal(format!("Failed to insert execution artifact: {e}"))
            })?;
        }

        tx.commit()
            .map_err(|e| CoreError::Internal(format!("Failed to commit execution record: {e}")))?;
        Ok(())
    }

    pub fn get_last_execution(
        db: &crate::db::Database,
        session_id: &InteractiveSessionId,
    ) -> Result<Option<ExecutionRecord>, CoreError> {
        let mut stmt = db
            .conn()
            .prepare(
                r#"
                SELECT execution_id, session_id, command, exit_code, duration_ms,
                       stdout_artifact, stderr_artifact, envelope_json, created_at
                FROM execution_history
                WHERE session_id = ?1
                ORDER BY created_at DESC
                LIMIT 1
                "#,
            )
            .map_err(|e| CoreError::Internal(format!("Failed to prepare query: {e}")))?;

        let mut rows = stmt
            .query_map(params![session_id.as_str()], |row| {
                let eid: String = row.get(0)?;
                let sid: String = row.get(1)?;
                let cmd: String = row.get(2)?;
                let code: Option<i32> = row.get(3)?;
                let dur: Option<i64> = row.get(4)?;
                let stdout_art: Option<String> = row.get(5)?;
                let stderr_art: Option<String> = row.get(6)?;
                let env_json: Option<String> = row.get(7)?;
                let cr: String = row.get(8)?;

                Ok(ExecutionRecord {
                    execution_id: ExecutionId::new(eid).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    session_id: InteractiveSessionId::new(sid).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    command: cmd,
                    exit_code: code,
                    duration_ms: dur,
                    stdout_artifact: stdout_art,
                    stderr_artifact: stderr_art,
                    envelope_json: env_json,
                    created_at: cr,
                })
            })
            .map_err(|e| CoreError::Internal(format!("Failed to query execution: {e}")))?;

        if let Some(res) = rows.next() {
            let rec = res.map_err(|e| CoreError::Internal(e.to_string()))?;
            Ok(Some(rec))
        } else {
            Ok(None)
        }
    }

    pub fn get_execution(
        db: &crate::db::Database,
        execution_id: &ExecutionId,
    ) -> Result<Option<ExecutionRecord>, CoreError> {
        let mut stmt = db
            .conn()
            .prepare(
                r#"
                SELECT execution_id, session_id, command, exit_code, duration_ms,
                       stdout_artifact, stderr_artifact, envelope_json, created_at
                FROM execution_history
                WHERE execution_id = ?1
                LIMIT 1
                "#,
            )
            .map_err(|e| CoreError::Internal(format!("Failed to prepare query: {e}")))?;

        let mut rows = stmt
            .query_map(params![execution_id.as_str()], |row| {
                let eid: String = row.get(0)?;
                let sid: String = row.get(1)?;
                let cmd: String = row.get(2)?;
                let code: Option<i32> = row.get(3)?;
                let dur: Option<i64> = row.get(4)?;
                let stdout_art: Option<String> = row.get(5)?;
                let stderr_art: Option<String> = row.get(6)?;
                let env_json: Option<String> = row.get(7)?;
                let cr: String = row.get(8)?;

                Ok(ExecutionRecord {
                    execution_id: ExecutionId::new(eid).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    session_id: InteractiveSessionId::new(sid).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    command: cmd,
                    exit_code: code,
                    duration_ms: dur,
                    stdout_artifact: stdout_art,
                    stderr_artifact: stderr_art,
                    envelope_json: env_json,
                    created_at: cr,
                })
            })
            .map_err(|e| CoreError::Internal(format!("Failed to query execution by id: {e}")))?;

        if let Some(res) = rows.next() {
            let rec = res.map_err(|e| CoreError::Internal(e.to_string()))?;
            Ok(Some(rec))
        } else {
            Ok(None)
        }
    }

    pub fn get_last_failed_execution(
        db: &crate::db::Database,
        session_id: &InteractiveSessionId,
    ) -> Result<Option<ExecutionRecord>, CoreError> {
        let mut stmt = db
            .conn()
            .prepare(
                r#"
                SELECT execution_id, session_id, command, exit_code, duration_ms,
                       stdout_artifact, stderr_artifact, envelope_json, created_at
                FROM execution_history
                WHERE session_id = ?1 AND (exit_code IS NOT NULL AND exit_code != 0)
                ORDER BY created_at DESC
                LIMIT 1
                "#,
            )
            .map_err(|e| CoreError::Internal(format!("Failed to prepare query: {e}")))?;

        let mut rows = stmt
            .query_map(params![session_id.as_str()], |row| {
                let eid: String = row.get(0)?;
                let sid: String = row.get(1)?;
                let cmd: String = row.get(2)?;
                let code: Option<i32> = row.get(3)?;
                let dur: Option<i64> = row.get(4)?;
                let stdout_art: Option<String> = row.get(5)?;
                let stderr_art: Option<String> = row.get(6)?;
                let env_json: Option<String> = row.get(7)?;
                let cr: String = row.get(8)?;

                Ok(ExecutionRecord {
                    execution_id: ExecutionId::new(eid).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    session_id: InteractiveSessionId::new(sid).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    command: cmd,
                    exit_code: code,
                    duration_ms: dur,
                    stdout_artifact: stdout_art,
                    stderr_artifact: stderr_art,
                    envelope_json: env_json,
                    created_at: cr,
                })
            })
            .map_err(|e| CoreError::Internal(format!("Failed to query execution: {e}")))?;

        if let Some(res) = rows.next() {
            let rec = res.map_err(|e| CoreError::Internal(e.to_string()))?;
            Ok(Some(rec))
        } else {
            Ok(None)
        }
    }

    pub fn list_session_executions(
        db: &crate::db::Database,
        session_id: &InteractiveSessionId,
        limit: usize,
    ) -> Result<Vec<ExecutionRecord>, CoreError> {
        let mut stmt = db
            .conn()
            .prepare(
                r#"
                SELECT execution_id, session_id, command, exit_code, duration_ms,
                       stdout_artifact, stderr_artifact, envelope_json, created_at
                FROM execution_history
                WHERE session_id = ?1
                ORDER BY created_at DESC
                LIMIT ?2
                "#,
            )
            .map_err(|e| CoreError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(params![session_id.as_str(), limit as i64], |row| {
                let eid: String = row.get(0)?;
                let sid: String = row.get(1)?;
                let cmd: String = row.get(2)?;
                let code: Option<i32> = row.get(3)?;
                let dur: Option<i64> = row.get(4)?;
                let stdout_art: Option<String> = row.get(5)?;
                let stderr_art: Option<String> = row.get(6)?;
                let env_json: Option<String> = row.get(7)?;
                let cr: String = row.get(8)?;

                Ok(ExecutionRecord {
                    execution_id: ExecutionId::new(eid).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    session_id: InteractiveSessionId::new(sid).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    command: cmd,
                    exit_code: code,
                    duration_ms: dur,
                    stdout_artifact: stdout_art,
                    stderr_artifact: stderr_art,
                    envelope_json: env_json,
                    created_at: cr,
                })
            })
            .map_err(|e| CoreError::Internal(format!("Failed to query executions: {e}")))?;

        let mut list = Vec::new();
        for item in rows {
            list.push(item.map_err(|e| CoreError::Internal(e.to_string()))?);
        }
        Ok(list)
    }

    pub fn list_all_executions(
        db: &crate::db::Database,
        limit: usize,
    ) -> Result<Vec<ExecutionRecord>, CoreError> {
        let mut stmt = db
            .conn()
            .prepare(
                r#"
                SELECT execution_id, session_id, command, exit_code, duration_ms,
                       stdout_artifact, stderr_artifact, envelope_json, created_at
                FROM execution_history
                ORDER BY created_at DESC
                LIMIT ?1
                "#,
            )
            .map_err(|e| CoreError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                let eid: String = row.get(0)?;
                let sid: String = row.get(1)?;
                let cmd: String = row.get(2)?;
                let code: Option<i32> = row.get(3)?;
                let dur: Option<i64> = row.get(4)?;
                let stdout_art: Option<String> = row.get(5)?;
                let stderr_art: Option<String> = row.get(6)?;
                let env_json: Option<String> = row.get(7)?;
                let cr: String = row.get(8)?;

                Ok(ExecutionRecord {
                    execution_id: ExecutionId::new(eid).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    session_id: InteractiveSessionId::new(sid).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    command: cmd,
                    exit_code: code,
                    duration_ms: dur,
                    stdout_artifact: stdout_art,
                    stderr_artifact: stderr_art,
                    envelope_json: env_json,
                    created_at: cr,
                })
            })
            .map_err(|e| CoreError::Internal(format!("Failed to query executions: {e}")))?;

        let mut list = Vec::new();
        for item in rows {
            list.push(item.map_err(|e| CoreError::Internal(e.to_string()))?);
        }
        Ok(list)
    }

    pub fn get_execution_resources(
        db: &crate::db::Database,
        execution_id: &ExecutionId,
    ) -> Result<Vec<(ResourceUri, String)>, CoreError> {
        let mut stmt = db
            .conn()
            .prepare("SELECT resource_uri, role FROM execution_resources WHERE execution_id = ?1")
            .map_err(|e| CoreError::Internal(format!("Failed to prepare resources query: {e}")))?;

        let rows = stmt
            .query_map(params![execution_id.as_str()], |row| {
                let uri_str: String = row.get(0)?;
                let role: String = row.get(1)?;
                let uri = ResourceUri::parse(&uri_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                Ok((uri, role))
            })
            .map_err(|e| CoreError::Internal(format!("Failed to query resources: {e}")))?;

        let mut list = Vec::new();
        for item in rows {
            list.push(item.map_err(|e| CoreError::Internal(e.to_string()))?);
        }
        Ok(list)
    }

    pub fn get_execution_artifacts(
        db: &crate::db::Database,
        execution_id: &ExecutionId,
    ) -> Result<Vec<ResourceUri>, CoreError> {
        let mut stmt = db
            .conn()
            .prepare("SELECT artifact_uri FROM execution_artifacts WHERE execution_id = ?1")
            .map_err(|e| CoreError::Internal(format!("Failed to prepare artifacts query: {e}")))?;

        let rows = stmt
            .query_map(params![execution_id.as_str()], |row| {
                let uri_str: String = row.get(0)?;
                let uri = ResourceUri::parse(&uri_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                Ok(uri)
            })
            .map_err(|e| CoreError::Internal(format!("Failed to query artifacts: {e}")))?;

        let mut list = Vec::new();
        for item in rows {
            list.push(item.map_err(|e| CoreError::Internal(e.to_string()))?);
        }
        Ok(list)
    }
}
