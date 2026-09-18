use chrono::Utc;
use omen_core::{Assurance, CoreError, FactId, ResourceUri, ValidityState};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactRecord {
    pub fact_id: FactId,
    pub resource_uri: ResourceUri,
    pub value: String,
    pub validity: ValidityState,
    pub assurance: Assurance,
    pub producer: String,
    pub witness: Option<String>,
    pub superseded_by: Option<FactId>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyRecord {
    pub generation_name: String,
    pub recorded_generation: i64,
    pub current_generation: i64,
    pub is_dirty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactProvenance {
    pub fact: FactRecord,
    pub dependencies: Vec<DependencyRecord>,
    pub artifacts: Vec<ResourceUri>,
    pub history: Vec<FactRecord>,
}

pub struct PublishFactRequest<'a> {
    pub resource: &'a ResourceUri,
    pub value: &'a str,
    pub assurance: Assurance,
    pub producer: &'a str,
    pub witness: Option<&'a str>,
    pub dependencies: &'a [(String, i64)],
    pub artifacts: &'a [ResourceUri],
}

pub struct FactRegistry;

impl FactRegistry {
    pub fn get_generation(db: &crate::db::Database, name: &str) -> Result<i64, CoreError> {
        let mut stmt = db
            .conn()
            .prepare("SELECT generation FROM resource_generations WHERE name = ?1")
            .map_err(|e| CoreError::Internal(format!("Failed to prepare generation query: {e}")))?;

        let generation_val = stmt.query_row(params![name], |row| row.get(0)).unwrap_or(0);
        Ok(generation_val)
    }

    pub fn set_generation(
        db: &mut crate::db::Database,
        name: &str,
        generation: i64,
    ) -> Result<(), CoreError> {
        db.conn_mut()
            .execute(
                r#"
                INSERT INTO resource_generations (name, generation)
                VALUES (?1, ?2)
                ON CONFLICT(name) DO UPDATE SET generation = ?2
                "#,
                params![name, generation],
            )
            .map_err(|e| CoreError::Internal(format!("Failed to set generation: {e}")))?;

        Self::propagate_dirty(db, name, generation)?;
        Ok(())
    }

    pub fn increment_generation(
        db: &mut crate::db::Database,
        name: &str,
    ) -> Result<i64, CoreError> {
        let current = Self::get_generation(db, name)?;
        let next = current + 1;
        Self::set_generation(db, name, next)?;
        Ok(next)
    }

    fn propagate_dirty(
        db: &mut crate::db::Database,
        name: &str,
        current_generation: i64,
    ) -> Result<(), CoreError> {
        // Dirty propagation: Any CURRENT fact whose recorded generation < current_generation transitions to DIRTY
        let sql = r#"
            UPDATE facts
            SET validity = 'DIRTY'
            WHERE validity = 'CURRENT'
              AND fact_id IN (
                  SELECT fact_id FROM fact_dependencies
                  WHERE generation_name = ?1 AND recorded_generation < ?2
              )
        "#;
        db.conn_mut()
            .execute(sql, params![name, current_generation])
            .map_err(|e| CoreError::Internal(format!("Dirty propagation failed: {e}")))?;
        Ok(())
    }

    pub fn publish_fact(
        db: &mut crate::db::Database,
        req: PublishFactRequest<'_>,
    ) -> Result<FactRecord, CoreError> {
        let now = Utc::now().to_rfc3339();

        let mut hasher = Sha256::new();
        hasher.update(req.resource.as_str().as_bytes());
        hasher.update(req.value.as_bytes());
        hasher.update(now.as_bytes());
        let hash = hex::encode(hasher.finalize());
        let fact_id = FactId::new(format!("fact-{}", &hash[..16]))?;

        let assurance_str = format!("{:?}", req.assurance).to_uppercase();

        let tx = db
            .conn_mut()
            .transaction()
            .map_err(|e| CoreError::Internal(format!("Failed to begin transaction: {e}")))?;

        // Supersede any active (CURRENT or DIRTY) fact for this resource URI
        tx.execute(
            r#"
            UPDATE facts
            SET validity = 'SUPERSEDED', superseded_by = ?1
            WHERE resource_uri = ?2 AND (validity = 'CURRENT' OR validity = 'DIRTY')
            "#,
            params![fact_id.as_str(), req.resource.as_str()],
        )
        .map_err(|e| CoreError::Internal(format!("Failed to supersede existing facts: {e}")))?;

        // Insert new fact as CURRENT
        tx.execute(
            r#"
            INSERT INTO facts (fact_id, resource_uri, value, validity, assurance, producer, witness, superseded_by, created_at)
            VALUES (?1, ?2, ?3, 'CURRENT', ?4, ?5, ?6, NULL, ?7)
            "#,
            params![
                fact_id.as_str(),
                req.resource.as_str(),
                req.value,
                assurance_str,
                req.producer,
                req.witness,
                now
            ],
        )
        .map_err(|e| CoreError::Internal(format!("Failed to insert fact: {e}")))?;

        // Record dependencies
        for (gen_name, recorded_gen) in req.dependencies {
            tx.execute(
                r#"
                INSERT INTO fact_dependencies (fact_id, generation_name, recorded_generation)
                VALUES (?1, ?2, ?3)
                "#,
                params![fact_id.as_str(), gen_name, recorded_gen],
            )
            .map_err(|e| CoreError::Internal(format!("Failed to insert fact dependency: {e}")))?;
        }

        // Record artifacts
        for art in req.artifacts {
            tx.execute(
                r#"
                INSERT INTO fact_artifacts (fact_id, artifact_uri)
                VALUES (?1, ?2)
                "#,
                params![fact_id.as_str(), art.as_str()],
            )
            .map_err(|e| CoreError::Internal(format!("Failed to link fact artifact: {e}")))?;
        }

        tx.commit()
            .map_err(|e| CoreError::Internal(format!("Failed to commit fact publish: {e}")))?;

        Ok(FactRecord {
            fact_id,
            resource_uri: req.resource.clone(),
            value: req.value.to_string(),
            validity: ValidityState::Current,
            assurance: req.assurance,
            producer: req.producer.to_string(),
            witness: req.witness.map(Into::into),
            superseded_by: None,
            created_at: now,
        })
    }

    pub fn get_fact(
        db: &crate::db::Database,
        resource: &ResourceUri,
        require_current: bool,
    ) -> Result<FactRecord, CoreError> {
        let mut stmt = db
            .conn()
            .prepare(
                r#"
                SELECT fact_id, resource_uri, value, validity, assurance, producer, witness, superseded_by, created_at
                FROM facts
                WHERE resource_uri = ?1 AND (validity = 'CURRENT' OR validity = 'DIRTY')
                ORDER BY created_at DESC
                LIMIT 1
                "#,
            )
            .map_err(|e| CoreError::Internal(format!("Failed to prepare get_fact query: {e}")))?;

        let fact = stmt
            .query_row(params![resource.as_str()], |row| {
                let fid: String = row.get(0)?;
                let ruri: String = row.get(1)?;
                let val: String = row.get(2)?;
                let val_state: String = row.get(3)?;
                let ass: String = row.get(4)?;
                let prod: String = row.get(5)?;
                let wit: Option<String> = row.get(6)?;
                let sby: Option<String> = row.get(7)?;
                let cr: String = row.get(8)?;

                let validity = match val_state.as_str() {
                    "CURRENT" => ValidityState::Current,
                    "DIRTY" => ValidityState::Dirty,
                    "STALE" => ValidityState::Stale,
                    "SUPERSEDED" => ValidityState::Superseded,
                    _ => ValidityState::Historical,
                };

                let assurance = match ass.as_str() {
                    "DETERMINISTIC" => Assurance::Deterministic,
                    "VERIFIED" => Assurance::Verified,
                    "ENFORCED" => Assurance::Enforced,
                    "OBSERVED" => Assurance::Observed,
                    "CLAIMED" => Assurance::Claimed,
                    "INFERRED" => Assurance::Inferred,
                    _ => Assurance::Unknown,
                };

                let fact_id = FactId::new(fid).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;

                let resource_uri = ResourceUri::parse(&ruri).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;

                let superseded_by = match sby {
                    Some(s) => Some(FactId::new(s).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            7,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?),
                    None => None,
                };

                Ok(FactRecord {
                    fact_id,
                    resource_uri,
                    value: val,
                    validity,
                    assurance,
                    producer: prod,
                    witness: wit,
                    superseded_by,
                    created_at: cr,
                })
            })
            .map_err(|_| {
                CoreError::NotFound(format!("No active fact found for {}", resource.as_str()))
            })?;

        if require_current && fact.validity == ValidityState::Dirty {
            return Err(CoreError::FactDirty(format!(
                "Fact {} is DIRTY: witness generation changed; explicit revalidation required",
                resource.as_str()
            )));
        }

        Ok(fact)
    }

    pub fn why_fact(
        db: &crate::db::Database,
        resource: &ResourceUri,
    ) -> Result<FactProvenance, CoreError> {
        let current_fact = Self::get_fact(db, resource, false)?;

        // Fetch dependencies
        let mut dep_stmt = db
            .conn()
            .prepare(
                "SELECT generation_name, recorded_generation FROM fact_dependencies WHERE fact_id = ?1",
            )
            .map_err(|e| CoreError::Internal(format!("Failed to prepare dep query: {e}")))?;

        let dep_rows = dep_stmt
            .query_map(params![current_fact.fact_id.as_str()], |row| {
                let name: String = row.get(0)?;
                let rec_gen: i64 = row.get(1)?;
                Ok((name, rec_gen))
            })
            .map_err(|e| CoreError::Internal(format!("Failed to query dependencies: {e}")))?;

        let mut dependencies = Vec::new();
        for item in dep_rows {
            let (name, rec_gen) = item.map_err(|e| CoreError::Internal(e.to_string()))?;
            let cur_gen = Self::get_generation(db, &name)?;
            dependencies.push(DependencyRecord {
                generation_name: name,
                recorded_generation: rec_gen,
                current_generation: cur_gen,
                is_dirty: rec_gen < cur_gen,
            });
        }

        // Fetch artifacts
        let mut art_stmt = db
            .conn()
            .prepare("SELECT artifact_uri FROM fact_artifacts WHERE fact_id = ?1")
            .map_err(|e| CoreError::Internal(format!("Failed to prepare art query: {e}")))?;

        let art_rows = art_stmt
            .query_map(params![current_fact.fact_id.as_str()], |row| {
                let uri: String = row.get(0)?;
                Ok(uri)
            })
            .map_err(|e| CoreError::Internal(format!("Failed to query artifacts: {e}")))?;

        let mut artifacts = Vec::new();
        for item in art_rows {
            let u = item.map_err(|e| CoreError::Internal(e.to_string()))?;
            artifacts.push(ResourceUri::parse(&u)?);
        }

        // Fetch historical superseded facts
        let mut hist_stmt = db
            .conn()
            .prepare(
                r#"
                SELECT fact_id, resource_uri, value, validity, assurance, producer, witness, superseded_by, created_at
                FROM facts
                WHERE resource_uri = ?1 AND validity = 'SUPERSEDED'
                ORDER BY created_at DESC
                "#,
            )
            .map_err(|e| CoreError::Internal(format!("Failed to prepare history query: {e}")))?;

        let hist_rows = hist_stmt
            .query_map(params![resource.as_str()], |row| {
                let fid: String = row.get(0)?;
                let ruri: String = row.get(1)?;
                let val: String = row.get(2)?;
                let val_state: String = row.get(3)?;
                let ass: String = row.get(4)?;
                let prod: String = row.get(5)?;
                let wit: Option<String> = row.get(6)?;
                let sby: Option<String> = row.get(7)?;
                let cr: String = row.get(8)?;

                let validity = match val_state.as_str() {
                    "SUPERSEDED" => ValidityState::Superseded,
                    _ => ValidityState::Historical,
                };

                let assurance = match ass.as_str() {
                    "DETERMINISTIC" => Assurance::Deterministic,
                    "VERIFIED" => Assurance::Verified,
                    "ENFORCED" => Assurance::Enforced,
                    "OBSERVED" => Assurance::Observed,
                    "CLAIMED" => Assurance::Claimed,
                    "INFERRED" => Assurance::Inferred,
                    _ => Assurance::Unknown,
                };

                let fact_id = FactId::new(fid).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;

                let resource_uri = ResourceUri::parse(&ruri).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;

                let superseded_by = match sby {
                    Some(s) => Some(FactId::new(s).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            7,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?),
                    None => None,
                };

                Ok(FactRecord {
                    fact_id,
                    resource_uri,
                    value: val,
                    validity,
                    assurance,
                    producer: prod,
                    witness: wit,
                    superseded_by,
                    created_at: cr,
                })
            })
            .map_err(|e| CoreError::Internal(format!("Failed to query history: {e}")))?;

        let mut history = Vec::new();
        for item in hist_rows {
            history.push(item.map_err(|e| CoreError::Internal(e.to_string()))?);
        }

        Ok(FactProvenance {
            fact: current_fact,
            dependencies,
            artifacts,
            history,
        })
    }

    pub fn list_active_facts(
        db: &crate::db::Database,
        limit: usize,
    ) -> Result<Vec<FactRecord>, CoreError> {
        let mut stmt = db
            .conn()
            .prepare(
                r#"
                SELECT fact_id, resource_uri, value, validity, assurance, producer, witness, superseded_by, created_at
                FROM facts
                WHERE validity = 'CURRENT' OR validity = 'DIRTY'
                ORDER BY created_at DESC
                LIMIT ?1
                "#,
            )
            .map_err(|e| CoreError::Internal(format!("Failed to prepare list_active_facts: {e}")))?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                let fid: String = row.get(0)?;
                let ruri: String = row.get(1)?;
                let val: String = row.get(2)?;
                let val_state: String = row.get(3)?;
                let ass: String = row.get(4)?;
                let prod: String = row.get(5)?;
                let wit: Option<String> = row.get(6)?;
                let sby: Option<String> = row.get(7)?;
                let cr: String = row.get(8)?;

                let validity = match val_state.as_str() {
                    "CURRENT" => ValidityState::Current,
                    "DIRTY" => ValidityState::Dirty,
                    "STALE" => ValidityState::Stale,
                    "SUPERSEDED" => ValidityState::Superseded,
                    _ => ValidityState::Historical,
                };

                let assurance = match ass.as_str() {
                    "DETERMINISTIC" => Assurance::Deterministic,
                    "VERIFIED" => Assurance::Verified,
                    "ENFORCED" => Assurance::Enforced,
                    "OBSERVED" => Assurance::Observed,
                    "CLAIMED" => Assurance::Claimed,
                    "INFERRED" => Assurance::Inferred,
                    _ => Assurance::Unknown,
                };

                let fact_id = FactId::new(fid).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;

                let resource_uri = ResourceUri::parse(&ruri).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;

                let superseded_by = match sby {
                    Some(s) => Some(FactId::new(s).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            7,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?),
                    None => None,
                };

                Ok(FactRecord {
                    fact_id,
                    resource_uri,
                    value: val,
                    validity,
                    assurance,
                    producer: prod,
                    witness: wit,
                    superseded_by,
                    created_at: cr,
                })
            })
            .map_err(|e| CoreError::Internal(format!("Failed to query active facts: {e}")))?;

        let mut facts = Vec::new();
        for item in rows {
            facts.push(item.map_err(|e| CoreError::Internal(e.to_string()))?);
        }
        Ok(facts)
    }
}
