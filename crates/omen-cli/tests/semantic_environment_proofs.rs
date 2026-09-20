use omen_adapters::{
    AstGrepAdapter, CargoSemanticProvider, NpmSemanticProvider, PythonUvSemanticProvider,
    RustAnalyzerProvider, ScipProvider, ThreadMothAdapter, ThreadMothBridge, scip_proto,
};
use omen_core::ValidityState;
use omen_engine::ProcessSupervisor;
use omen_interactive::AiLaneDispatcher;
use omen_interactive::completion::{CompletionContext, HotSemanticIndex, OmenCompleter};
use omen_semantic::types::{lsp_utf16_to_utf8_col, utf8_to_lsp_utf16_col};
use omen_semantic::{ProviderKind, SemanticGeneration, SemanticProvider, SemanticWitness};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const STATUS_NOT_EXECUTED: u8 = 0;
const STATUS_PASS: u8 = 1;

static AST_GREP_STATUS: AtomicU8 = AtomicU8::new(STATUS_NOT_EXECUTED);
static THREADMOTH_STATUS: AtomicU8 = AtomicU8::new(STATUS_NOT_EXECUTED);
static RUST_ANALYZER_STATUS: AtomicU8 = AtomicU8::new(STATUS_NOT_EXECUTED);
static SCIP_STATUS: AtomicU8 = AtomicU8::new(STATUS_NOT_EXECUTED);

fn is_external_acceptance_enabled() -> bool {
    std::env::var("OMEN_REQUIRE_SEMANTIC_EXTERNAL_PROOFS").as_deref() == Ok("1")
}

fn record_proof_status(tool_name: &str, passed: bool) {
    let val = if passed {
        STATUS_PASS
    } else {
        STATUS_NOT_EXECUTED
    };
    match tool_name {
        "ast-grep" => AST_GREP_STATUS.store(val, Ordering::SeqCst),
        "threadmoth" => THREADMOTH_STATUS.store(val, Ordering::SeqCst),
        "rust-analyzer" => RUST_ANALYZER_STATUS.store(val, Ordering::SeqCst),
        "scip" => SCIP_STATUS.store(val, Ordering::SeqCst),
        _ => {}
    }
}

fn emit_external_proofs_summary() {
    let ag = if AST_GREP_STATUS.load(Ordering::SeqCst) == STATUS_PASS {
        "PASS"
    } else {
        "NOT EXECUTED"
    };
    let tm = if THREADMOTH_STATUS.load(Ordering::SeqCst) == STATUS_PASS {
        "PASS"
    } else {
        "NOT EXECUTED"
    };
    let ra = if RUST_ANALYZER_STATUS.load(Ordering::SeqCst) == STATUS_PASS {
        "PASS"
    } else {
        "NOT EXECUTED"
    };
    let sc = if SCIP_STATUS.load(Ordering::SeqCst) == STATUS_PASS {
        "PASS"
    } else {
        "NOT EXECUTED"
    };

    println!(
        "SEMANTIC EXTERNAL PROOFS:\n  ast-grep: [{ag}]\n  threadmoth: [{tm}]\n  rust-analyzer: [{ra}]\n  scip: [{sc}]"
    );
}

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

