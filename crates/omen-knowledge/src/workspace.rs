use chrono::Utc;
use omen_core::CoreError;
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::db::Database;

/// Computes a deterministic workspace ID from the canonical root path.
pub fn deterministic_workspace_id(root: &Path) -> String {
    let canonical = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/");
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let hex_str = hex::encode(hasher.finalize());
    format!("ws_{}", &hex_str[..16])
}

/// Resolves the Omen state directory for a given workspace.
pub fn resolve_workspace_dir(root: &Path) -> PathBuf {
    let base_dir = if let Ok(custom) = env::var("OMEN_STATE_HOME") {
        PathBuf::from(custom)
    } else if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        PathBuf::from(local_app_data).join("Omen")
    } else if let Ok(home) = env::var("HOME") {
        PathBuf::from(home).join(".omen")
    } else {
        PathBuf::from(".omen-state")
    };

    let ws_id = deterministic_workspace_id(root);
    let ws_dir = base_dir.join("workspaces").join(ws_id);
    fs::create_dir_all(&ws_dir).expect("Failed to create workspace state directory");
    ws_dir
}

/// Canonical database file name used by all Omen runtimes for a workspace.
pub const CANONICAL_DB_FILE_NAME: &str = "state.sqlite";

/// Resolves the canonical database file path for a workspace.
pub fn canonical_workspace_db_path(root: &Path) -> PathBuf {
    resolve_workspace_dir(root).join(CANONICAL_DB_FILE_NAME)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub workspace_id: String,
    pub canonical_path: String,
    pub epoch: u64,
    pub attached_at: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRecord {
    pub workspace_id: String,
    pub name: String,
    pub command: String,
    pub pid: Option<u32>,
    pub state: String,
    pub started_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestReceiptRecord {
    pub consequential_request_id: String,
    pub execution_id: Option<String>,
    pub status: String,
    pub recorded_at: String,
}

pub struct WorkspacePersistence;

impl WorkspacePersistence {
    pub fn upsert_workspace(
        db: &Database,
        workspace_id: &str,
        canonical_path: &str,
        epoch: u64,
    ) -> Result<(), CoreError> {
        let now = Utc::now().to_rfc3339();
        db.conn()
            .execute(
                r#"
                INSERT INTO daemon_workspaces (workspace_id, canonical_path, epoch, attached_at, last_seen)
                VALUES (?1, ?2, ?3, ?4, ?4)
                ON CONFLICT(workspace_id) DO UPDATE SET
                    canonical_path = excluded.canonical_path,
                    epoch = excluded.epoch,
                    last_seen = excluded.last_seen
                "#,
                params![workspace_id, canonical_path, epoch as i64, now],
            )
            .map_err(|e| CoreError::Internal(format!("Failed to upsert workspace record: {e}")))?;
        Ok(())
    }

    pub fn get_workspace(
        db: &Database,
        workspace_id: &str,
    ) -> Result<Option<WorkspaceRecord>, CoreError> {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT workspace_id, canonical_path, epoch, attached_at, last_seen FROM daemon_workspaces WHERE workspace_id = ?1",
            )
            .map_err(|e| CoreError::Internal(format!("Failed to prepare workspace query: {e}")))?;

        let mut rows = stmt
            .query(params![workspace_id])
            .map_err(|e| CoreError::Internal(format!("Failed to execute workspace query: {e}")))?;

        if let Some(row) = rows
            .next()
            .map_err(|e| CoreError::Internal(format!("Failed to read workspace row: {e}")))?
        {
            let epoch: i64 = row
                .get(2)
                .map_err(|e| CoreError::Internal(format!("Failed to get epoch: {e}")))?;
            Ok(Some(WorkspaceRecord {
                workspace_id: row
                    .get(0)
                    .map_err(|e| CoreError::Internal(format!("Failed to get workspace_id: {e}")))?,
                canonical_path: row.get(1).map_err(|e| {
                    CoreError::Internal(format!("Failed to get canonical_path: {e}"))
                })?,
                epoch: epoch as u64,
                attached_at: row
                    .get(3)
                    .map_err(|e| CoreError::Internal(format!("Failed to get attached_at: {e}")))?,
                last_seen: row
                    .get(4)
                    .map_err(|e| CoreError::Internal(format!("Failed to get last_seen: {e}")))?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn list_workspaces(db: &Database) -> Result<Vec<WorkspaceRecord>, CoreError> {
        let mut stmt = db
            .conn()
            .prepare("SELECT workspace_id, canonical_path, epoch, attached_at, last_seen FROM daemon_workspaces ORDER BY last_seen DESC")
            .map_err(|e| CoreError::Internal(format!("Failed to prepare list workspaces: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                let epoch: i64 = row.get(2)?;
                Ok(WorkspaceRecord {
                    workspace_id: row.get(0)?,
                    canonical_path: row.get(1)?,
                    epoch: epoch as u64,
                    attached_at: row.get(3)?,
                    last_seen: row.get(4)?,
                })
            })
            .map_err(|e| CoreError::Internal(format!("Failed to query workspaces: {e}")))?;

        let mut results = Vec::new();
        for r in rows {
            results
                .push(r.map_err(|e| CoreError::Internal(format!("Error reading workspace: {e}")))?);
        }
        Ok(results)
    }

    pub fn upsert_service(db: &Database, record: &ServiceRecord) -> Result<(), CoreError> {
        let now = Utc::now().to_rfc3339();
        db.conn()
            .execute(
                r#"
                INSERT INTO service_records (workspace_id, name, command, pid, state, started_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                ON CONFLICT(workspace_id, name) DO UPDATE SET
                    command = excluded.command,
                    pid = excluded.pid,
                    state = excluded.state,
                    updated_at = excluded.updated_at
                "#,
                params![
                    record.workspace_id,
                    record.name,
                    record.command,
                    record.pid.map(|p| p as i64),
                    record.state,
                    now
                ],
            )
            .map_err(|e| CoreError::Internal(format!("Failed to upsert service record: {e}")))?;
        Ok(())
    }

    pub fn get_service(
        db: &Database,
        workspace_id: &str,
        name: &str,
    ) -> Result<Option<ServiceRecord>, CoreError> {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT workspace_id, name, command, pid, state, started_at, updated_at FROM service_records WHERE workspace_id = ?1 AND name = ?2",
            )
            .map_err(|e| CoreError::Internal(format!("Failed to prepare service query: {e}")))?;

        let mut rows = stmt
            .query(params![workspace_id, name])
            .map_err(|e| CoreError::Internal(format!("Failed to query service: {e}")))?;

        if let Some(row) = rows
            .next()
            .map_err(|e| CoreError::Internal(format!("Failed to read service row: {e}")))?
        {
            let pid_i64: Option<i64> = row
                .get(3)
                .map_err(|e| CoreError::Internal(format!("Failed to get pid: {e}")))?;
            Ok(Some(ServiceRecord {
                workspace_id: row
                    .get(0)
                    .map_err(|e| CoreError::Internal(format!("Failed to get workspace_id: {e}")))?,
                name: row
                    .get(1)
                    .map_err(|e| CoreError::Internal(format!("Failed to get name: {e}")))?,
                command: row
                    .get(2)
                    .map_err(|e| CoreError::Internal(format!("Failed to get command: {e}")))?,
                pid: pid_i64.map(|p| p as u32),
                state: row
                    .get(4)
                    .map_err(|e| CoreError::Internal(format!("Failed to get state: {e}")))?,
                started_at: row
                    .get(5)
                    .map_err(|e| CoreError::Internal(format!("Failed to get started_at: {e}")))?,
                updated_at: row
                    .get(6)
                    .map_err(|e| CoreError::Internal(format!("Failed to get updated_at: {e}")))?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn list_services(
        db: &Database,
        workspace_id: &str,
    ) -> Result<Vec<ServiceRecord>, CoreError> {
        let mut stmt = db
            .conn()
            .prepare("SELECT workspace_id, name, command, pid, state, started_at, updated_at FROM service_records WHERE workspace_id = ?1 ORDER BY name ASC")
            .map_err(|e| CoreError::Internal(format!("Failed to prepare list services: {e}")))?;

        let rows = stmt
            .query_map(params![workspace_id], |row| {
                let pid_i64: Option<i64> = row.get(3)?;
                Ok(ServiceRecord {
                    workspace_id: row.get(0)?,
                    name: row.get(1)?,
                    command: row.get(2)?,
                    pid: pid_i64.map(|p| p as u32),
                    state: row.get(4)?,
                    started_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })
            .map_err(|e| CoreError::Internal(format!("Failed to query services: {e}")))?;

        let mut results = Vec::new();
        for r in rows {
            results
                .push(r.map_err(|e| CoreError::Internal(format!("Error reading service: {e}")))?);
        }
        Ok(results)
    }

    pub fn record_request_receipt(
        db: &Database,
        receipt: &RequestReceiptRecord,
    ) -> Result<(), CoreError> {
        let now = Utc::now().to_rfc3339();
        db.conn()
            .execute(
                r#"
                INSERT INTO request_receipts (consequential_request_id, execution_id, status, recorded_at)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(consequential_request_id) DO UPDATE SET
                    execution_id = excluded.execution_id,
                    status = excluded.status,
                    recorded_at = excluded.recorded_at
                "#,
                params![
                    receipt.consequential_request_id,
                    receipt.execution_id,
                    receipt.status,
                    now
                ],
            )
            .map_err(|e| CoreError::Internal(format!("Failed to record request receipt: {e}")))?;
        Ok(())
    }

    pub fn get_request_receipt(
        db: &Database,
        consequential_request_id: &str,
    ) -> Result<Option<RequestReceiptRecord>, CoreError> {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT consequential_request_id, execution_id, status, recorded_at FROM request_receipts WHERE consequential_request_id = ?1",
            )
            .map_err(|e| CoreError::Internal(format!("Failed to prepare receipt query: {e}")))?;

        let mut rows = stmt
            .query(params![consequential_request_id])
            .map_err(|e| CoreError::Internal(format!("Failed to query receipt: {e}")))?;

        if let Some(row) = rows
            .next()
            .map_err(|e| CoreError::Internal(format!("Failed to read receipt row: {e}")))?
        {
            Ok(Some(RequestReceiptRecord {
                consequential_request_id: row
                    .get(0)
                    .map_err(|e| CoreError::Internal(format!("Failed to get receipt id: {e}")))?,
                execution_id: row
                    .get(1)
                    .map_err(|e| CoreError::Internal(format!("Failed to get execution id: {e}")))?,
                status: row
                    .get(2)
                    .map_err(|e| CoreError::Internal(format!("Failed to get status: {e}")))?,
                recorded_at: row
                    .get(3)
                    .map_err(|e| CoreError::Internal(format!("Failed to get recorded_at: {e}")))?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn reconcile_running_receipts(db: &Database) -> Result<usize, CoreError> {
        db.conn()
            .execute(
                r#"
                UPDATE request_receipts
                SET status = 'Unknown'
                WHERE status = 'Running'
                "#,
                [],
            )
            .map_err(|e| CoreError::Internal(format!("Failed to reconcile running receipts: {e}")))
    }
}
