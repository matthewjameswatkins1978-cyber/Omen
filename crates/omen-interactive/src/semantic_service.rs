use omen_adapters::get_workspace_semantic_registry;
use omen_semantic::SemanticProviderRegistry;
use std::path::Path;
use std::sync::Arc;

/// Gets or initializes the canonical workspace semantic provider registry.
pub fn build_semantic_registry(workspace_root: &Path) -> Arc<SemanticProviderRegistry> {
    get_workspace_semantic_registry(workspace_root)
}