/// Enforces the requirement that external tools must be present when OMEN_REQUIRE_SEMANTIC_EXTERNAL_PROOFS=1.
/// In normal portable CI (OMEN_REQUIRE_SEMANTIC_EXTERNAL_PROOFS!=1), external tools like rust-analyzer
/// are never executed opportunistically merely because they exist on PATH, avoiding external startup/indexing latency.
fn require_external_proof(tool_name: &str, is_available: bool) -> bool {
    let required = is_external_acceptance_enabled();
    if !required {
        // Normal portable CI:
        // Rule 1: Normal portable CI must never opportunistically run the real rust-analyzer acceptance proof.
        if tool_name == "rust-analyzer" {
            record_proof_status(tool_name, false);
            emit_external_proofs_summary();
            eprintln!(
                "SKIPPED / NOT EXECUTED: '{tool_name}' real acceptance proof requires OMEN_REQUIRE_SEMANTIC_EXTERNAL_PROOFS=1"
            );
            return false;
        }

        // For other external tools in normal CI, do not execute if unavailable on PATH
        if !is_available {
            record_proof_status(tool_name, false);
            emit_external_proofs_summary();
            eprintln!(
                "SKIPPED / NOT EXECUTED: '{tool_name}' is not available on PATH (set OMEN_REQUIRE_SEMANTIC_EXTERNAL_PROOFS=1 to require)"
            );
            return false;
        }
    } else if !is_available {
        // Rule 2: Dedicated external semantic acceptance must require the real tools.
        // When OMEN_REQUIRE_SEMANTIC_EXTERNAL_PROOFS=1, missing any required tool is a HARD FAILURE.
        panic!(
            "OMEN_REQUIRE_SEMANTIC_EXTERNAL_PROOFS=1 but required external tool '{tool_name}' was not found on PATH or doctor failed"
        );
    }
    true
}

async fn run_with_watchdog<F, T>(test_name: &str, deadline: Duration, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    match tokio::time::timeout(deadline, fut).await {
        Ok(res) => res,
        Err(_) => panic!(
            "{test_name} overall watchdog timed out after {:?}",
            deadline
        ),
    }
}

async fn run_phase<F, T>(test_name: &str, phase_name: &str, limit: Duration, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    match tokio::time::timeout(limit, fut).await {
        Ok(res) => res,
        Err(_) => panic!(
            "{test_name} timed out in phase '{phase_name}' after {:?}",
            limit
        ),
    }
}

// =============================================================================
// PROOF A: Structural search distinguishes syntax from text
// =============================================================================
#[tokio::test]
async fn test_proof_a_structural_search_distinguishes_syntax_from_text() {
    run_with_watchdog("test_proof_a", Duration::from_secs(15), async {
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
        if !require_external_proof("ast-grep", adapter.is_available()) {
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

        record_proof_status("ast-grep", true);
        emit_external_proofs_summary();
    })
    .await;
}

// =============================================================================
// PROOF B: Structural rewrite routes through ThreadMoth
// =============================================================================
#[tokio::test]
async fn test_proof_b_structural_rewrite_routes_through_threadmoth() {
    run_with_watchdog("test_proof_b", Duration::from_secs(15), async {
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
        if !require_external_proof("ast-grep", adapter.is_available()) {
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
        let threadmoth_ok = ThreadMothAdapter::doctor(&supervisor).await.is_ok();
        if !require_external_proof("threadmoth", threadmoth_ok) {
            return;
        }

        // 3. Dispatch through ThreadMothBridge
        let cert =
            ThreadMothBridge::apply_structural_rewrite(&supervisor, temp_dir.path(), candidate)
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

        record_proof_status("ast-grep", true);
        record_proof_status("threadmoth", true);
        emit_external_proofs_summary();
    })
    .await;
}

// =============================================================================
// PROOF C: Real rust-analyzer all-symbol function search, definition, and references
// =============================================================================
#[tokio::test]
async fn test_proof_c_real_lsp_symbol_definition_and_references() {
    if !is_external_acceptance_enabled() {
        require_external_proof("rust-analyzer", false);
        return;
    }

    run_with_watchdog("test_proof_c", Duration::from_secs(25), async {
        let temp_dir = tempfile::tempdir().unwrap();
        let src_dir = temp_dir.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();

        let cargo_toml = r#"[package]
name = "fixture-lsp"
version = "0.1.0"
edition = "2024"
"#;
        std::fs::write(temp_dir.path().join("Cargo.toml"), cargo_toml).unwrap();

        let lib_code = r#"pub fn refresh_token() -> &'static str {
    "token"
}

pub fn caller() -> &'static str {
    refresh_token()
}

pub struct SessionToken;
"#;
        std::fs::write(src_dir.join("lib.rs"), lib_code).unwrap();

        let ra_provider = RustAnalyzerProvider::new(temp_dir.path().to_path_buf());
        if !require_external_proof("rust-analyzer", ra_provider.is_available()) {
            return;
        }

        let symbols = run_phase(
            "test_proof_c",
            "lsp_function_symbol_search",
            Duration::from_secs(8),
            ra_provider.symbol_search("refresh_token", 10),
        )
        .await
        .expect("real rust-analyzer function symbol search must succeed");
        assert!(
            symbols.iter().any(|symbol| {
                symbol.name == "refresh_token"
                    && symbol.kind == omen_semantic::SymbolKind::Function
            }),
            "real rust-analyzer all-symbol search must return refresh_token"
        );

        let definition = run_phase(
            "test_proof_c",
            "lsp_function_definition",
            Duration::from_secs(8),
            ra_provider.symbol_definition("refresh_token", None, None, None),
        )
        .await
        .expect("real rust-analyzer function definition lookup must succeed");
        assert!(
            definition.is_resolved(),
            "real rust-analyzer must resolve refresh_token without a caller-provided location"
        );
        let location = definition.as_resolved().unwrap();
        assert_eq!(location.file, "src/lib.rs");
        assert_eq!(location.range.start_line, 0);
        assert_eq!(location.provider.as_str(), "rust-analyzer");

        let references = run_phase(
            "test_proof_c",
            "lsp_function_references",
            Duration::from_secs(8),
            ra_provider.symbol_references("refresh_token", None, None, None, 10),
        )
        .await
        .expect("real rust-analyzer function references lookup must succeed");
        let references = references
            .as_resolved()
            .expect("real rust-analyzer refresh_token references must resolve");
        assert!(
            references.iter().any(|reference| reference.location.range.start_line == 5),
            "real rust-analyzer references must include the fixture caller"
        );

        let type_definition = run_phase(
            "test_proof_c",
            "lsp_type_regression",
            Duration::from_secs(8),
            ra_provider.symbol_definition("SessionToken", None, None, None),
        )
        .await
        .expect("real rust-analyzer type lookup must succeed");
        assert!(
            type_definition.is_resolved(),
            "all-symbol function repair must not regress ordinary type lookup"
        );

        record_proof_status("rust-analyzer", true);
        emit_external_proofs_summary();
    })
    .await;
}

