use chrono::Utc;
use omen_core::{BlobState, CoreError, ResourceUri, RetentionClass};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMetadata {
    pub digest: String,
    pub uri: ResourceUri,
    pub size: u64,
    pub media_type: String,
    pub producer: String,
    pub retention: RetentionClass,
    pub blob_state: BlobState,
    pub created_at: String,
    pub last_accessed_at: String,
}

pub struct ContentAddressedStore {
    root: PathBuf,
}

impl ContentAddressedStore {
    pub fn new(root: PathBuf) -> Self {
        fs::create_dir_all(&root).expect("Failed to create CAS root directory");
        Self { root }
    }

    pub fn blob_path(&self, digest: &str) -> PathBuf {
        let prefix = &digest[..2];
        self.root.join("sha256").join(prefix).join(digest)
    }

    pub fn store(
        &self,
        db: &mut crate::db::Database,
        bytes: &[u8],
        media_type: &str,
        producer: &str,
        retention: RetentionClass,
    ) -> Result<ArtifactMetadata, CoreError> {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hex::encode(hasher.finalize());
        let uri_str = format!("artifact://sha256/{digest}");
        let uri = ResourceUri::parse(&uri_str)?;

        let dest_path = self.blob_path(&digest);
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                CoreError::Internal(format!("Failed to create CAS prefix directory: {e}"))
            })?;
        }

        if !dest_path.exists() {
            let temp_path = dest_path.with_extension(format!("tmp.{}", std::process::id()));
            fs::write(&temp_path, bytes)
                .map_err(|e| CoreError::Internal(format!("Failed to write CAS temp file: {e}")))?;
            fs::rename(&temp_path, &dest_path)
                .map_err(|e| CoreError::Internal(format!("Failed to commit CAS file: {e}")))?;
        }

        let now = Utc::now().to_rfc3339();
        let size = bytes.len() as u64;
        let retention_str = format!("{retention:?}").to_uppercase();
        let blob_state_str = "PRESENT";

        db.conn_mut()
            .execute(
                r#"
                INSERT INTO artifacts (digest, size, media_type, producer, retention, blob_state, created_at, last_accessed_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(digest) DO UPDATE SET
                    last_accessed_at = ?8
                "#,
                params![digest, size, media_type, producer, retention_str, blob_state_str, now, now],
            )
            .map_err(|e| CoreError::Internal(format!("Failed to insert artifact metadata: {e}")))?;

        Ok(ArtifactMetadata {
            digest,
            uri,
            size,
            media_type: media_type.to_string(),
            producer: producer.to_string(),
            retention,
            blob_state: BlobState::Present,
            created_at: now.clone(),
            last_accessed_at: now,
        })
    }

    pub fn inspect(
        &self,
        db: &crate::db::Database,
        digest: &str,
    ) -> Result<ArtifactMetadata, CoreError> {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT digest, size, media_type, producer, retention, blob_state, created_at, last_accessed_at FROM artifacts WHERE digest = ?1",
            )
            .map_err(|e| CoreError::Internal(format!("Failed to prepare inspect query: {e}")))?;

        let metadata = stmt
            .query_row(params![digest], |row| {
                let d: String = row.get(0)?;
                let s: i64 = row.get(1)?;
                let mt: String = row.get(2)?;
                let prod: String = row.get(3)?;
                let ret: String = row.get(4)?;
                let bs: String = row.get(5)?;
                let cr: String = row.get(6)?;
                let la: String = row.get(7)?;

                let retention = match ret.as_str() {
                    "PINNED" => RetentionClass::Pinned,
                    "REFERENCED" => RetentionClass::Referenced,
                    "CACHE" => RetentionClass::Cache,
                    "EPHEMERAL" => RetentionClass::Ephemeral,
                    _ => RetentionClass::Cache,
                };

                let blob_state = match bs.as_str() {
                    "PRESENT" => BlobState::Present,
                    "EVICTED" => BlobState::Evicted,
                    "MISSING" => BlobState::Missing,
                    _ => BlobState::Missing,
                };

                let uri = ResourceUri::parse(&format!("artifact://sha256/{d}")).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;

                Ok(ArtifactMetadata {
                    digest: d,
                    uri,
                    size: s as u64,
                    media_type: mt,
                    producer: prod,
                    retention,
                    blob_state,
                    created_at: cr,
                    last_accessed_at: la,
                })
            })
            .map_err(|_| CoreError::NotFound(format!("Artifact not found: {digest}")))?;

        Ok(metadata)
    }

    pub fn read_slice(
        &self,
        db: &mut crate::db::Database,
        digest: &str,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, CoreError> {
        let meta = self.inspect(db, digest)?;
        if meta.blob_state != BlobState::Present {
            return Err(CoreError::NotFound(format!(
                "Artifact blob is {state:?}: {digest}",
                state = meta.blob_state
            )));
        }

        let blob_path = self.blob_path(digest);
        let mut file = fs::File::open(&blob_path)
            .map_err(|e| CoreError::Internal(format!("Failed to open artifact file: {e}")))?;

        file.seek(SeekFrom::Start(offset))
            .map_err(|e| CoreError::Internal(format!("Failed to seek artifact: {e}")))?;

        let mut buffer = vec![0u8; length as usize];
        let bytes_read = file
            .read(&mut buffer)
            .map_err(|e| CoreError::Internal(format!("Failed to read artifact slice: {e}")))?;
        buffer.truncate(bytes_read);

        // Update last accessed
        let now = Utc::now().to_rfc3339();
        let _ = db.conn_mut().execute(
            "UPDATE artifacts SET last_accessed_at = ?1 WHERE digest = ?2",
            params![now, digest],
        );

        Ok(buffer)
    }

    pub fn gc(&self, db: &mut crate::db::Database, dry_run: bool) -> Result<GcReport, CoreError> {
        let to_reclaim: Vec<(String, u64)> = {
            let mut stmt = db
                .conn()
                .prepare(
                    "SELECT digest, size FROM artifacts WHERE retention = 'EPHEMERAL' AND blob_state = 'PRESENT'",
                )
                .map_err(|e| CoreError::Internal(format!("Failed to prepare GC query: {e}")))?;

            let rows = stmt
                .query_map([], |row| {
                    let d: String = row.get(0)?;
                    let s: i64 = row.get(1)?;
                    Ok((d, s as u64))
                })
                .map_err(|e| CoreError::Internal(format!("Failed to execute GC query: {e}")))?;

            let mut list = Vec::new();
            for item in rows {
                let pair = item.map_err(|e| CoreError::Internal(format!("GC row error: {e}")))?;
                list.push(pair);
            }
            list
        };

        let mut reclaimed_digests = Vec::new();
        let mut reclaimed_bytes = 0u64;

        for (digest, size) in to_reclaim {
            reclaimed_digests.push(digest.clone());
            reclaimed_bytes += size;

            if !dry_run {
                let path = self.blob_path(&digest);
                if path.exists() {
                    let _ = fs::remove_file(path);
                }
                let _ = db.conn_mut().execute(
                    "UPDATE artifacts SET blob_state = 'EVICTED' WHERE digest = ?1",
                    params![digest],
                );
            }
        }

        Ok(GcReport {
            dry_run,
            reclaimed_count: reclaimed_digests.len(),
            reclaimed_bytes,
            reclaimed_digests,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcReport {
    pub dry_run: bool,
    pub reclaimed_count: usize,
    pub reclaimed_bytes: u64,
    pub reclaimed_digests: Vec<String>,
}
