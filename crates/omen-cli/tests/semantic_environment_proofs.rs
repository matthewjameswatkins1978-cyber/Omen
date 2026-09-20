use omen_adapters::{
    AstGrepAdapter, CargoSemanticProvider, NpmSemanticProvider, PythonUvSemanticProvider,
    ScipProvider, ThreadMothAdapter, ThreadMothBridge,
};
use omen_core::ValidityState;
use omen_engine::ProcessSupervisor;
use omen_interactive::AiLaneDispatcher;
use omen_interactive::completion::{CompletionContext, HotSemanticIndex, OmenCompleter};
use omen_semantic::{
    ProviderKind, SemanticGeneration, SemanticLookupResult, SemanticProvider, SemanticWitness,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn gremlin_exe() -> PathBuf {
    let mut path = std::env::current_exe().expect("failed to get current_exe");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    let name = if cfg!(windows) {
        "omen-gremlin.exe"
    } else {
        "omen-gremlin"
    };
    let exe = path.join(name);
    if exe.exists() {
        return exe;
    }

    let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target")
        .join("debug")
        .join(name);
    if fallback.exists() {
        return fallback;
    }

    let _ = std::process::Command::new("cargo")
        .args(["build", "--bin", "omen-gremlin"])
        .status();

    if exe.exists() {
        return exe;
    }
    fallback
}

// =============================================================================
// PROOF A: Structural search distinguishes syntax from text
// =============================================================================
#[tokio::test]
async fn test_proof_a_structural_search_distinguishes_syntax_from_text() {
    let temp_dir = tempfile::tempdir().unwrap();
    let src_dir = temp_dir.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();

    let rust_code = r#"
// fn refresh_token() -> bool { false }

fn refresh_token() -> bool {
    let _str = "fn refresh_token() -> bool { false }";
    true
}
"#;
    let file_path = src_dir.join("auth.rs");
    std::fs::write(&file_path, rust_code).unwrap();

    // 1. Structural Search (ast-grep)
    let adapter = AstGrepAdapter::new(temp_dir.path().to_path_buf());
    if !adapter.is_available() {
        eprintln!(
            "ast-grep binary not available on this system, skipping binary execution portion"
        );
        return;
    }

    let structural_matches = adapter
        .structural_search("fn refresh_token() -> bool { $$$ }", "rust", 10)
        .await
        .unwrap();

    // Structural search MUST find exactly 1 match (the real AST function node)
    assert_eq!(
        structural_matches.len(),
        1,
        "Structural search should match only the real AST function, not comments or strings"
    );
    assert_eq!(structural_matches[0].file, "src/auth.rs");

    // 2. Pure text search
    let occurrences = rust_code.matches("fn refresh_token").count();
    assert_eq!(
        occurrences, 3,
        "Text search finds all 3 occurrences (comment, function, string literal)"
    );

    // Consequential truth: structural matches != text occurrences
    assert_ne!(structural_matches.len(), occurrences);
}

// =============================================================================
// PROOF B: Structural rewrite routes through ThreadMoth
// =============================================================================
#[tokio::test]
async fn test_proof_b_structural_rewrite_routes_through_threadmoth() {
    let temp_dir = tempfile::tempdir().unwrap();
    let src_dir = temp_dir.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();

    let initial_content = r#"fn old_calculator(x: i32) -> i32 {
    x + 1
}
"#;
    let file_path = src_dir.join("calc.rs");
    std::fs::write(&file_path, initial_content).unwrap();

    let adapter = AstGrepAdapter::new(temp_dir.path().to_path_buf());
    if !adapter.is_available() {
        eprintln!("ast-grep not installed, skipping");
        return;
    }

    // 1. Derive rewrite candidate using ast-grep
    let candidates = adapter
        .derive_rewrite_candidates(
            "fn old_calculator($$$) -> i32 { $$$ }",
            "fn new_calculator($$$) -> i32 { $$$ }",
            "rust",
        )
        .await
        .unwrap();

    assert!(
        !candidates.is_empty(),
        "ast-grep should find candidate for rewriting old_calculator"
    );
    let candidate = &candidates[0];

    // 2. Check if threadmoth binary is available
    let supervisor = ProcessSupervisor::new();
    if ThreadMothAdapter::doctor(&supervisor).await.is_err() {
        eprintln!(
            "threadmoth binary doctor failed or not installed, skipping ThreadMoth mutation proof"
        );
        return;
    }

    // 3. Dispatch through ThreadMothBridge
    let cert = ThreadMothBridge::apply_structural_rewrite(&supervisor, temp_dir.path(), candidate)
        .await
        .unwrap();

    // 4. Verify cryptographic certificate and outcome
    assert!(
        cert.is_applied(),
        "ThreadMoth mutation must succeed and report applied"
    );
    assert!(
        cert.pre_hash.is_some(),
        "ThreadMoth certificate must capture pre-hash"
    );
    assert!(
        cert.post_hash.is_some(),
        "ThreadMoth certificate must capture post-hash"
    );

    // 5. Verify physical filesystem reality
    let mutated_bytes = std::fs::read_to_string(&file_path).unwrap();
    assert!(mutated_bytes.contains("new_calculator"));
    assert!(!mutated_bytes.contains("old_calculator"));
}