// =============================================================================
// PROOF D: LSP timeout and late-response are bounded
// =============================================================================
#[tokio::test]
async fn test_proof_d_lsp_timeout_and_late_response_are_bounded() {
    run_with_watchdog("test_proof_d", Duration::from_secs(15), async {
        let temp_dir = tempfile::tempdir().unwrap();
        let gremlin = gremlin_exe();

        // 1. Test timeout bounds using HostileLspServer in slow-init mode
        run_phase(
            "test_proof_d",
            "hostile_slow_init",
            Duration::from_secs(3),
            async {
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
                let _ = client.shutdown().await;
            },
        )
        .await;

        // 2. Test late-response discard
        run_phase(
            "test_proof_d",
            "hostile_late_response",
            Duration::from_secs(3),
            async {
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
                let _ = client_late.shutdown().await;
            },
        )
        .await;

        // 3. Test HugeResponse 16 MiB hard-cap bounded rejection
        run_phase(
            "test_proof_d",
            "hostile_huge_response",
            Duration::from_secs(3),
            async {
                let client_huge_res = omen_adapters::HostileLspServer::spawn_client(
                    &gremlin,
                    omen_adapters::lsp_fixture::HostileLspMode::HugeResponse,
                    temp_dir.path().to_path_buf(),
                )
                .await;

                assert!(client_huge_res.is_ok());
                let mut client_huge = client_huge_res.unwrap();
                let _ = client_huge.initialize(Duration::from_millis(200)).await;

                let huge_req_res = client_huge
                    .send_request(
                        "textDocument/definition",
                        serde_json::json!({}),
                        Duration::from_millis(1000),
                    )
                    .await;

                assert!(
                    huge_req_res.is_err(),
                    "Huge LSP response (>16 MiB) must be rejected"
                );
                let err_str = huge_req_res.unwrap_err().to_string();
                assert!(
                    err_str.contains("exceeded maximum allowed"),
                    "Error must explicitly explain hard-cap violation: {err_str}"
                );
                let _ = client_huge.shutdown().await;
            },
        )
        .await;
    })
    .await;
}

