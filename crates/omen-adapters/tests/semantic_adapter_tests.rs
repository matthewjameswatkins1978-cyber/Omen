use omen_adapters::{
    CargoSemanticProvider, GoSemanticProvider, NpmSemanticProvider, PythonUvSemanticProvider,
    RustAnalyzerProvider, ScipProvider,
};
use omen_semantic::{ProviderKind, SemanticLookupResult, SemanticProvider, SymbolKind};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[test]
fn test_npm_semantic_provider_parse() {
    let raw = r#"{
        "name": "my-service",
        "version": "1.2.3",
        "scripts": {
            "build": "tsc -b",
            "test": "jest"
        },
        "dependencies": {
            "express": "^4.18.2"
        },
        "devDependencies": {
            "typescript": "^5.0.0"
        }
    }"#;
    let pkg = NpmSemanticProvider::parse_package_json(raw, Path::new("package.json")).unwrap();
    assert_eq!(pkg.name, "my-service");
    assert_eq!(pkg.version, "1.2.3");
    assert_eq!(pkg.ecosystem, "npm");
    assert_eq!(pkg.dependencies.len(), 2);
    assert_eq!(pkg.tasks.len(), 2);
    assert_eq!(pkg.uri.as_str(), "package://npm/my-service");
}

#[test]
fn test_python_uv_semantic_provider_parse() {
    let raw = r#"
[project]
name = "ml-core"
version = "0.4.1"
dependencies = [
    "torch>=2.0.0",
    "numpy"
]

[project.scripts]
serve = "ml_core.server:main"
"#;
    let pkg = PythonUvSemanticProvider::parse_pyproject(raw, Path::new("pyproject.toml")).unwrap();
    assert_eq!(pkg.name, "ml-core");
    assert_eq!(pkg.version, "0.4.1");
    assert_eq!(pkg.ecosystem, "python");
    assert_eq!(pkg.dependencies.len(), 2);
    assert!(pkg.tasks.iter().any(|t| t.name == "serve"));
    assert_eq!(pkg.uri.as_str(), "package://python/ml-core");
}

#[test]
fn test_go_semantic_provider_parse() {
    let raw = r#"
module github.com/acme/backend

go 1.22

require (
    github.com/gin-gonic/gin v1.9.1
    golang.org/x/sync v0.6.0
)
"#;
    let pkg = GoSemanticProvider::parse_go_mod(raw, Path::new("go.mod")).unwrap();
    assert_eq!(pkg.name, "github.com/acme/backend");
    assert_eq!(pkg.version, "1.22");
    assert_eq!(pkg.ecosystem, "go");
    assert_eq!(pkg.dependencies.len(), 2);
    assert_eq!(pkg.tasks.len(), 2);
    assert_eq!(pkg.uri.as_str(), "package://go/github_com_acme_backend");
}

#[test]
fn test_cargo_semantics_parse() {
    let raw = r#"{
        "packages": [
            {
                "name": "omen-demo",
                "version": "0.1.0",
                "manifest_path": "/workspace/Cargo.toml",
                "dependencies": [
                    {
                        "name": "serde",
                        "req": "^1.0",
                        "kind": null
                    }
                ],
                "targets": [
                    {
                        "name": "omen-demo",
                        "kind": ["bin"],
                        "src_path": "/workspace/src/main.rs"
                    }
                ]
            }
        ]
    }"#;
    let pkgs = CargoSemanticProvider::parse_metadata(raw).unwrap();
    assert_eq!(pkgs.len(), 1);
    let pkg = &pkgs[0];
    assert_eq!(pkg.name, "omen-demo");
    assert_eq!(pkg.version, "0.1.0");
    assert_eq!(pkg.ecosystem, "cargo");
    assert_eq!(pkg.dependencies.len(), 1);
    assert_eq!(pkg.targets.len(), 1);
    assert_eq!(pkg.tasks.len(), 3);
    assert_eq!(pkg.uri.as_str(), "package://cargo/omen-demo");
}

