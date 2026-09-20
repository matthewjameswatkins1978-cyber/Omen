use omen_adapters::{
    AstGrepAdapter, CargoSemanticProvider, DockerSemanticProvider, GitHubCliSemanticProvider,
    GoSemanticProvider, NpmSemanticProvider, PythonUvSemanticProvider, ScipProvider,
};
use omen_semantic::{SemanticProvider, SemanticProviderRegistry};
use std::path::Path;
use std::sync::Arc;

pub fn build_semantic_registry(workspace_root: &Path) -> SemanticProviderRegistry {
    let mut reg = SemanticProviderRegistry::new(workspace_root.to_path_buf());

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