// =============================================================================
// =============================================================================
// PROOF E: SCIP genuine Protobuf index and stale-index detection
// =============================================================================
#[tokio::test]
async fn test_proof_e_scip_symbol_query_and_stale_index_detection() {
    run_with_watchdog("test_proof_e", Duration::from_secs(10), async {
        let temp_dir = tempfile::tempdir().unwrap();
        let src_dir = temp_dir.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();

        let auth_path = src_dir.join("auth.rs");
        std::fs::write(&auth_path, "pub fn refresh_token() -> bool { true }\n").unwrap();

        // Genuine SCIP Protobuf index
        let index = scip_proto::Index {
            metadata: Some(scip_proto::Metadata {
                version: 1,
                tool_info: None,
                project_root: temp_dir.path().to_string_lossy().to_string(),
                text_document_encoding: 1,
            }),
            documents: vec![scip_proto::Document {
                language: "rust".into(),
                relative_path: "src/auth.rs".into(),
                occurrences: vec![scip_proto::Occurrence {
                    range: vec![0, 7, 0, 20],
                    symbol: "crates/auth/refresh_token".into(),
                    symbol_roles: 1,
                    override_documentation: vec![],
                    syntax_kind: 0,
                }],
                symbols: vec![],
                text: String::new(),
            }],
            external_symbols: vec![],
        };

        let index_bytes = ScipProvider::encode_index(&index);
        let index_file = temp_dir.path().join("index.scip");
        std::fs::write(&index_file, index_bytes).unwrap();

        let provider =
            ScipProvider::load_from_file(temp_dir.path().to_path_buf(), index_file).unwrap();
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

        // 4. Stale query must return Stale and never Resolved
        let stale_def = provider
            .symbol_definition("refresh_token", None, None, None)
            .await
            .unwrap();
        assert!(
            stale_def.is_stale(),
            "Stale SCIP index must return Stale lookup result"
        );
        assert!(
            !stale_def.is_resolved(),
            "Stale lookup result must never report Resolved"
        );

        record_proof_status("scip", true);
        emit_external_proofs_summary();
    })
    .await;
}

// =============================================================================
// PROOF F: Semantic result invalidates after source change
// =============================================================================
#[tokio::test]
async fn test_proof_f_semantic_result_invalidates_after_source_change() {
    run_with_watchdog("test_proof_f", Duration::from_secs(10), async {
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
    })
    .await;
}

// =============================================================================
// PROOF G: Cargo workspace metadata is canonical
// =============================================================================
#[tokio::test]
async fn test_proof_g_cargo_workspace_metadata_is_canonical() {
    run_with_watchdog("test_proof_g", Duration::from_secs(10), async {
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
    })
    .await;
}

// =============================================================================
// PROOF H: Two non-Rust ecosystems produce semantic metadata
// =============================================================================
#[tokio::test]
async fn test_proof_h_two_non_rust_ecosystems_produce_semantic_metadata() {
    run_with_watchdog("test_proof_h", Duration::from_secs(10), async {
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
    })
    .await;
}

