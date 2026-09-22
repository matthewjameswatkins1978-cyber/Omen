use omen_core::CoreError;
use rusqlite::{Connection, OpenFlags};
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

    /// Opens an existing SQLite database without creating or migrating state.
    ///
    /// E2 read-after-write contract: the reader MUST observe every committed
    /// transaction, including frames still sitting in the WAL of a live
    /// writer (the daemon holds its connection open for its whole lifetime).
    /// Therefore this intentionally does NOT use `immutable=1` (which would
    /// freeze the reader at whatever was checkpointed into the main file and
    /// hide the daemon's un-checkpointed commits until daemon exit).
    ///
    /// Read-only + WAL is concurrency-safe: the reader takes SHARED locks and
    /// reads committed WAL frames; it never writes, migrates, or creates
    /// files. A bounded `busy_timeout` lets the reader ride out a writer's
    /// checkpoint instead of failing instantly; expiry surfaces as an error
    /// (control returns to the caller), never a hang.
    pub fn open_read_only(path: &Path) -> Result<Self, CoreError> {
        if !path.is_file() {
            return Err(CoreError::Internal(format!(
                "SQLite database does not exist: {}",
                path.display()
            )));
        }

        let normalized = path
            .canonicalize()
            .map_err(|e| CoreError::Internal(format!("Failed to resolve SQLite database: {e}")))?
            .to_string_lossy()
            .replace('\\', "/");
        let normalized = normalized.strip_prefix("//?/").unwrap_or(&normalized);
        let uri = if normalized.as_bytes().get(1) == Some(&b':') {
            format!("file:///{normalized}")
        } else {
            format!("file://{normalized}")
        };
        let conn = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| {
            CoreError::Internal(format!("Failed to open SQLite database read-only: {e}"))
        })?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| {
                CoreError::Internal(format!("Failed to set read-only busy timeout: {e}"))
            })?;
        Ok(Self { conn })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_open_uses_existing_database() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.sqlite");
        {
            let db = Database::open(&path).unwrap();
            db.conn()
                .execute("INSERT INTO resource_generations (name, generation) VALUES ('fs:workspace', 7)", [])
                .unwrap();
        }
        let db = Database::open_read_only(&path).unwrap();
        let generation: i64 = db
            .conn()
            .query_row(
                "SELECT generation FROM resource_generations WHERE name = 'fs:workspace'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generation, 7);
    }

    #[test]
    fn read_only_reader_sees_live_writer_commits() {
        // E2.1: the daemon writer connection stays open for its whole life
        // with commits sitting in WAL. The read-only path must see them.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.sqlite");
        let writer = Database::open(&path).unwrap();
        writer
            .conn()
            .execute(
                "INSERT INTO resource_generations (name, generation) VALUES ('fs:workspace', 7)",
                [],
            )
            .unwrap();
        let reader = Database::open_read_only(&path).unwrap();
        let seen: i64 = reader
            .conn()
            .query_row(
                "SELECT generation FROM resource_generations WHERE name = 'fs:workspace'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(seen, 7);
        writer
            .conn()
            .execute(
                "UPDATE resource_generations SET generation = 8 WHERE name = 'fs:workspace'",
                [],
            )
            .unwrap();
        let reader2 = Database::open_read_only(&path).unwrap();
        let seen2: i64 = reader2
            .conn()
            .query_row(
                "SELECT generation FROM resource_generations WHERE name = 'fs:workspace'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(seen2, 8);
    }
}
