pub mod cache;
pub mod provider;
pub mod registry;
pub mod types;

pub use cache::{CachedEntry, SemanticCache, SemanticWitness};
pub use provider::{
    BoxFuture, ProviderCapabilities, ProviderKind, SemanticLookupResult, SemanticProvider,
};
pub use registry::{
    DEFAULT_LIVE_SEMANTIC_TIMEOUT, DEFAULT_RESULT_LIMIT, DEFAULT_SEMANTIC_TIMEOUT,
    SemanticProviderRegistry, WorkspaceCoverage,
};
pub use types::{
    DiagnosticSeverity, PackageDependency, PackageRecord, ReferenceRecord,
    SEMANTIC_RESULT_SCHEMA_VERSION, SemanticCoverage, SemanticDefinitionData, SemanticDiagnostic,
    SemanticGeneration, SemanticHint, SemanticOperation, SemanticOutcome, SemanticReferencesData,
    SemanticResult, SemanticSearchData, SemanticTargetKey, SourceLocation, SourceRange,
    StructuralMatch, SymbolKind, SymbolRecord, TargetRecord, TaskRecord,
};
