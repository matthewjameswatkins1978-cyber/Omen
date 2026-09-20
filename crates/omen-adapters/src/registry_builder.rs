use crate::{
    AstGrepAdapter, CargoSemanticProvider, DockerSemanticProvider, GitHubCliSemanticProvider,
    GoSemanticProvider, NpmSemanticProvider, PythonUvSemanticProvider, RustAnalyzerProvider,
    ScipProvider,
};
use omen_semantic::{SemanticProvider, SemanticProviderRegistry};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

static REGISTRIES: LazyLock<Mutex<HashMap<PathBuf, Arc<SemanticProviderRegistry>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Returns or initializes the shared workspace semantic registry for `workspace_root`.
pub fn get_workspace_semantic_registry(workspace_root: &Path) -> Arc<SemanticProviderRegistry> {
    let mut guard = REGISTRIES.lock().unwrap();
    let root = workspace_root.to_path_buf();
    if let Some(reg) = guard.get(&root) {
        return reg.clone();
    }
    let reg = Arc::new(build_workspace_semantic_registry(workspace_root));
    guard.insert(root, reg.clone());
    reg
}

/// Builds the canonical workspace semantic provider registry.
///
/// Shared across interactive shell, MCP server, and tests.
pub fn build_workspace_semantic_registry(workspace_root: &Path) -> SemanticProviderRegistry {
    let mut reg = SemanticProviderRegistry::new(workspace_root.to_path_buf());

    // Live LSP (rust-analyzer)
    let ra = RustAnalyzerProvider::new(workspace_root.to_path_buf());
    if ra.is_available() {
        reg.register(Arc::new(ra));
    }

    // AST-grep structural search & rewrite adapter
    let ast_grep = AstGrepAdapter::new(workspace_root.to_path_buf());
    if ast_grep.is_available() {
        reg.register(Arc::new(ast_grep));
    }

    // Cargo semantics
    let cargo = CargoSemanticProvider::new(workspace_root);
    if cargo.is_available() {
        reg.register(Arc::new(cargo));
    }

    // NPM semantics
    let npm = NpmSemanticProvider::new(workspace_root);
    if npm.is_available() {
        reg.register(Arc::new(npm));
    }

    // Python / uv semantics
    let py = PythonUvSemanticProvider::new(workspace_root);
    if py.is_available() {
        reg.register(Arc::new(py));
    }

    // Go semantics
    let go = GoSemanticProvider::new(workspace_root);
    if go.is_available() {
        reg.register(Arc::new(go));
    }

    // Docker semantics
    let docker = DockerSemanticProvider::new(workspace_root);
    if docker.is_available() {
        reg.register(Arc::new(docker));
    }

    // GitHub CLI semantics
    let gh = GitHubCliSemanticProvider::new(workspace_root);
    if gh.is_available() {
        reg.register(Arc::new(gh));
    }

    // Check for index.scip in workspace root or .omen directory
    for candidate in &[
        workspace_root.join("index.scip"),
        workspace_root.join(".omen/index.scip"),
    ] {
        if candidate.exists()
            && let Ok(scip) =
                ScipProvider::load_from_file(workspace_root.to_path_buf(), candidate.clone())
        {
            reg.register(Arc::new(scip));
            break;
        }
    }

    reg
}
