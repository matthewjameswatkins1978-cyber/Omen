use crate::types::*;
use omen_core::{CoreError, SemanticProviderId};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Classification of semantic provider nature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Live process-backed intelligence (e.g. LSP / rust-analyzer).
    Live,
    /// Indexed offline symbol database (e.g. SCIP).
    Indexed,
    /// Syntax-aware structural pattern engine (e.g. ast-grep).
    Structural,
    /// Native build tool / package manager metadata (e.g. Cargo, npm, uv, go).
    Ecosystem,
}

/// Declared capabilities of a semantic provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub symbol_search: bool,
    pub definition: bool,
    pub references: bool,
    pub structural_search: bool,
    pub diagnostics: bool,
    pub packages: bool,
}

/// Result of a semantic lookup that strictly respects ambiguity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticLookupResult<T> {
    /// Unambiguously resolved to an exact record.
    Resolved(T),
    /// Ambiguous candidates found — never guess or collapse!
    Ambiguous(Vec<SymbolRecord>),
    /// Requested symbol not found.
    NotFound,
    /// Requested operation not supported by this provider.
    Unsupported,
}

impl<T> SemanticLookupResult<T> {
    pub fn is_resolved(&self) -> bool {
        matches!(self, Self::Resolved(_))
    }

    pub fn is_ambiguous(&self) -> bool {
        matches!(self, Self::Ambiguous(_))
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound)
    }

    pub fn as_ref(&self) -> SemanticLookupResult<&T> {
        match self {
            Self::Resolved(val) => SemanticLookupResult::Resolved(val),
            Self::Ambiguous(c) => SemanticLookupResult::Ambiguous(c.clone()),
            Self::NotFound => SemanticLookupResult::NotFound,
            Self::Unsupported => SemanticLookupResult::Unsupported,
        }
    }
}

/// The core pluggable semantic provider trait.
pub trait SemanticProvider: Send + Sync {
    fn id(&self) -> SemanticProviderId;
    fn name(&self) -> &str;
    fn kind(&self) -> ProviderKind;
    fn capabilities(&self) -> ProviderCapabilities;
    fn is_available(&self) -> bool;

    fn symbol_search<'a>(
        &'a self,
        _query: &'a str,
        _limit: usize,
    ) -> BoxFuture<'a, Result<Vec<SymbolRecord>, CoreError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn symbol_definition<'a>(
        &'a self,
        _symbol: &'a str,
        _file: Option<&'a str>,
        _line: Option<usize>,
        _col: Option<usize>,
    ) -> BoxFuture<'a, Result<SemanticLookupResult<SourceLocation>, CoreError>> {
        Box::pin(async { Ok(SemanticLookupResult::Unsupported) })
    }

    fn symbol_references<'a>(
        &'a self,
        _symbol: &'a str,
        _file: Option<&'a str>,
        _line: Option<usize>,
        _col: Option<usize>,
        _limit: usize,
    ) -> BoxFuture<'a, Result<SemanticLookupResult<Vec<ReferenceRecord>>, CoreError>> {
        Box::pin(async { Ok(SemanticLookupResult::Unsupported) })
    }

    fn structural_search<'a>(
        &'a self,
        _pattern: &'a str,
        _language: &'a str,
        _limit: usize,
    ) -> BoxFuture<'a, Result<Vec<StructuralMatch>, CoreError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn diagnostics<'a>(&'a self) -> BoxFuture<'a, Result<Vec<SemanticDiagnostic>, CoreError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn packages<'a>(&'a self) -> BoxFuture<'a, Result<Vec<PackageRecord>, CoreError>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}