#[tokio::test]
async fn test_scip_provider_symbol_and_staleness() {
    use omen_adapters::scip_proto;

    let index = scip_proto::Index {
        metadata: None,
        documents: vec![scip_proto::Document {
            language: "rust".into(),
            relative_path: "src/auth.rs".into(),
            occurrences: vec![
                scip_proto::Occurrence {
                    range: vec![10, 4, 10, 17],
                    symbol: "crates/auth/refresh_token".into(),
                    symbol_roles: 1,
                    override_documentation: vec![],
                    syntax_kind: 0,
                },
                scip_proto::Occurrence {
                    range: vec![25, 8, 25, 21],
                    symbol: "crates/auth/refresh_token".into(),
                    symbol_roles: 0,
                    override_documentation: vec![],
                    syntax_kind: 0,
                },
            ],
            symbols: vec![],
            text: String::new(),
        }],
        external_symbols: vec![],
    };

    let temp_dir = tempfile::tempdir().unwrap();
    let auth_file = temp_dir.path().join("src/auth.rs");
    std::fs::create_dir_all(auth_file.parent().unwrap()).unwrap();
    std::fs::write(&auth_file, "pub fn refresh_token() {}\n").unwrap();

    let index_bytes = ScipProvider::encode_index(&index);
    let index_file = temp_dir.path().join("index.scip");
    std::fs::write(&index_file, index_bytes).unwrap();

    let provider = ScipProvider::load_from_file(temp_dir.path().to_path_buf(), index_file).unwrap();
    assert_eq!(provider.kind(), ProviderKind::Indexed);
    assert!(!provider.is_stale());

    let def_res = provider
        .symbol_definition("crates/auth/refresh_token", None, None, None)
        .await
        .unwrap();
    assert!(def_res.is_resolved());

    // Mutate source file to invalidate hash witness
    std::fs::write(&auth_file, "pub fn refresh_token_modified() {}\n").unwrap();
    assert!(provider.is_stale());

    // When stale, must report Stale and not Resolved
    let stale_res = provider
        .symbol_definition("crates/auth/refresh_token", None, None, None)
        .await
        .unwrap();
    assert!(stale_res.is_stale(), "Stale index must return Stale result");
    assert!(!stale_res.is_resolved());
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

    let status = std::process::Command::new("cargo")
        .args(["build", "-p", "omen-test-fixtures", "--bin", "omen-gremlin"])
        .status()
        .expect("failed to invoke cargo for omen-gremlin");
    assert!(
        status.success(),
        "failed to build omen-gremlin test fixture"
    );
    assert!(fallback.exists(), "omen-gremlin fixture was not produced");
    fallback
}

fn rust_semantic_fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        r#"[package]
name = "omen-semantic-fixture"
version = "0.1.0"
edition = "2024"
"#,
    )
    .unwrap();
    std::fs::write(
        temp.path().join("src/lib.rs"),
        r#"pub fn refresh_token() -> &'static str { "token" }

pub fn caller() -> &'static str { refresh_token() }

pub fn duplicate() {}

pub mod other { pub fn duplicate() {} }

pub struct SessionToken;
"#,
    )
    .unwrap();
    temp
}

fn gremlin_provider(workspace: &Path, mode: &str) -> RustAnalyzerProvider {
    RustAnalyzerProvider::with_binary_args(
        workspace.to_path_buf(),
        gremlin_exe(),
        vec!["--lsp-mode".into(), mode.into()],
    )
}

#[tokio::test]
async fn rust_analyzer_all_symbol_lookup_finds_functions_and_preserves_ambiguity() {
    let temp = rust_semantic_fixture();
    let provider = gremlin_provider(temp.path(), "semantic-filtered");

    let symbols = provider.symbol_search("refresh_token", 10).await.unwrap();
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "refresh_token");
    assert_eq!(symbols[0].kind, SymbolKind::Function);

    let definition = provider
        .symbol_definition("refresh_token", None, None, None)
        .await
        .unwrap();
    let location = definition
        .as_resolved()
        .expect("function definition must resolve through all-symbol lookup");
    assert_eq!(location.file, "src/lib.rs");
    assert_eq!(location.range.start_line, 0);

    let references = provider
        .symbol_references("refresh_token", None, None, None, 10)
        .await
        .unwrap();
    let references = references
        .as_resolved()
        .expect("function references must resolve through all-symbol lookup");
    assert!(
        references
            .iter()
            .any(|reference| reference.location.range.start_line == 2),
        "known refresh_token call site must be returned"
    );

    let duplicate = provider
        .symbol_definition("duplicate", None, None, None)
        .await
        .unwrap();
    match duplicate {
        SemanticLookupResult::Ambiguous(candidates) => assert_eq!(candidates.len(), 2),
        other => panic!("duplicate exact names must remain ambiguous, got {other:?}"),
    }

    let type_definition = provider
        .symbol_definition("SessionToken", None, None, None)
        .await
        .unwrap();
    assert!(
        type_definition.is_resolved(),
        "broadening workspace lookup must not regress type resolution"
    );
}