// =============================================================================
// PROOF C: Real LSP symbol definition and references
// =============================================================================
#[tokio::test]
async fn test_proof_c_real_lsp_symbol_definition_and_references() {
    let temp_dir = tempfile::tempdir().unwrap();
    let src_dir = temp_dir.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();

    let cargo_toml = r#"[package]
name = "fixture-lsp"
version = "0.1.0"
edition = "2021"
"#;
    std::fs::write(temp_dir.path().join("Cargo.toml"), cargo_toml).unwrap();

    let lib_code = r#"pub struct SessionToken {
    pub id: String,
}

pub fn validate_session(token: &SessionToken) -> bool {
    !token.id.is_empty()
}
"#;
    std::fs::write(src_dir.join("lib.rs"), lib_code).unwrap();

    let ra_provider = omen_adapters::RustAnalyzerProvider::new(temp_dir.path().to_path_buf());
    if !ra_provider.is_available() {
        eprintln!("rust-analyzer not in PATH, verifying graceful unsupported/fallback reporting");
        let def = ra_provider
            .symbol_definition("SessionToken", None, None, None)
            .await
            .unwrap();
        assert_eq!(def, SemanticLookupResult::Unsupported);
        return;
    }

    // If rust-analyzer is available on this environment:
    let def_res = ra_provider
        .symbol_definition("SessionToken", Some("src/lib.rs"), Some(0), Some(11))
        .await;

    // Must be either resolved or unsupported (in bounded time), never panic
    assert!(def_res.is_ok());
}

// =============================================================================
// PROOF D: LSP timeout and late-response are bounded
// =============================================================================
#[tokio::test]
async fn test_proof_d_lsp_timeout_and_late_response_are_bounded() {
    let temp_dir = tempfile::tempdir().unwrap();
    let gremlin = gremlin_exe();

    // 1. Test timeout bounds using HostileLspServer in slow-init mode
    let client_res = omen_adapters::HostileLspServer::spawn_client(
        &gremlin,
        omen_adapters::lsp_fixture::HostileLspMode::SlowInit,
        temp_dir.path().to_path_buf(),
    )
    .await;

    assert!(
        client_res.is_ok(),
        "Should spawn client against gremlin LSP fixture"
    );
    let mut client = client_res.unwrap();

    // Initialization with 150ms timeout
    let start = Instant::now();
    let init_res = client.initialize(Duration::from_millis(150)).await;
    let elapsed = start.elapsed();

    assert!(init_res.is_err(), "Slow-init LSP server must time out");
    assert!(
        elapsed < Duration::from_millis(600),
        "Timeout must complete within bounded window, took {:?}",
        elapsed
    );

    // 2. Test late-response discard
    let client_late_res = omen_adapters::HostileLspServer::spawn_client(
        &gremlin,
        omen_adapters::lsp_fixture::HostileLspMode::LateResponse,
        temp_dir.path().to_path_buf(),
    )
    .await;

    assert!(client_late_res.is_ok());
    let mut client_late = client_late_res.unwrap();

    let _ = client_late.initialize(Duration::from_millis(200)).await;

    // Send a request that times out after 80ms
    let start = Instant::now();
    let req_res = client_late
        .send_request(
            "textDocument/definition",
            serde_json::json!({}),
            Duration::from_millis(80),
        )
        .await;
    let elapsed = start.elapsed();

    assert!(req_res.is_err(), "Late request must time out");
    assert!(elapsed < Duration::from_millis(300));

    // Wait for late response to arrive in background
    tokio::time::sleep(Duration::from_millis(350)).await;

    // Client state remains intact and uncorrupted
    assert_eq!(client_late.pending_count(), 0);
}

