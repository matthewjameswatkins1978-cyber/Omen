use chrono::Utc;
use omen_core::{CoreError, ExecutionId, FactId, InteractiveSessionId, ResourceUri};
use rusqlite::params;
use serde::{Deserialize, Serialize};

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