#[tokio::test]
async fn rust_analyzer_all_symbol_lookup_uses_hash_fallback_without_extension() {
    let temp = rust_semantic_fixture();
    let provider = gremlin_provider(temp.path(), "semantic-fallback");

    let symbols = provider.symbol_search("refresh_token", 10).await.unwrap();
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "refresh_token");
    assert_eq!(symbols[0].kind, SymbolKind::Function);

    let definition = provider
        .symbol_definition("refresh_token", None, None, None)
        .await
        .unwrap();
    assert!(
        definition.is_resolved(),
        "documented rust-analyzer # fallback must remain internal and resolve functions"
    );
}

#[tokio::test]
async fn rust_analyzer_readiness_waits_for_quiescent_status() {
    let temp = rust_semantic_fixture();
    let provider = RustAnalyzerProvider::with_binary_args_and_readiness_timeout(
        temp.path().to_path_buf(),
        gremlin_exe(),
        vec!["--lsp-mode".into(), "ready-sequence".into()],
        Duration::from_millis(500),
    );

    let symbols = provider.symbol_search("refresh_token", 10).await.unwrap();
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "refresh_token");
}

#[tokio::test]
async fn rust_analyzer_readiness_timeout_is_explicit_and_not_not_found() {
    let temp = rust_semantic_fixture();
    let provider = RustAnalyzerProvider::with_binary_args_and_readiness_timeout(
        temp.path().to_path_buf(),
        gremlin_exe(),
        vec!["--lsp-mode".into(), "never-quiescent".into()],
        Duration::from_millis(100),
    );

    let error = provider
        .symbol_search("refresh_token", 10)
        .await
        .expect_err("never-quiescent provider must fail readiness");
    let text = error.to_string();
    assert!(text.contains("provider=rust-analyzer"));
    assert!(text.contains("phase=readiness.wait"));
    assert!(text.contains("health=ok"));
    assert!(text.contains("quiescent=false"));
    assert!(text.contains("deadline_ms=100"));
}

#[tokio::test]
async fn rust_analyzer_warning_status_can_be_ready_and_is_preserved() {
    let temp = rust_semantic_fixture();
    let provider = RustAnalyzerProvider::with_binary_args_and_readiness_timeout(
        temp.path().to_path_buf(),
        gremlin_exe(),
        vec!["--lsp-mode".into(), "warning-ready".into()],
        Duration::from_millis(500),
    );

    let symbols = provider.symbol_search("refresh_token", 10).await.unwrap();
    assert_eq!(symbols.len(), 1);
    let status = provider
        .latest_server_status()
        .await
        .expect("latest server status must be retained");
    assert_eq!(status.health.as_deref(), Some("warning"));
    assert_eq!(status.quiescent, Some(true));
    assert_eq!(status.message.as_deref(), Some("usable with warning"));
}

#[tokio::test]
async fn rust_analyzer_exit_before_readiness_is_distinct_from_not_found() {
    let temp = rust_semantic_fixture();
    let provider = RustAnalyzerProvider::with_binary_args_and_readiness_timeout(
        temp.path().to_path_buf(),
        gremlin_exe(),
        vec!["--lsp-mode".into(), "exit-before-ready".into()],
        Duration::from_millis(500),
    );

    let error = provider
        .symbol_search("refresh_token", 10)
        .await
        .expect_err("provider exit before readiness must fail");
    let text = error.to_string();
    assert!(text.contains("provider=rust-analyzer"));
    assert!(text.contains("phase=readiness.wait"));
    assert!(text.contains("reason=server_status_channel_closed"));
    assert!(!text.contains("not_found"));
}
