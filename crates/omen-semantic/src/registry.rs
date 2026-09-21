use crate::cache::{SemanticCache, SemanticWitness};
use crate::provider::{ProviderKind, SemanticLookupResult, SemanticProvider};
use crate::types::*;
use omen_core::{CoreError, SemanticProviderId};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_SEMANTIC_TIMEOUT: Duration = Duration::from_millis(3000);
/// Live providers may need to start a process, initialize, become ready, and
/// then answer the request. This must cover the rust-analyzer readiness and
/// request bounds without making indexed/structural providers slower.
pub const DEFAULT_LIVE_SEMANTIC_TIMEOUT: Duration = Duration::from_millis(40000);
pub const DEFAULT_RESULT_LIMIT: usize = 50;

fn provider_timeout(kind: ProviderKind, explicit: Option<Duration>) -> Duration {
    explicit.unwrap_or(match kind {
        ProviderKind::Live => DEFAULT_LIVE_SEMANTIC_TIMEOUT,
        ProviderKind::Indexed | ProviderKind::Structural | ProviderKind::Ecosystem => {
            DEFAULT_SEMANTIC_TIMEOUT
        }
    })
}

/// Central registry managing semantic providers and deterministic routing.
pub struct SemanticProviderRegistry {
    providers: Vec<Arc<dyn SemanticProvider>>,
    cache: SemanticCache,
    workspace_root: PathBuf,
    workspace_generation: std::sync::atomic::AtomicU64,
}

fn validate_hint(
    file: Option<&str>,
    line: Option<usize>,
    col: Option<usize>,
) -> Result<Option<SemanticHint>, CoreError> {
    match (file, line, col) {
        (None, None, None) => Ok(None),
        (Some(file), Some(line), Some(col)) => Ok(Some(SemanticHint {
            file: file.replace('\\', "/").trim_start_matches("./").to_owned(),
            line,
            col,
        })),
        _ => Err(CoreError::SchemaViolation(
            "SEMANTIC_HINT_INCOMPLETE: file, line, and col must be supplied together".into(),
        )),
    }
}

