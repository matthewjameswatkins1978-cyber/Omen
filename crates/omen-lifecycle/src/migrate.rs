//! State schema versioning and migration checkpoints (H items 31-33).
//!
//! Requirements: deterministic, versioned, crash-aware, restartable,
//! bounded. No destructive migration runs without a snapshot/recovery
//! strategy. Before a consequential migration, a checkpoint records intent,
//! source, target, snapshot identity, and stage; after a crash, restart
//! recovers truthfully instead of guessing.

use crate::error::LifecycleError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Current Omen local-state schema version. Bumped only with an explicit
/// migration entry in [`MIGRATIONS`]. Machine Contract stays 0.8: lifecycle
/// schema versions are separately versioned (H item 81/82).
pub const STATE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationStep {
    pub from: u32,
    pub to: u32,
    pub description: String,
    /// Whether this migration mutates payload bytes (needs snapshot).
    pub destructive: bool,
}

pub fn migrations() -> Vec<MigrationStep> {
    vec![
        // v0 used the one-shot CREATE batch with no version tracking.
        // v1 introduces user_version tracking + lifecycle checkpoints.
        MigrationStep {
            from: 0,
            to: 1,
            description: "record schema version; create lifecycle_checkpoints".to_string(),
            destructive: false,
        },
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationStage {
    Intended,
    SnapshotTaken,
    Migrating,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationCheckpoint {
    pub schema_version: u32,
    pub migration_id: String,
    pub source_version: u32,
    pub target_version: u32,
    pub snapshot_identity: Option<String>,
    pub stage: MigrationStage,
    pub updated_at: String,
}

pub fn checkpoint_path(base: &Path) -> PathBuf {
    base.join("update").join("migration-checkpoint.json")
}

pub fn load_checkpoint(base: &Path) -> Result<Option<MigrationCheckpoint>, LifecycleError> {
    let path = checkpoint_path(base);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).map_err(|e| LifecycleError::Io(e.to_string()))?;
    let cp: MigrationCheckpoint =
        serde_json::from_slice(&bytes).map_err(|e| LifecycleError::Manifest(e.to_string()))?;
    Ok(Some(cp))
}

pub fn save_checkpoint(base: &Path, cp: &MigrationCheckpoint) -> Result<(), LifecycleError> {
    let path = checkpoint_path(base);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LifecycleError::Io(e.to_string()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(cp).unwrap())
        .map_err(|e| LifecycleError::Io(e.to_string()))?;
    std::fs::rename(&tmp, &path).map_err(|e| LifecycleError::Io(e.to_string()))?;
    Ok(())
}

pub fn clear_checkpoint(base: &Path) -> Result<(), LifecycleError> {
    let path = checkpoint_path(base);
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| LifecycleError::Io(e.to_string()))?;
    }
    Ok(())
}

/// Classify restart state after a possible crash during migration.
/// Never guesses: an unfinished checkpoint without a snapshot is
/// `RecoveryRequired`, not silently resumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartState {
    Clean,
    MigrationIncomplete { checkpoint: MigrationCheckpoint },
    RecoveryRequired { reason: String },
}

pub fn classify_restart(base: &Path) -> Result<RestartState, LifecycleError> {
    match load_checkpoint(base)? {
        None => Ok(RestartState::Clean),
        Some(cp) => match cp.stage {
            MigrationStage::Completed => Ok(RestartState::Clean),
            MigrationStage::Failed => Ok(RestartState::RecoveryRequired {
                reason: format!(
                    "migration {} failed; snapshot {:?}",
                    cp.migration_id, cp.snapshot_identity
                ),
            }),
            _ if cp.snapshot_identity.is_none()
                && migrations()
                    .iter()
                    .any(|m| m.from == cp.source_version && m.destructive) =>
            {
                Ok(RestartState::RecoveryRequired {
                    reason: "destructive migration interrupted before snapshot".to_string(),
                })
            }
            _ => Ok(RestartState::MigrationIncomplete { checkpoint: cp }),
        },
    }
}

/// Ensure a workspace SQLite DB carries the current schema version.
/// Non-destructive, idempotent, bounded: creates the checkpoints table and
/// stamps `user_version`. Returns the version found before ensuring.
pub fn ensure_workspace_schema(db_path: &Path) -> Result<u32, LifecycleError> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| LifecycleError::Io(e.to_string()))?;
    }
    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| LifecycleError::Migrate(format!("open db: {e}")))?;
    let found: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| LifecycleError::Migrate(e.to_string()))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS lifecycle_checkpoints (
            migration_id TEXT PRIMARY KEY,
            source_version INTEGER NOT NULL,
            target_version INTEGER NOT NULL,
            stage TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );",
    )
    .map_err(|e| LifecycleError::Migrate(e.to_string()))?;
    if found < STATE_SCHEMA_VERSION {
        conn.pragma_update(None, "user_version", STATE_SCHEMA_VERSION)
            .map_err(|e| LifecycleError::Migrate(e.to_string()))?;
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_restart_with_no_checkpoint() {
        let base = tempfile::tempdir().unwrap();
        assert_eq!(classify_restart(base.path()).unwrap(), RestartState::Clean);
    }

    #[test]
    fn interrupted_migration_is_explicit() {
        let base = tempfile::tempdir().unwrap();
        save_checkpoint(
            base.path(),
            &MigrationCheckpoint {
                schema_version: 1,
                migration_id: "m1".to_string(),
                source_version: 0,
                target_version: 1,
                snapshot_identity: Some("snap:abc".to_string()),
                stage: MigrationStage::Migrating,
                updated_at: "now".to_string(),
            },
        )
        .unwrap();
        match classify_restart(base.path()).unwrap() {
            RestartState::MigrationIncomplete { checkpoint } => {
                assert_eq!(checkpoint.migration_id, "m1");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn schema_ensure_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.sqlite");
        let found = ensure_workspace_schema(&db).unwrap();
        assert_eq!(found, 0);
        let found2 = ensure_workspace_schema(&db).unwrap();
        assert_eq!(found2, STATE_SCHEMA_VERSION);
    }
}
