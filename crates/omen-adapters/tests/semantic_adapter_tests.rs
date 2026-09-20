use omen_adapters::{
    CargoSemanticProvider, GoSemanticProvider, NpmSemanticProvider, PythonUvSemanticProvider,
    ScipProvider,
};
use omen_semantic::{ProviderKind, SemanticProvider};
use std::path::Path;

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