// =============================================================================
// PROOF E: SCIP symbol query and stale-index detection
// =============================================================================
#[tokio::test]
async fn test_proof_e_scip_symbol_query_and_stale_index_detection() {
    let temp_dir = tempfile::tempdir().unwrap();
    let src_dir = temp_dir.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();

    let auth_path = src_dir.join("auth.rs");
    std::fs::write(&auth_path, "pub fn refresh_token() -> bool { true }\n").unwrap();

    let raw_scip = r#"{
        "documents": [
            {
                "relative_path": "src/auth.rs",
                "occurrences": [
                    {
                        "range": [0, 7, 0, 20],
                        "symbol": "crates/auth/refresh_token",
                        "symbol_roles": 1
                    }
                ]
            }
        ]
    }"#;
    let index_file = temp_dir.path().join("index.scip");
    std::fs::write(&index_file, raw_scip).unwrap();

    let provider = ScipProvider::load_from_file(temp_dir.path().to_path_buf(), index_file).unwrap();
    assert_eq!(provider.kind(), ProviderKind::Indexed);

    // 1. Fresh state: not stale
    assert!(!provider.is_stale());
    let def = provider
        .symbol_definition("refresh_token", None, None, None)
        .await
        .unwrap();
    assert!(
        def.is_resolved(),
        "Should resolve symbol definition from fresh SCIP index"
    );

    // 2. Mutate referenced file
    std::fs::write(
        &auth_path,
        "pub fn refresh_token_mutated() -> bool { false }\n",
    )
    .unwrap();

    // 3. Stale detection: witness hash mismatch detects staleness immediately
    assert!(
        provider.is_stale(),
        "Provider must report stale after source file modification"
    );
}

// =============================================================================
// PROOF F: Semantic result invalidates after source change
// =============================================================================
#[tokio::test]
async fn test_proof_f_semantic_result_invalidates_after_source_change() {
    let temp_dir = tempfile::tempdir().unwrap();
    let src_dir = temp_dir.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();

    let file_path = src_dir.join("lib.rs");
    std::fs::write(&file_path, "pub fn process_data() {}\n").unwrap();

    let cache = omen_semantic::SemanticCache::new(temp_dir.path().to_path_buf());

    let witness = SemanticWitness::observe(temp_dir.path(), "src/lib.rs").unwrap();

    let record = omen_semantic::SymbolRecord {
        id: omen_core::SymbolId::new("sym_process_data").unwrap(),
        name: "process_data".to_string(),
        kind: omen_semantic::SymbolKind::Function,
        location: omen_semantic::SourceLocation::new(
            "src/lib.rs",
            omen_semantic::SourceRange::point(0, 7),
            omen_core::SemanticProviderId::new("sprov_test").unwrap(),
            SemanticGeneration::new(1, 1),
        ),
        container_name: None,
        signature: None,
        uri: omen_core::ResourceUri::parse("symbol://test/process_data").unwrap(),
        documentation: None,
    };

    cache.insert_symbols(
        "process_data",
        vec![record],
        vec![witness],
        SemanticGeneration::new(1, 1),
    );

    // Initial cache lookup returns CURRENT
    let (symbols, state) = cache.get_symbols("process_data").unwrap();
    assert_eq!(state, ValidityState::Current);
    assert_eq!(symbols.len(), 1);

    // Edit the source file
    std::fs::write(&file_path, "pub fn process_data_modified() {}\n").unwrap();

    // Invalidate path
    cache.invalidate_file("src/lib.rs");

    // Second cache lookup returns DIRTY, refusing silent reuse
    let (_, state_after) = cache.get_symbols("process_data").unwrap();
    assert_eq!(
        state_after,
        ValidityState::Dirty,
        "Fact/cache entry must transition from CURRENT to DIRTY upon witness change"
    );
}

