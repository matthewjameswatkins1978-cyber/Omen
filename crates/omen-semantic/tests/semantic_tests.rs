use omen_core::{ResourceUri, SemanticProviderId, SymbolId, ValidityState};
use omen_semantic::cache::{SemanticCache, SemanticWitness};
use omen_semantic::provider::{
    BoxFuture, ProviderCapabilities, ProviderKind, SemanticLookupResult, SemanticProvider,
};
use omen_semantic::registry::SemanticProviderRegistry;
use omen_semantic::types::*;
use std::fs;
use std::sync::Arc;
use tempfile::tempdir;

struct MockLiveLspProvider {
    id: SemanticProviderId,
    available: bool,
    records: Vec<SymbolRecord>,
}

impl SemanticProvider for MockLiveLspProvider {
    fn id(&self) -> SemanticProviderId {
        self.id.clone()
    }
    fn name(&self) -> &str {
        "mock-lsp"
    }
    fn kind(&self) -> ProviderKind {
        ProviderKind::Live
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            symbol_search: true,
            definition: true,
            references: true,
            structural_search: false,
            diagnostics: true,
            packages: false,
        }
    }
    fn is_available(&self) -> bool {
        self.available
    }

    fn symbol_search<'a>(
        &'a self,
        query: &'a str,
        _limit: usize,
    ) -> BoxFuture<'a, Result<Vec<SymbolRecord>, omen_core::CoreError>> {
        let matches: Vec<_> = self
            .records
            .iter()
            .filter(|r| r.name.contains(query))
            .cloned()
            .collect();
        Box::pin(async move { Ok(matches) })
    }

    fn symbol_definition<'a>(
        &'a self,
        symbol: &'a str,
        _file: Option<&'a str>,
        _line: Option<usize>,
        _col: Option<usize>,
    ) -> BoxFuture<'a, Result<SemanticLookupResult<SourceLocation>, omen_core::CoreError>> {
        let matches: Vec<_> = self
            .records
            .iter()
            .filter(|r| r.name == symbol)
            .cloned()
            .collect();
        Box::pin(async move {
            if matches.is_empty() {
                Ok(SemanticLookupResult::NotFound)
            } else if matches.len() == 1 {
                Ok(SemanticLookupResult::Resolved(matches[0].location.clone()))
            } else {
                Ok(SemanticLookupResult::Ambiguous(matches))
            }
        })
    }
}

#[tokio::test]
async fn test_cache_and_witness_invalidation() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Create a witness source file
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let auth_file = src_dir.join("auth.rs");
    fs::write(&auth_file, "pub fn refresh_token() {}").unwrap();

    let cache = SemanticCache::new(root.to_path_buf());
    let provider_id = SemanticProviderId::new("test_prov").unwrap();
    let generation = SemanticGeneration::new(1, 1);

    let witness = SemanticWitness::observe(root, "src/auth.rs").unwrap();
    assert!(witness.is_current(root));

    let record = SymbolRecord {
        id: SymbolId::new("refresh_token").unwrap(),
        name: "refresh_token".into(),
        kind: SymbolKind::Function,
        location: SourceLocation::new(
            "src/auth.rs",
            SourceRange::new(0, 7, 0, 20),
            provider_id.clone(),
            generation.clone(),
        ),
        container_name: None,
        signature: Some("pub fn refresh_token()".into()),
        uri: ResourceUri::parse("symbol://crate/auth/refresh_token").unwrap(),
        documentation: None,
    };

    cache.insert_symbols(
        "refresh_token",
        vec![record.clone()],
        vec![witness],
        generation.clone(),
    );

    // 1. Initial lookup: CURRENT
    let (results, validity) = cache.get_symbols("refresh_token").unwrap();
    assert_eq!(validity, ValidityState::Current);
    assert_eq!(results.len(), 1);

    // 2. Modify witness file
    fs::write(&auth_file, "pub fn refresh_token_v2() {}").unwrap();

    // 3. Subsequent lookup: witness hash changed -> transitions to DIRTY
    let (_, validity_after) = cache.get_symbols("refresh_token").unwrap();
    assert_eq!(validity_after, ValidityState::Dirty);
}

