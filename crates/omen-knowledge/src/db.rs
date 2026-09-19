use omen_core::CoreError;
use rusqlite::Connection;
use std::path::Path;

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, CoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Internal(format!("Failed to create DB directory: {e}")))?;
        }

        let conn = Connection::open(path)
            .map_err(|e| CoreError::Internal(format!("Failed to open SQLite database: {e}")))?;

        // WAL mode and durability pragmas
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| CoreError::Internal(format!("Failed to set WAL mode: {e}")))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| CoreError::Internal(format!("Failed to set synchronous pragma: {e}")))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| CoreError::Internal(format!("Failed to enable foreign keys: {e}")))?;

        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self, CoreError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| CoreError::Internal(format!("Failed to open in-memory DB: {e}")))?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    fn migrate(&self) -> Result<(), CoreError> {
        self.conn
            .execute_batch(
                r#"
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY
            );

            CREATE TABLE IF NOT EXISTS resource_generations (
                name TEXT PRIMARY KEY,
                generation INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS facts (
                fact_id TEXT PRIMARY KEY,
                resource_uri TEXT NOT NULL,
                value TEXT NOT NULL,
                validity TEXT NOT NULL,
                assurance TEXT NOT NULL,
                producer TEXT NOT NULL,
                witness TEXT,
                superseded_by TEXT,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_facts_resource ON facts(resource_uri);
            CREATE INDEX IF NOT EXISTS idx_facts_validity ON facts(validity);

            CREATE TABLE IF NOT EXISTS fact_dependencies (
                fact_id TEXT NOT NULL,
                generation_name TEXT NOT NULL,
                recorded_generation INTEGER NOT NULL,
                PRIMARY KEY (fact_id, generation_name),
                FOREIGN KEY (fact_id) REFERENCES facts(fact_id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS artifacts (
                digest TEXT PRIMARY KEY,
                size INTEGER NOT NULL,
                media_type TEXT NOT NULL,
                producer TEXT NOT NULL,
                retention TEXT NOT NULL,
                blob_state TEXT NOT NULL,
                created_at TEXT NOT NULL,
                last_accessed_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS fact_artifacts (
                fact_id TEXT NOT NULL,
                artifact_uri TEXT NOT NULL,
                PRIMARY KEY (fact_id, artifact_uri),
                FOREIGN KEY (fact_id) REFERENCES facts(fact_id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS interactive_sessions (
                session_id TEXT PRIMARY KEY,
                actor TEXT NOT NULL,
                created_at TEXT NOT NULL,
                last_active TEXT NOT NULL,
                cwd TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS execution_history (
                execution_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                command TEXT NOT NULL,
                exit_code INTEGER,
                duration_ms INTEGER,
                stdout_artifact TEXT,
                stderr_artifact TEXT,
                envelope_json TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES interactive_sessions(session_id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_execution_session ON execution_history(session_id, created_at DESC);

            CREATE TABLE IF NOT EXISTS execution_resources (
                execution_id TEXT NOT NULL,
                resource_uri TEXT NOT NULL,
                role TEXT NOT NULL,
                PRIMARY KEY (execution_id, resource_uri, role),
                FOREIGN KEY (execution_id) REFERENCES execution_history(execution_id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS execution_facts (
                execution_id TEXT NOT NULL,
                fact_id TEXT NOT NULL,
                PRIMARY KEY (execution_id, fact_id),
                FOREIGN KEY (execution_id) REFERENCES execution_history(execution_id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS execution_artifacts (
                execution_id TEXT NOT NULL,
                artifact_uri TEXT NOT NULL,
                PRIMARY KEY (execution_id, artifact_uri),
                FOREIGN KEY (execution_id) REFERENCES execution_history(execution_id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS daemon_workspaces (
                workspace_id TEXT PRIMARY KEY,
                canonical_path TEXT NOT NULL UNIQUE,
                epoch INTEGER NOT NULL,
                attached_at TEXT NOT NULL,
                last_seen TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS service_records (
                workspace_id TEXT NOT NULL,
                name TEXT NOT NULL,
                command TEXT NOT NULL,
                pid INTEGER,
                state TEXT NOT NULL,
                started_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (workspace_id, name)
            );

            CREATE TABLE IF NOT EXISTS request_receipts (
                consequential_request_id TEXT PRIMARY KEY,
                execution_id TEXT,
                status TEXT NOT NULL,
                recorded_at TEXT NOT NULL
            );
            "#,
            )
            .map_err(|e| CoreError::Internal(format!("Database migration failed: {e}")))?;

        Ok(())
    }
}