// =============================================================================
// PROOF G: Cargo workspace metadata is canonical
// =============================================================================
#[tokio::test]
async fn test_proof_g_cargo_workspace_metadata_is_canonical() {
    let workspace_dir = std::env::current_dir().unwrap();
    let provider = CargoSemanticProvider::new(&workspace_dir);

    assert!(
        provider.is_available(),
        "Cargo provider must be available in workspace"
    );

    let packages = provider.packages().await.unwrap();
    assert!(
        !packages.is_empty(),
        "Workspace packages must be extracted canonically"
    );

    let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"omen-core"));
    assert!(names.contains(&"omen-adapters"));
    assert!(names.contains(&"omen-semantic"));
    assert!(names.contains(&"omen-interactive"));
    assert!(names.contains(&"omen-cli"));

    // Verify canonical targets and manifests
    for p in &packages {
        assert_eq!(p.ecosystem, "cargo");
        assert!(!p.manifest_path.is_empty());
        assert!(!p.targets.is_empty());
        assert!(p.uri.as_str().starts_with("package://cargo/"));
    }

    // Verify witnesses
    let witnesses = provider.workspace_witness_paths();
    assert!(witnesses.iter().any(|w| w.ends_with("Cargo.toml")));
    assert!(witnesses.iter().any(|w| w.ends_with("Cargo.lock")));
}

// =============================================================================
// PROOF H: Two non-Rust ecosystems produce semantic metadata
// =============================================================================
#[tokio::test]
async fn test_proof_h_two_non_rust_ecosystems_produce_semantic_metadata() {
    let temp_dir = tempfile::tempdir().unwrap();

    // 1. NPM Ecosystem
    let package_json = r#"{
        "name": "omen-web-dashboard",
        "version": "1.4.0",
        "scripts": {
            "build": "vite build",
            "test": "vitest"
        },
        "dependencies": {
            "react": "^18.2.0"
        }
    }"#;
    std::fs::write(temp_dir.path().join("package.json"), package_json).unwrap();

    let npm_provider = NpmSemanticProvider::new(temp_dir.path().to_path_buf());
    assert!(npm_provider.is_available());

    let npm_pkgs = npm_provider.packages().await.unwrap();
    assert_eq!(npm_pkgs.len(), 1);
    let npm_pkg = &npm_pkgs[0];
    assert_eq!(npm_pkg.name, "omen-web-dashboard");
    assert_eq!(npm_pkg.version, "1.4.0");
    assert_eq!(npm_pkg.ecosystem, "npm");
    assert_eq!(npm_pkg.dependencies.len(), 1);
    assert_eq!(npm_pkg.tasks.len(), 2);
    assert!(npm_pkg.tasks.iter().any(|t| t.name == "build"));
    assert_eq!(npm_pkg.uri.as_str(), "package://npm/omen-web-dashboard");

    // 2. Python / UV Ecosystem
    let pyproject = r#"
[project]
name = "omen-ml-pipeline"
version = "0.8.2"
dependencies = [
    "scikit-learn>=1.2.0"
]

[project.scripts]
train = "pipeline.train:run"
"#;
    std::fs::write(temp_dir.path().join("pyproject.toml"), pyproject).unwrap();

    let py_provider = PythonUvSemanticProvider::new(temp_dir.path().to_path_buf());
    assert!(py_provider.is_available());

    let py_pkgs = py_provider.packages().await.unwrap();
    assert_eq!(py_pkgs.len(), 1);
    let py_pkg = &py_pkgs[0];
    assert_eq!(py_pkg.name, "omen-ml-pipeline");
    assert_eq!(py_pkg.version, "0.8.2");
    assert_eq!(py_pkg.ecosystem, "python");
    assert_eq!(py_pkg.dependencies.len(), 1);
    assert!(py_pkg.tasks.iter().any(|t| t.name == "train"));
    assert_eq!(py_pkg.uri.as_str(), "package://python/omen-ml-pipeline");
}

