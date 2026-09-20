use omen_core::{CoreError, ResourceUri, SemanticProviderId, SymbolId};
use omen_semantic::provider::{
    BoxFuture, ProviderCapabilities, ProviderKind, SemanticLookupResult, SemanticProvider,
};
use omen_semantic::types::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// SCIP document descriptor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScipDocument {
    pub relative_path: String,
    #[serde(default)]
    pub symbols: Vec<ScipSymbolInformation>,
    #[serde(default)]
    pub occurrences: Vec<ScipOccurrence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScipSymbolInformation {
    pub symbol: String,
    pub documentation: Option<Vec<String>>,
    pub relationships: Option<Vec<ScipRelationship>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScipRelationship {
    pub symbol: String,
    pub is_reference: bool,
    pub is_implementation: bool,
    pub is_type_definition: bool,
    pub is_definition: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScipOccurrence {
    pub range: Vec<usize>, // [start_line, start_col, end_col] or [start_line, start_col, end_line, end_col]
    pub symbol: String,
    #[serde(default)]
    pub symbol_roles: usize, // 1 = definition, 0 = reference
    #[serde(default)]
    pub syntax_kind: Option<usize>,
}

/// Canonical SCIP index container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScipIndex {
    pub documents: Vec<ScipDocument>,
}

/// SCIP indexed semantic provider with cryptographic freshness verification.
pub struct ScipProvider {
    id: SemanticProviderId,
    workspace_root: PathBuf,
    index_path: PathBuf,
    index_digest: String,
    file_hashes_at_index_time: HashMap<String, String>,
    index: ScipIndex,
}

impl ScipProvider {
    pub fn load_from_file(workspace_root: PathBuf, index_path: PathBuf) -> Result<Self, CoreError> {
        let bytes = fs::read(&index_path).map_err(|e| {
            CoreError::NotFound(format!(
                "Failed to read SCIP index file at {:?}: {e}",
                index_path
            ))
        })?;

        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let index_digest = hex::encode(hasher.finalize());

        let index: ScipIndex = serde_json::from_slice(&bytes).map_err(|e| {
            CoreError::SchemaViolation(format!("Failed to parse SCIP JSON index: {e}"))
        })?;

        // Snapshot current file hashes of all referenced documents at index load time
        let mut file_hashes = HashMap::new();
        for doc in &index.documents {
            let full_path = workspace_root.join(&doc.relative_path);
            if let Ok(src_bytes) = fs::read(&full_path) {
                let mut h = Sha256::new();
                h.update(&src_bytes);
                file_hashes.insert(doc.relative_path.clone(), hex::encode(h.finalize()));
            }
        }

        let id = SemanticProviderId::new("scip").unwrap();
        Ok(Self {
            id,
            workspace_root,
            index_path,
            index_digest,
            file_hashes_at_index_time: file_hashes,
            index,
        })
    }

    pub fn index_digest(&self) -> &str {
        &self.index_digest
    }

    /// Checks whether the SCIP index is still current against the workspace files.
    /// If any indexed source file has changed, returns false (STALE).
    pub fn is_index_current(&self) -> bool {
        for (rel_path, expected_hash) in &self.file_hashes_at_index_time {
            let full = self.workspace_root.join(rel_path);
            let Ok(bytes) = fs::read(&full) else {
                return false;
            };
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let current_hash = hex::encode(hasher.finalize());
            if current_hash != *expected_hash {
                return false;
            }
        }
        true
    }

    pub fn is_stale(&self) -> bool {
        !self.is_index_current()
    }

    fn generation(&self) -> SemanticGeneration {
        SemanticGeneration::with_digest(1, 1, self.index_digest.clone())
    }

    fn parse_range(&self, range: &[usize]) -> SourceRange {
        if range.len() == 3 {
            SourceRange::new(range[0], range[1], range[0], range[2])
        } else if range.len() >= 4 {
            SourceRange::new(range[0], range[1], range[2], range[3])
        } else {
            SourceRange::point(0, 0)
        }
    }
}

impl SemanticProvider for ScipProvider {
    fn id(&self) -> SemanticProviderId {
        self.id.clone()
    }

    fn name(&self) -> &str {
        "scip"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Indexed
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            symbol_search: true,
            definition: true,
            references: true,
            structural_search: false,
            diagnostics: false,
            packages: false,
        }
    }

    fn is_available(&self) -> bool {
        self.index_path.exists()
    }

    fn symbol_search<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<SymbolRecord>, CoreError>> {
        Box::pin(async move {
            let current_gen = self.generation();
            let mut results = Vec::new();

            for doc in &self.index.documents {
                for occ in &doc.occurrences {
                    if (occ.symbol_roles & 1) != 0 && occ.symbol.contains(query) {
                        let range = self.parse_range(&occ.range);
                        let id = SymbolId::new(occ.symbol.replace(['/', ':', ' '], "_")).unwrap();
                        let uri_path = format!("symbol://scip/{}", occ.symbol);
                        let uri = ResourceUri::parse(&uri_path).unwrap_or_else(|_| {
                            ResourceUri::parse("symbol://scip/symbol").unwrap()
                        });

                        results.push(SymbolRecord {
                            id,
                            name: occ.symbol.clone(),
                            kind: SymbolKind::Function,
                            location: SourceLocation::new(
                                doc.relative_path.clone(),
                                range,
                                self.id.clone(),
                                current_gen.clone(),
                            ),
                            container_name: None,
                            signature: None,
                            uri,
                            documentation: None,
                        });

                        if results.len() >= limit {
                            return Ok(results);
                        }
                    }
                }
            }

            Ok(results)
        })
    }

    fn symbol_definition<'a>(
        &'a self,
        symbol: &'a str,
        _file: Option<&'a str>,
        _line: Option<usize>,
        _col: Option<usize>,
    ) -> BoxFuture<'a, Result<SemanticLookupResult<SourceLocation>, CoreError>> {
        Box::pin(async move {
            let current_gen = self.generation();
            let mut matches = Vec::new();

            for doc in &self.index.documents {
                for occ in &doc.occurrences {
                    if (occ.symbol_roles & 1) != 0
                        && (occ.symbol == symbol || occ.symbol.ends_with(symbol))
                    {
                        let range = self.parse_range(&occ.range);
                        matches.push(SourceLocation::new(
                            doc.relative_path.clone(),
                            range,
                            self.id.clone(),
                            current_gen.clone(),
                        ));
                    }
                }
            }

            if matches.is_empty() {
                Ok(SemanticLookupResult::NotFound)
            } else if matches.len() == 1 {
                Ok(SemanticLookupResult::Resolved(matches.remove(0)))
            } else {
                let candidates = matches
                    .into_iter()
                    .enumerate()
                    .map(|(i, loc)| SymbolRecord {
                        id: SymbolId::new(format!("scip_cand_{i}")).unwrap(),
                        name: symbol.to_string(),
                        kind: SymbolKind::Function,
                        location: loc,
                        container_name: None,
                        signature: None,
                        uri: ResourceUri::parse(&format!("symbol://scip/candidate_{i}")).unwrap(),
                        documentation: None,
                    })
                    .collect();
                Ok(SemanticLookupResult::Ambiguous(candidates))
            }
        })
    }

    fn symbol_references<'a>(
        &'a self,
        symbol: &'a str,
        _file: Option<&'a str>,
        _line: Option<usize>,
        _col: Option<usize>,
        limit: usize,
    ) -> BoxFuture<'a, Result<SemanticLookupResult<Vec<ReferenceRecord>>, CoreError>> {
        Box::pin(async move {
            let current_gen = self.generation();
            let mut refs = Vec::new();

            for doc in &self.index.documents {
                for occ in &doc.occurrences {
                    if occ.symbol == symbol || occ.symbol.ends_with(symbol) {
                        let range = self.parse_range(&occ.range);
                        let is_def = (occ.symbol_roles & 1) != 0;
                        refs.push(ReferenceRecord {
                            symbol_id: SymbolId::new(symbol.replace(['/', ':', ' '], "_")).unwrap(),
                            location: SourceLocation::new(
                                doc.relative_path.clone(),
                                range,
                                self.id.clone(),
                                current_gen.clone(),
                            ),
                            is_definition: is_def,
                            is_write: false,
                            snippet: None,
                        });

                        if refs.len() >= limit {
                            break;
                        }
                    }
                }
            }

            if refs.is_empty() {
                Ok(SemanticLookupResult::NotFound)
            } else {
                Ok(SemanticLookupResult::Resolved(refs))
            }
        })
    }
}