#[tokio::test]
async fn test_ambiguous_symbol_resolution_never_collapses() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let mut registry = SemanticProviderRegistry::new(root.to_path_buf());

    let prov_id = SemanticProviderId::new("lsp").unwrap();
    let generation = SemanticGeneration::new(1, 1);

    let sym_a = SymbolRecord {
        id: SymbolId::new("parser_parse").unwrap(),
        name: "parse".into(),
        kind: SymbolKind::Function,
        location: SourceLocation::new(
            "src/parser.rs",
            SourceRange::point(10, 0),
            prov_id.clone(),
            generation.clone(),
        ),
        container_name: Some("parser".into()),
        signature: Some("fn parse(&str)".into()),
        uri: ResourceUri::parse("symbol://crate/parser/parse").unwrap(),
        documentation: None,
    };

    let sym_b = SymbolRecord {
        id: SymbolId::new("config_parse").unwrap(),
        name: "parse".into(),
        kind: SymbolKind::Function,
        location: SourceLocation::new(
            "src/config.rs",
            SourceRange::point(20, 0),
            prov_id.clone(),
            generation.clone(),
        ),
        container_name: Some("config".into()),
        signature: Some("fn parse(Path)".into()),
        uri: ResourceUri::parse("symbol://crate/config/parse").unwrap(),
        documentation: None,
    };

    registry.register(Arc::new(MockLiveLspProvider {
        id: prov_id,
        available: true,
        records: vec![sym_a, sym_b],
    }));

    let result = registry
        .find_definition("parse", None, None, None, None)
        .await
        .unwrap();

    // Must be AMBIGUOUS with 2 candidates — never silently picked one!
    match result {
        SemanticLookupResult::Ambiguous(candidates) => {
            assert_eq!(candidates.len(), 2);
            assert_eq!(candidates[0].container_name.as_deref(), Some("parser"));
            assert_eq!(candidates[1].container_name.as_deref(), Some("config"));
        }
        other => panic!("Expected Ambiguous result, got {:?}", other),
    }
}

#[test]
fn semantic_cache_isolates_same_named_targets_in_both_orders() {
    let tmp = tempdir().unwrap();
    let cache = SemanticCache::new(tmp.path().to_path_buf());
    let provider = SemanticProviderId::new("lsp").unwrap();
    let generation = SemanticGeneration::new(1, 1);
    let location_a = SourceLocation::new(
        "src/a.rs",
        SourceRange::point(2, 4),
        provider.clone(),
        generation.clone(),
    );
    let location_b =
        SourceLocation::new("src/b.rs", SourceRange::point(8, 4), provider, generation);
    let key_a = SemanticTargetKey::from_location("process", &location_a);
    let key_b = SemanticTargetKey::from_location("process", &location_b);
    cache.insert_definition(
        &key_a,
        location_a.clone(),
        vec![],
        location_a.generation.clone(),
    );
    cache.insert_definition(
        &key_b,
        location_b.clone(),
        vec![],
        location_b.generation.clone(),
    );
    assert_eq!(cache.get_definition(&key_a).unwrap().0, location_a);
    assert_eq!(cache.get_definition(&key_b).unwrap().0, location_b);

    let refs_a = vec![ReferenceRecord {
        symbol_id: SymbolId::new("a_ref").unwrap(),
        location: location_a.clone(),
        is_definition: false,
        is_write: false,
        snippet: None,
    }];
    let refs_b = vec![ReferenceRecord {
        symbol_id: SymbolId::new("b_ref").unwrap(),
        location: location_b.clone(),
        is_definition: false,
        is_write: false,
        snippet: None,
    }];
    cache.insert_references(
        &key_b,
        refs_b.clone(),
        vec![],
        location_b.generation.clone(),
    );
    cache.insert_references(
        &key_a,
        refs_a.clone(),
        vec![],
        location_a.generation.clone(),
    );
    assert_eq!(cache.get_references(&key_a).unwrap().0, refs_a);
    assert_eq!(cache.get_references(&key_b).unwrap().0, refs_b);
}