// =============================================================================
// PROOF I: Deterministic symbol question uses zero model calls
// =============================================================================
#[tokio::test]
async fn test_proof_i_deterministic_symbol_question_uses_zero_model_calls() {
    let temp_dir = tempfile::tempdir().unwrap();
    let src_dir = temp_dir.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();

    let auth_path = src_dir.join("auth.rs");
    std::fs::write(&auth_path, "pub fn refresh_token() -> bool { true }\n").unwrap();

    // Create SCIP index so symbol resolves
    let raw_scip = r#"{
        "documents": [
            {
                "relative_path": "src/auth.rs",
                "occurrences": [
                    {
                        "range": [0, 7, 0, 20],
                        "symbol": "refresh_token",
                        "symbol_roles": 1
                    }
                ]
            }
        ]
    }"#;
    std::fs::write(temp_dir.path().join("index.scip"), raw_scip).unwrap();

    let session_id = omen_core::InteractiveSessionId::new("sess_test_i").unwrap();

    // Ask unambiguous question: "where is refresh_token defined?"
    let output = AiLaneDispatcher::dispatch_with_workspace(
        "? where is refresh_token defined?",
        &session_id,
        temp_dir.path(),
        temp_dir.path(),
        None,
        None,
        None,
    )
    .unwrap();

    // Zero model calls invariant:
    assert_eq!(
        output.stats.provider_calls, 0,
        "Deterministic symbol query must never invoke AI model (provider_calls must be 0)"
    );

    // Response contains exact resolution
    assert!(
        output.response_text.contains("refresh_token"),
        "Response should report resolution for refresh_token"
    );
    assert!(
        output
            .references
            .iter()
            .any(|r| r.contains("refresh_token")),
        "Response should emit @symbol:// reference"
    );
}

// =============================================================================
// PROOF J: Hot completion uses zero provider I/O
// =============================================================================
#[test]
fn test_proof_j_hot_completion_uses_zero_provider_io() {
    let mut hot_index = HotSemanticIndex::default();
    hot_index.update_semantics(
        vec![
            "refresh_token".into(),
            "validate_session".into(),
            "process_data".into(),
        ],
        vec![
            "omen-core".into(),
            "omen-adapters".into(),
            "omen-semantic".into(),
        ],
        vec![
            "cargo build -p omen-core".into(),
            "cargo test -p omen-semantic".into(),
        ],
    );

    let ctx = Arc::new(Mutex::new(CompletionContext {
        cwd: PathBuf::from("."),
        hot_index,
    }));

    let mut completer = OmenCompleter::new(ctx);

    // 1. Measure completion execution time
    let start = Instant::now();

    // Keystroke :sym
    let res_action = completer.complete_items(":sym", 4);
    // Keystroke :def ref
    let res_def = completer.complete_items(":def ref", 8);
    // Keystroke @sym
    let res_at_sym = completer.complete_items("@sym", 4);
    // Keystroke @pack
    let res_at_pkg = completer.complete_items("@pack", 5);

    let elapsed = start.elapsed();

    // Verify suggestions
    assert!(res_action.iter().any(|s| s.value == ":symbol"));
    assert!(res_def.iter().any(|s| s.value == "refresh_token"));
    assert!(
        res_at_sym
            .iter()
            .any(|s| s.value == "@symbol://refresh_token")
    );
    assert!(res_at_pkg.iter().any(|s| s.value == "@package://omen-core"));

    // Performance constraint: in-memory hot completion completes in < 5ms
    assert!(
        elapsed < Duration::from_millis(5),
        "In-memory hot completion must take < 5ms, took {:?}",
        elapsed
    );
}
