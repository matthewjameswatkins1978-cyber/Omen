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
            "#,
            )
            .map_err(|e| CoreError::Internal(format!("Database migration failed: {e}")))?;

        Ok(())
    }
}
