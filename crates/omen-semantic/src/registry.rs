use crate::cache::{SemanticCache, SemanticWitness};
use crate::provider::{ProviderKind, SemanticLookupResult, SemanticProvider};
use crate::types::*;
use omen_core::{CoreError, SemanticProviderId};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_SEMANTIC_TIMEOUT: Duration = Duration::from_millis(3000);
pub const DEFAULT_RESULT_LIMIT: usize = 50;

/// Central registry managing semantic providers and deterministic routing.
pub struct SemanticProviderRegistry {
    providers: Vec<Arc<dyn SemanticProvider>>,
    cache: SemanticCache,
    workspace_root: PathBuf,
    workspace_generation: std::sync::atomic::AtomicU64,
}

impl SemanticProviderRegistry {
    pub fn new(workspace_root: PathBuf) -> Self {
        let cache = SemanticCache::new(workspace_root.clone());
        Self {
            providers: Vec::new(),
            cache,
            workspace_root,
            workspace_generation: std::sync::atomic::AtomicU64::new(1),
        }
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn cache(&self) -> &SemanticCache {
        &self.cache
    }

    pub fn register(&mut self, provider: Arc<dyn SemanticProvider>) {
        // Replace existing provider with same ID if present
        let id = provider.id();
        self.providers.retain(|p| p.id() != id);
        self.providers.push(provider);
    }

    pub fn list_providers(&self) -> Vec<(SemanticProviderId, String, ProviderKind, bool)> {
        self.providers
            .iter()
            .map(|p| (p.id(), p.name().to_string(), p.kind(), p.is_available()))
            .collect()
    }

    pub fn get_provider(&self, id: &SemanticProviderId) -> Option<Arc<dyn SemanticProvider>> {
        self.providers.iter().find(|p| &p.id() == id).cloned()
    }

    pub fn bump_workspace_generation(&self) -> u64 {
        self.workspace_generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1
    }

    pub fn current_generation(&self) -> SemanticGeneration {
        let generation = self
            .workspace_generation
            .load(std::sync::atomic::Ordering::SeqCst);
        SemanticGeneration::new(generation, 1)
    }

    /// Searches for symbols across active providers following hierarchy:
    /// Live (LSP) -> Indexed (SCIP).
    pub async fn symbol_search(
        &self,
        query: &str,
        limit: Option<usize>,
        timeout: Option<Duration>,
    ) -> Result<Vec<SymbolRecord>, CoreError> {
        let limit = limit.unwrap_or(DEFAULT_RESULT_LIMIT);
        let timeout_duration = timeout.unwrap_or(DEFAULT_SEMANTIC_TIMEOUT);

        // Check cache first
        if let Some((records, omen_core::ValidityState::Current)) = self.cache.get_symbols(query) {
            return Ok(records.into_iter().take(limit).collect());
        }

        // 1. Try Live providers (LSP)
        for p in self
            .providers
            .iter()
            .filter(|p| p.kind() == ProviderKind::Live && p.is_available())
        {
            if !p.capabilities().symbol_search {
                continue;
            }
            if let Ok(Ok(records)) =
                tokio::time::timeout(timeout_duration, p.symbol_search(query, limit)).await
                && !records.is_empty()
            {
                let witnesses = self.extract_witnesses(&records);
                self.cache.insert_symbols(
                    query,
                    records.clone(),
                    witnesses,
                    self.current_generation(),
                );
                return Ok(records);
            }
        }

        // 2. Try Indexed SCIP providers
        for p in self
            .providers
            .iter()
            .filter(|p| p.kind() == ProviderKind::Indexed && p.is_available())
        {
            if !p.capabilities().symbol_search {
                continue;
            }
            if let Ok(Ok(records)) =
                tokio::time::timeout(timeout_duration, p.symbol_search(query, limit)).await
                && !records.is_empty()
            {
                let witnesses = self.extract_witnesses(&records);
                self.cache.insert_symbols(
                    query,
                    records.clone(),
                    witnesses,
                    self.current_generation(),
                );
                return Ok(records);
            }
        }

        Ok(Vec::new())
    }

    /// Resolves definition for a symbol or location following hierarchy:
    /// Live (LSP) -> Indexed (SCIP).
    pub async fn find_definition(
        &self,
        symbol: &str,
        file: Option<&str>,
        line: Option<usize>,
        col: Option<usize>,
        timeout: Option<Duration>,
    ) -> Result<SemanticLookupResult<SourceLocation>, CoreError> {
        let timeout_duration = timeout.unwrap_or(DEFAULT_SEMANTIC_TIMEOUT);

        if let Some((loc, omen_core::ValidityState::Current)) = self.cache.get_definition(symbol) {
            return Ok(SemanticLookupResult::Resolved(loc));
        }

        // 1. Try Live LSP
        for p in self
            .providers
            .iter()
            .filter(|p| p.kind() == ProviderKind::Live && p.is_available())
        {
            if !p.capabilities().definition {
                continue;
            }
            let res = tokio::time::timeout(
                timeout_duration,
                p.symbol_definition(symbol, file, line, col),
            )
            .await
            .map_err(|_| {
                CoreError::ExecutionFailed(format!(
                    "Semantic provider '{}' timed out during definition lookup for '{}'",
                    p.name(),
                    symbol
                ))
            })??;

            match res {
                SemanticLookupResult::Resolved(loc) => {
                    let witnesses = self.witness_file(&loc.file);
                    self.cache.insert_definition(
                        symbol,
                        loc.clone(),
                        witnesses,
                        self.current_generation(),
                    );
                    return Ok(SemanticLookupResult::Resolved(loc));
                }
                SemanticLookupResult::Ambiguous(c) => {
                    return Ok(SemanticLookupResult::Ambiguous(c));
                }
                SemanticLookupResult::Stale(loc) => {
                    return Ok(SemanticLookupResult::Stale(loc));
                }
                SemanticLookupResult::NotFound => {}
                SemanticLookupResult::Unsupported => {}
            }
        }

        // 2. Try Indexed SCIP
        for p in self
            .providers
            .iter()
            .filter(|p| p.kind() == ProviderKind::Indexed && p.is_available())
        {
            if !p.capabilities().definition {
                continue;
            }
            let res = tokio::time::timeout(
                timeout_duration,
                p.symbol_definition(symbol, file, line, col),
            )
            .await
            .map_err(|_| {
                CoreError::ExecutionFailed(format!(
                    "Semantic provider '{}' timed out during definition lookup for '{}'",
                    p.name(),
                    symbol
                ))
            })??;

            match res {
                SemanticLookupResult::Resolved(loc) => {
                    let witnesses = self.witness_file(&loc.file);
                    self.cache.insert_definition(
                        symbol,
                        loc.clone(),
                        witnesses,
                        self.current_generation(),
                    );
                    return Ok(SemanticLookupResult::Resolved(loc));
                }
                SemanticLookupResult::Ambiguous(c) => {
                    return Ok(SemanticLookupResult::Ambiguous(c));
                }
                SemanticLookupResult::Stale(loc) => {
                    return Ok(SemanticLookupResult::Stale(loc));
                }
                SemanticLookupResult::NotFound => {}
                SemanticLookupResult::Unsupported => {}
            }
        }

        Ok(SemanticLookupResult::NotFound)
    }

    /// Finds references / call sites following hierarchy:
    /// Live (LSP) -> Indexed (SCIP).
    pub async fn find_references(
        &self,
        symbol: &str,
        file: Option<&str>,
        line: Option<usize>,
        col: Option<usize>,
        limit: Option<usize>,
        timeout: Option<Duration>,
    ) -> Result<SemanticLookupResult<Vec<ReferenceRecord>>, CoreError> {
        let limit = limit.unwrap_or(DEFAULT_RESULT_LIMIT);
        let timeout_duration = timeout.unwrap_or(DEFAULT_SEMANTIC_TIMEOUT);

        if let Some((refs, omen_core::ValidityState::Current)) = self.cache.get_references(symbol) {
            return Ok(SemanticLookupResult::Resolved(
                refs.into_iter().take(limit).collect(),
            ));
        }

        // 1. Try Live LSP
        for p in self
            .providers
            .iter()
            .filter(|p| p.kind() == ProviderKind::Live && p.is_available())
        {
            if !p.capabilities().references {
                continue;
            }
            let res = tokio::time::timeout(
                timeout_duration,
                p.symbol_references(symbol, file, line, col, limit),
            )
            .await
            .map_err(|_| {
                CoreError::ExecutionFailed(format!(
                    "Semantic provider '{}' timed out during references lookup for '{}'",
                    p.name(),
                    symbol
                ))
            })??;

            match res {
                SemanticLookupResult::Resolved(refs) => {
                    let witnesses = self.extract_ref_witnesses(&refs);
                    self.cache.insert_references(
                        symbol,
                        refs.clone(),
                        witnesses,
                        self.current_generation(),
                    );
                    return Ok(SemanticLookupResult::Resolved(refs));
                }
                SemanticLookupResult::Ambiguous(c) => {
                    return Ok(SemanticLookupResult::Ambiguous(c));
                }
                SemanticLookupResult::Stale(refs) => {
                    return Ok(SemanticLookupResult::Stale(refs));
                }
                SemanticLookupResult::NotFound => {}
                SemanticLookupResult::Unsupported => {}
            }
        }

        // 2. Try Indexed SCIP
        for p in self
            .providers
            .iter()
            .filter(|p| p.kind() == ProviderKind::Indexed && p.is_available())
        {
            if !p.capabilities().references {
                continue;
            }
            let res = tokio::time::timeout(
                timeout_duration,
                p.symbol_references(symbol, file, line, col, limit),
            )
            .await
            .map_err(|_| {
                CoreError::ExecutionFailed(format!(
                    "Semantic provider '{}' timed out during references lookup for '{}'",
                    p.name(),
                    symbol
                ))
            })??;

            match res {
                SemanticLookupResult::Resolved(refs) => {
                    let witnesses = self.extract_ref_witnesses(&refs);
                    self.cache.insert_references(
                        symbol,
                        refs.clone(),
                        witnesses,
                        self.current_generation(),
                    );
                    return Ok(SemanticLookupResult::Resolved(refs));
                }
                SemanticLookupResult::Ambiguous(c) => {
                    return Ok(SemanticLookupResult::Ambiguous(c));
                }
                SemanticLookupResult::Stale(refs) => {
                    return Ok(SemanticLookupResult::Stale(refs));
                }
                SemanticLookupResult::NotFound => {}
                SemanticLookupResult::Unsupported => {}
            }
        }

        Ok(SemanticLookupResult::NotFound)
    }

    /// Structural search (routed to ast-grep).
    pub async fn structural_search(
        &self,
        pattern: &str,
        language: &str,
        limit: Option<usize>,
        timeout: Option<Duration>,
    ) -> Result<Vec<StructuralMatch>, CoreError> {
        let limit = limit.unwrap_or(DEFAULT_RESULT_LIMIT);
        let timeout_duration = timeout.unwrap_or(DEFAULT_SEMANTIC_TIMEOUT);

        for p in self
            .providers
            .iter()
            .filter(|p| p.kind() == ProviderKind::Structural && p.is_available())
        {
            if !p.capabilities().structural_search {
                continue;
            }
            let res = tokio::time::timeout(
                timeout_duration,
                p.structural_search(pattern, language, limit),
            )
            .await;
            if let Ok(Ok(matches)) = res {
                return Ok(matches);
            }
        }
        Ok(Vec::new())
    }

    /// Discovers packages across ecosystem providers (Cargo, npm, uv, go).
    pub async fn packages(
        &self,
        timeout: Option<Duration>,
    ) -> Result<Vec<PackageRecord>, CoreError> {
        let timeout_duration = timeout.unwrap_or(DEFAULT_SEMANTIC_TIMEOUT);
        let mut all_packages = Vec::new();

        for p in self
            .providers
            .iter()
            .filter(|p| p.kind() == ProviderKind::Ecosystem && p.is_available())
        {
            if !p.capabilities().packages {
                continue;
            }
            if let Ok(Ok(pkgs)) = tokio::time::timeout(timeout_duration, p.packages()).await {
                all_packages.extend(pkgs);
            }
        }
        Ok(all_packages)
    }

    /// Collects diagnostics across all available providers with deduplication.
    pub async fn diagnostics(
        &self,
        timeout: Option<Duration>,
    ) -> Result<Vec<SemanticDiagnostic>, CoreError> {
        let timeout_duration = timeout.unwrap_or(DEFAULT_SEMANTIC_TIMEOUT);
        let mut all_diags = Vec::new();

        for p in self
            .providers
            .iter()
            .filter(|p| p.is_available() && p.capabilities().diagnostics)
        {
            if let Ok(Ok(diags)) = tokio::time::timeout(timeout_duration, p.diagnostics()).await {
                all_diags.extend(diags);
            }
        }

        // Deduplicate diagnostics by (file, start_line, code, message)
        let mut deduped: Vec<SemanticDiagnostic> = Vec::new();
        for d in all_diags {
            if !deduped.iter().any(|existing| {
                existing.location.file == d.location.file
                    && existing.location.range.start_line == d.location.range.start_line
                    && existing.code == d.code
                    && existing.message == d.message
            }) {
                deduped.push(d);
            }
        }
        Ok(deduped)
    }

    fn extract_witnesses(&self, records: &[SymbolRecord]) -> Vec<SemanticWitness> {
        let mut witnesses = Vec::new();
        for r in records {
            let Some(w) = SemanticWitness::observe(&self.workspace_root, &r.location.file) else {
                continue;
            };
            if !witnesses
                .iter()
                .any(|existing: &SemanticWitness| existing.relative_path == w.relative_path)
            {
                witnesses.push(w);
            }
        }
        witnesses
    }

    fn extract_ref_witnesses(&self, refs: &[ReferenceRecord]) -> Vec<SemanticWitness> {
        let mut witnesses = Vec::new();
        for r in refs {
            let Some(w) = SemanticWitness::observe(&self.workspace_root, &r.location.file) else {
                continue;
            };
            if !witnesses
                .iter()
                .any(|existing: &SemanticWitness| existing.relative_path == w.relative_path)
            {
                witnesses.push(w);
            }
        }
        witnesses
    }

    fn witness_file(&self, relative_path: &str) -> Vec<SemanticWitness> {
        if let Some(w) = SemanticWitness::observe(&self.workspace_root, relative_path) {
            vec![w]
        } else {
            Vec::new()
        }
    }
}
