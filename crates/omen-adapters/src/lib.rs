//! Omen tool adapters (ThreadMoth, Cargo, Git, ripgrep).

pub mod cargo;
pub mod git;
pub mod ripgrep;
pub mod threadmoth;

pub use cargo::{
    CargoAdapter, CargoCheckResult, CargoTestResult, CompilerDiagnostic, CompilerDiagnosticSpan,
};
pub use git::{GitAdapter, GitStatusResult};
pub use ripgrep::{RipgrepAdapter, RipgrepMatch, RipgrepSearchResult};
pub use threadmoth::{
    ThreadMothAdapter, ThreadMothBudget, ThreadMothByteRange, ThreadMothCardinality,
    ThreadMothCertificate, ThreadMothNamespace, ThreadMothOperationWrapper, ThreadMothRequest,
};
