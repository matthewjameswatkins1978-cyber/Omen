//! Omen tool adapters (ThreadMoth, Cargo, Git, ripgrep, AST-grep, LSP, SCIP, Ecosystem).

pub mod ast_grep;
pub mod cargo;
pub mod cargo_semantics;
pub mod composition_executor;
pub mod ecosystem;
pub mod git;
pub mod lsp;
pub mod lsp_fixture;
pub mod registry_builder;
pub mod ripgrep;
pub mod scip;
pub mod threadmoth;
pub mod threadmoth_bridge;

pub use ast_grep::AstGrepAdapter;
pub use cargo::{
    CargoAdapter, CargoCheckResult, CargoTestResult, CompilerDiagnostic, CompilerDiagnosticSpan,
};
pub use cargo_semantics::CargoSemanticProvider;
pub use composition_executor::{
    CapabilityExecutor, CapabilityRequest, DefaultCapabilityExecutor, execute_plan,
};
pub use ecosystem::{
    DockerSemanticProvider, GitHubCliSemanticProvider, GoSemanticProvider, NpmSemanticProvider,
    PythonUvSemanticProvider,
};
pub use git::{GitAdapter, GitStatusResult};
pub use lsp::{LspClient, RustAnalyzerProvider};
pub use lsp_fixture::HostileLspServer;
pub use registry_builder::{build_workspace_semantic_registry, get_workspace_semantic_registry};
pub use ripgrep::{RipgrepAdapter, RipgrepMatch, RipgrepSearchResult};
pub use scip::{ScipProvider, proto as scip_proto};
pub use threadmoth::{
    ThreadMothAdapter, ThreadMothBudget, ThreadMothByteRange, ThreadMothCardinality,
    ThreadMothCertificate, ThreadMothNamespace, ThreadMothOperationWrapper, ThreadMothRequest,
};
pub use threadmoth_bridge::ThreadMothBridge;