fn location_matches_hint(location: &SourceLocation, hint: &SemanticHint) -> bool {
    location.file.replace('\\', "/").trim_start_matches("./") == hint.file
        && location.range.contains(hint.line, hint.col)
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
            let timeout_duration = provider_timeout(p.kind(), timeout);
            let records = tokio::time::timeout(timeout_duration, p.symbol_search(query, limit))
                .await
                .map_err(|_| {
                    CoreError::ExecutionFailed(format!(
                        "Semantic provider '{}' timed out during symbol search for '{}'",
                        p.name(),
                        query
                    ))
                })??;
            if !records.is_empty() {
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
        let hint = validate_hint(file, line, col)?;

        // 1. Try Live LSP
        for p in self
            .providers
            .iter()
            .filter(|p| p.kind() == ProviderKind::Live && p.is_available())
        {
            if !p.capabilities().definition {
                continue;
            }
            let timeout_duration = provider_timeout(p.kind(), timeout);
            let res = tokio::time::timeout(
                timeout_duration,
                p.symbol_definition(symbol, None, None, None),
            )
            .await
            .map_err(|_| {
                CoreError::ExecutionFailed(format!(
                    "Semantic provider '{}' timed out during definition lookup for '{}'",
                    p.name(),
                    symbol
                ))
            })??;

            match resolve_target(symbol, hint.as_ref(), res)? {
                SemanticLookupResult::Resolved(loc) => {
                    let key = SemanticTargetKey::from_location(symbol, &loc);
                    if let Some((cached, omen_core::ValidityState::Current)) =
                        self.cache.get_definition(&key)
                    {
                        return Ok(SemanticLookupResult::Resolved(cached));
                    }
                    let witnesses = self.witness_file(&loc.file);
                    self.cache.insert_definition(
                        &key,
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
                p.symbol_definition(symbol, None, None, None),
            )
            .await
            .map_err(|_| {
                CoreError::ExecutionFailed(format!(
                    "Semantic provider '{}' timed out during definition lookup for '{}'",
                    p.name(),
                    symbol
                ))
            })??;

            match resolve_target(symbol, hint.as_ref(), res)? {
                SemanticLookupResult::Resolved(loc) => {
                    let key = SemanticTargetKey::from_location(symbol, &loc);
                    if let Some((cached, omen_core::ValidityState::Current)) =
                        self.cache.get_definition(&key)
                    {
                        return Ok(SemanticLookupResult::Resolved(cached));
                    }
                    let witnesses = self.witness_file(&loc.file);
                    self.cache.insert_definition(
                        &key,
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
        let hint = validate_hint(file, line, col)?;

        // 1. Try Live LSP
        for p in self
            .providers
            .iter()
            .filter(|p| p.kind() == ProviderKind::Live && p.is_available())
        {
            if !p.capabilities().references {
                continue;
            }
            let timeout_duration = provider_timeout(p.kind(), timeout);
            let res = tokio::time::timeout(
                timeout_duration,
                p.symbol_definition(symbol, None, None, None),
            )
            .await
            .map_err(|_| {
                CoreError::ExecutionFailed(format!(
                    "Semantic provider '{}' timed out during references lookup for '{}'",
                    p.name(),
                    symbol
                ))
            })??;

            let target = match resolve_target(symbol, hint.as_ref(), res)? {
                SemanticLookupResult::Resolved(location) => location,
                SemanticLookupResult::Ambiguous(candidates) => {
                    return Ok(SemanticLookupResult::Ambiguous(candidates));
                }
                SemanticLookupResult::Stale(_location) => {
                    return Ok(SemanticLookupResult::Stale(Vec::new()));
                }
                SemanticLookupResult::NotFound => continue,
                SemanticLookupResult::Unsupported => continue,
            };
            let key = SemanticTargetKey::from_location(symbol, &target);
            if let Some((refs, omen_core::ValidityState::Current)) = self.cache.get_references(&key)
            {
                return Ok(SemanticLookupResult::Resolved(
                    refs.into_iter().take(limit).collect(),
                ));
            }
            let res = tokio::time::timeout(
                timeout_duration,
                p.symbol_references(
                    symbol,
                    Some(&target.file),
                    Some(target.range.start_line),
                    Some(target.range.start_col),
                    limit,
                ),
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
                        &key,
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
                p.symbol_definition(symbol, None, None, None),
            )
            .await
            .map_err(|_| {
                CoreError::ExecutionFailed(format!(
                    "Semantic provider '{}' timed out during references lookup for '{}'",
                    p.name(),
                    symbol
                ))
            })??;

            let target = match resolve_target(symbol, hint.as_ref(), res)? {
                SemanticLookupResult::Resolved(location) => location,
                SemanticLookupResult::Ambiguous(candidates) => {
                    return Ok(SemanticLookupResult::Ambiguous(candidates));
                }
                SemanticLookupResult::Stale(_location) => {
                    return Ok(SemanticLookupResult::Stale(Vec::new()));
                }
                SemanticLookupResult::NotFound => continue,
                SemanticLookupResult::Unsupported => continue,
            };
            let key = SemanticTargetKey::from_location(symbol, &target);
            if let Some((refs, omen_core::ValidityState::Current)) = self.cache.get_references(&key)
            {
                return Ok(SemanticLookupResult::Resolved(
                    refs.into_iter().take(limit).collect(),
                ));
            }
            let res = tokio::time::timeout(
                timeout_duration,
                p.symbol_references(
                    symbol,
                    Some(&target.file),
                    Some(target.range.start_line),
                    Some(target.range.start_col),
                    limit,
                ),
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
                        &key,
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
        let mut all_diags = Vec::new();

        for p in self
            .providers
            .iter()
            .filter(|p| p.is_available() && p.capabilities().diagnostics)
        {
            let provider_timeout = provider_timeout(p.kind(), timeout);
            if let Ok(Ok(diags)) = tokio::time::timeout(provider_timeout, p.diagnostics()).await {
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

fn resolve_target(
    symbol: &str,
    hint: Option<&SemanticHint>,
    result: SemanticLookupResult<SourceLocation>,
) -> Result<SemanticLookupResult<SourceLocation>, CoreError> {
    match result {
        SemanticLookupResult::Resolved(location) => {
            if hint.is_some_and(|hint| !location_matches_hint(&location, hint)) {
                return Err(CoreError::SchemaViolation(format!(
                    "SEMANTIC_HINT_MISMATCH: coordinates do not identify requested symbol '{symbol}'"
                )));
            }
            Ok(SemanticLookupResult::Resolved(location))
        }
        SemanticLookupResult::Ambiguous(candidates) => {
            if let Some(hint) = hint {
                let matches: Vec<_> = candidates
                    .into_iter()
                    .filter(|candidate| location_matches_hint(&candidate.location, hint))
                    .collect();
                return match matches.as_slice() {
                    [candidate] => Ok(SemanticLookupResult::Resolved(candidate.location.clone())),
                    [] => Err(CoreError::SchemaViolation(format!(
                        "SEMANTIC_HINT_MISMATCH: coordinates do not identify requested symbol '{symbol}'"
                    ))),
                    _ => Ok(SemanticLookupResult::Ambiguous(matches)),
                };
            }
            Ok(SemanticLookupResult::Ambiguous(candidates))
        }
        other => Ok(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_provider_default_budget_covers_readiness_and_request_bounds() {
        assert!(DEFAULT_LIVE_SEMANTIC_TIMEOUT >= Duration::from_secs(15 + 5));
        assert!(DEFAULT_LIVE_SEMANTIC_TIMEOUT <= Duration::from_secs(45));
    }

    #[test]
    fn non_live_provider_default_remains_short_and_bounded() {
        assert_eq!(DEFAULT_SEMANTIC_TIMEOUT, Duration::from_secs(3));
        assert!(DEFAULT_SEMANTIC_TIMEOUT < DEFAULT_LIVE_SEMANTIC_TIMEOUT);
    }

    #[test]
    fn explicit_timeout_is_selected_over_live_default() {
        let explicit = Duration::from_millis(17);
        assert_eq!(
            provider_timeout(ProviderKind::Live, Some(explicit)),
            explicit
        );
        assert_eq!(
            provider_timeout(ProviderKind::Live, None),
            DEFAULT_LIVE_SEMANTIC_TIMEOUT
        );
        assert_eq!(
            provider_timeout(ProviderKind::Indexed, None),
            DEFAULT_SEMANTIC_TIMEOUT
        );
    }
}
