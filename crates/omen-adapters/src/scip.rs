use omen_core::{CoreError, ResourceUri, SemanticProviderId, SymbolId};
use omen_semantic::provider::{
    BoxFuture, ProviderCapabilities, ProviderKind, SemanticLookupResult, SemanticProvider,
};
use omen_semantic::types::*;
use prost::Message;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Genuine binary Protobuf SCIP wire models.
pub mod proto {
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct Index {
        #[prost(message, optional, tag = "1")]
        pub metadata: ::core::option::Option<Metadata>,
        #[prost(message, repeated, tag = "2")]
        pub documents: ::prost::alloc::vec::Vec<Document>,
        #[prost(message, repeated, tag = "3")]
        pub external_symbols: ::prost::alloc::vec::Vec<SymbolInformation>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct Metadata {
        #[prost(int32, tag = "1")]
        pub version: i32,
        #[prost(message, optional, tag = "2")]
        pub tool_info: ::core::option::Option<ToolInfo>,
        #[prost(string, tag = "3")]
        pub project_root: ::prost::alloc::string::String,
        #[prost(int32, tag = "4")]
        pub text_document_encoding: i32,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct ToolInfo {
        #[prost(string, tag = "1")]
        pub name: ::prost::alloc::string::String,
        #[prost(string, tag = "2")]
        pub version: ::prost::alloc::string::String,
        #[prost(string, repeated, tag = "3")]
        pub arguments: ::prost::alloc::vec::Vec<::prost::alloc::string::String>,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct Document {
        #[prost(string, tag = "1")]
        pub language: ::prost::alloc::string::String,
        #[prost(string, tag = "2")]
        pub relative_path: ::prost::alloc::string::String,
        #[prost(message, repeated, tag = "3")]
        pub occurrences: ::prost::alloc::vec::Vec<Occurrence>,
        #[prost(message, repeated, tag = "4")]
        pub symbols: ::prost::alloc::vec::Vec<SymbolInformation>,
        #[prost(string, tag = "5")]
        pub text: ::prost::alloc::string::String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SymbolInformation {
        #[prost(string, tag = "1")]
        pub symbol: ::prost::alloc::string::String,
        #[prost(string, repeated, tag = "3")]
        pub documentation: ::prost::alloc::vec::Vec<::prost::alloc::string::String>,
        #[prost(message, repeated, tag = "4")]
        pub relationships: ::prost::alloc::vec::Vec<Relationship>,
        #[prost(int32, tag = "5")]
        pub kind: i32,
        #[prost(string, tag = "6")]
        pub display_name: ::prost::alloc::string::String,
        #[prost(string, tag = "8")]
        pub enclosing_symbol: ::prost::alloc::string::String,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct Relationship {
        #[prost(string, tag = "1")]
        pub symbol: ::prost::alloc::string::String,
        #[prost(bool, tag = "2")]
        pub is_reference: bool,
        #[prost(bool, tag = "3")]
        pub is_implementation: bool,
        #[prost(bool, tag = "4")]
        pub is_type_definition: bool,
        #[prost(bool, tag = "5")]
        pub is_definition: bool,
    }

    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct Occurrence {
        #[prost(int32, repeated, tag = "1")]
        pub range: ::prost::alloc::vec::Vec<i32>,
        #[prost(string, tag = "2")]
        pub symbol: ::prost::alloc::string::String,
        #[prost(int32, tag = "3")]
        pub symbol_roles: i32,
        #[prost(string, repeated, tag = "4")]
        pub override_documentation: ::prost::alloc::vec::Vec<::prost::alloc::string::String>,
        #[prost(int32, tag = "5")]
        pub syntax_kind: i32,
    }
}

/// SCIP indexed semantic provider with cryptographic freshness verification.
pub struct ScipProvider {
    id: SemanticProviderId,
    workspace_root: PathBuf,
    index_path: PathBuf,
    index_digest: String,
    file_hashes_at_index_time: HashMap<String, String>,
    index: proto::Index,
}

impl ScipProvider {
    /// Loads and decodes a genuine binary Protobuf SCIP index from file.
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

        let index = proto::Index::decode(&bytes[..]).map_err(|e| {
            CoreError::SchemaViolation(format!("Failed to parse binary SCIP protobuf index: {e}"))
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

    /// Serializes an Index to binary Protobuf bytes.
    pub fn encode_index(index: &proto::Index) -> Vec<u8> {
        index.encode_to_vec()
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

    fn parse_range(&self, range: &[i32]) -> SourceRange {
        if range.len() == 3 {
            SourceRange::new(
                range[0].max(0) as usize,
                range[1].max(0) as usize,
                range[0].max(0) as usize,
                range[2].max(0) as usize,
            )
        } else if range.len() >= 4 {
            SourceRange::new(
                range[0].max(0) as usize,
                range[1].max(0) as usize,
                range[2].max(0) as usize,
                range[3].max(0) as usize,
            )
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
            let is_stale = self.is_stale();
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
            } else if is_stale {
                // Stale index must return Stale and never Resolved!
                Ok(SemanticLookupResult::Stale(matches.remove(0)))
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
            let is_stale = self.is_stale();
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
            } else if is_stale {
                // Stale index must return Stale and never Resolved!
                Ok(SemanticLookupResult::Stale(refs))
            } else {
                Ok(SemanticLookupResult::Resolved(refs))
            }
        })
    }
}