// =============================================================================
// PROOF I: Deterministic symbol question uses zero model calls & records semantic calls
// =============================================================================
#[tokio::test]
async fn test_proof_i_deterministic_symbol_question_uses_zero_model_calls() {
    run_with_watchdog("test_proof_i", Duration::from_secs(10), async {
        let temp_dir = tempfile::tempdir().unwrap();
        let src_dir = temp_dir.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();

        let auth_path = src_dir.join("auth.rs");
        std::fs::write(&auth_path, "pub fn refresh_token() -> bool { true }\n").unwrap();

        // Create binary SCIP index so symbol resolves
        let index = scip_proto::Index {
            metadata: None,
            documents: vec![scip_proto::Document {
                language: "rust".into(),
                relative_path: "src/auth.rs".into(),
                occurrences: vec![scip_proto::Occurrence {
                    range: vec![0, 7, 0, 20],
                    symbol: "refresh_token".into(),
                    symbol_roles: 1,
                    override_documentation: vec![],
                    syntax_kind: 0,
                }],
                symbols: vec![],
                text: String::new(),
            }],
            external_symbols: vec![],
        };
        let index_bytes = ScipProvider::encode_index(&index);
        std::fs::write(temp_dir.path().join("index.scip"), index_bytes).unwrap();

        let session_id = omen_core::InteractiveSessionId::new("sess_test_i").unwrap();

        // Ask unambiguous question: "where is refresh_token defined?"
        let output = run_phase(
            "test_proof_i",
            "deterministic_symbol_dispatch",
            Duration::from_secs(5),
            async {
                AiLaneDispatcher::dispatch_with_workspace(
                    "? where is refresh_token defined?",
                    &session_id,
                    temp_dir.path(),
                    temp_dir.path(),
                    None,
                    None,
                    None,
                )
            },
        )
        .await
        .unwrap();

        // Invariant: ai_provider_calls == 0 and semantic_provider_calls >= 1
        assert_eq!(
            output.stats.provider_calls, 0,
            "Deterministic symbol query must never invoke AI model (provider_calls must be 0)"
        );
        assert!(
            output.stats.semantic_provider_calls >= 1,
            "Semantic provider query must be recorded (semantic_provider_calls must be >= 1)"
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
    })
    .await;
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

// =============================================================================
// PROOF K: Multilingual coordinate normalization across UTF-8, UTF-16, and AST spans
// =============================================================================
#[test]
fn test_proof_k_multilingual_coordinate_normalization() {
    // String contains:
    // 'a' -> 1 UTF-8 byte, 1 UTF-16 code unit
    // 'é' -> 2 UTF-8 bytes, 1 UTF-16 code unit
    // '字' -> 3 UTF-8 bytes, 1 UTF-16 code unit
    // '🦀' -> 4 UTF-8 bytes, 2 UTF-16 code units (surrogate pair)
    // 'z' -> 1 UTF-8 byte, 1 UTF-16 code unit
    let line = "aé字🦀z";

    // Byte offsets in UTF-8:
    // 'a': 0..1
    // 'é': 1..3
    // '字': 3..6
    // '🦀': 6..10
    // 'z': 10..11
    assert_eq!(line.len(), 11);

    // UTF-16 code unit offsets:
    // 'a': 0..1
    // 'é': 1..2
    // '字': 2..3
    // '🦀': 3..5
    // 'z': 5..6
    assert_eq!(line.encode_utf16().count(), 6);

    // Verify UTF-8 -> UTF-16 conversion
    assert_eq!(utf8_to_lsp_utf16_col(line, 0), 0); // before 'a'
    assert_eq!(utf8_to_lsp_utf16_col(line, 1), 1); // after 'a', before 'é'
    assert_eq!(utf8_to_lsp_utf16_col(line, 3), 2); // after 'é', before '字'
    assert_eq!(utf8_to_lsp_utf16_col(line, 6), 3); // after '字', before '🦀'
    assert_eq!(utf8_to_lsp_utf16_col(line, 10), 5); // after '🦀', before 'z'
    assert_eq!(utf8_to_lsp_utf16_col(line, 11), 6); // end of line

    // Verify UTF-16 -> UTF-8 conversion
    assert_eq!(lsp_utf16_to_utf8_col(line, 0), 0);
    assert_eq!(lsp_utf16_to_utf8_col(line, 1), 1);
    assert_eq!(lsp_utf16_to_utf8_col(line, 2), 3);
    assert_eq!(lsp_utf16_to_utf8_col(line, 3), 6);
    assert_eq!(lsp_utf16_to_utf8_col(line, 5), 10);
    assert_eq!(lsp_utf16_to_utf8_col(line, 6), 11);
}

// =============================================================================
// =============================================================================
// PROOF L: Product path wiring through interactive shell actions and AI lane
// =============================================================================
#[tokio::test]
async fn test_proof_l_product_path_wiring_through_def_action_and_lane() {
    run_with_watchdog("test_proof_l", Duration::from_secs(25), async {
        let temp_dir = tempfile::tempdir().unwrap();
        let src_dir = temp_dir.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();

        let cargo_toml = r#"[package]
name = "fixture-product"
version = "0.1.0"
edition = "2021"
"#;
        std::fs::write(temp_dir.path().join("Cargo.toml"), cargo_toml).unwrap();

        let lib_code = r#"pub struct SessionToken {
    pub id: String,
}
"#;
        std::fs::write(src_dir.join("lib.rs"), lib_code).unwrap();

        // Verify that get_workspace_semantic_registry registers providers for workspace
        let reg = omen_adapters::get_workspace_semantic_registry(temp_dir.path());
        let providers = reg.list_providers();
        assert!(
            providers
                .iter()
                .any(|p| p.0.as_str().contains("cargo") || p.1.contains("cargo")),
            "Product registry must register cargo provider for Rust crate workspace"
        );

        let has_ra = providers.iter().any(|p| p.0.as_str() == "rust-analyzer");

        if !require_external_proof("rust-analyzer", has_ra) {
            return;
        }

        // Poll definition via registry to allow rust-analyzer to finish background loading
        let found = run_phase(
            "test_proof_l",
            "lsp_symbol_definition_discovery",
            Duration::from_secs(15),
            async {
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(14) {
                    if let Ok(res) = reg
                        .find_definition(
                            "SessionToken",
                            Some("src/lib.rs"),
                            Some(0),
                            Some(11),
                            None,
                        )
                        .await
                        && res.is_resolved()
                    {
                        return true;
                    }
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                false
            },
        )
        .await;

        assert!(
            found,
            "Registry must resolve SessionToken via live rust-analyzer"
        );

        let _syms = run_phase(
            "test_proof_l",
            "lsp_symbol_search",
            Duration::from_secs(5),
            reg.symbol_search("SessionToken", Some(10), None),
        )
        .await;

        // Product path dispatch: "? where is SessionToken defined?"
        let session_id = omen_core::InteractiveSessionId::new("sess_test_l").unwrap();
        let output = run_phase(
            "test_proof_l",
            "product_ai_lane_dispatch",
            Duration::from_secs(5),
            async {
                AiLaneDispatcher::dispatch_with_workspace(
                    "? where is SessionToken defined?",
                    &session_id,
                    temp_dir.path(),
                    temp_dir.path(),
                    None,
                    None,
                    None,
                )
            },
        )
        .await
        .unwrap();

        assert_eq!(output.stats.provider_calls, 0);
        assert!(output.stats.semantic_provider_calls >= 1);
        assert!(
            output.response_text.contains("SessionToken"),
            "Product lane response must resolve SessionToken"
        );
        assert!(
            output.references.iter().any(|r| r.contains("SessionToken")),
            "Product lane response must emit @symbol://SessionToken reference"
        );

        record_proof_status("rust-analyzer", true);
        emit_external_proofs_summary();
    })
    .await;
}

// =============================================================================
// REGRESSION TEST: Harness watchdog triggers and cleans up on stalled dependency
// =============================================================================
#[tokio::test]
async fn test_harness_watchdog_triggers_and_cleans_up_on_stalled_dependency() {
    let result = tokio::spawn(async {
        run_phase(
            "test_mock_stalled_lsp",
            "awaiting_stalled_response",
            Duration::from_millis(200),
            async {
                // Deliberately simulate an uncooperative/hung LSP server or subprocess
                tokio::time::sleep(Duration::from_secs(60)).await;
            },
        )
        .await;
    })
    .await;

    // The inner task must panic due to phase timeout
    assert!(
        result.is_err(),
        "Hung operation must be aborted by phase timeout"
    );
    let panic_err = result.unwrap_err();
    assert!(panic_err.is_panic(), "Expected panic from phase timeout");

    let panic_msg = if let Ok(msg) = panic_err.try_into_panic() {
        if let Some(s) = msg.downcast_ref::<String>() {
            s.clone()
        } else if let Some(s) = msg.downcast_ref::<&str>() {
            s.to_string()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    assert!(
        panic_msg.contains(
            "test_mock_stalled_lsp timed out in phase 'awaiting_stalled_response' after 200ms"
        ),
        "Panic message must report exact phase and duration: {panic_msg}"
    );
}
