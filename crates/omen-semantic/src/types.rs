use omen_core::{ResourceUri, SemanticProviderId, SymbolId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// 0-indexed internal source range with 1-indexed human display representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceRange {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

impl SourceRange {
    pub fn new(start_line: usize, start_col: usize, end_line: usize, end_col: usize) -> Self {
        Self {
            start_line,
            start_col,
            end_line,
            end_col,
        }
    }

    pub fn point(line: usize, col: usize) -> Self {
        Self {
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: col,
        }
    }

    /// 1-based human representation (e.g. "L10:C5" or "L10:C5-L12:C20")
    pub fn to_human_string(&self) -> String {
        if self.start_line == self.end_line && self.start_col == self.end_col {
            format!("{}:{}", self.start_line + 1, self.start_col + 1)
        } else if self.start_line == self.end_line {
            format!(
                "{}:{}-{}",
                self.start_line + 1,
                self.start_col + 1,
                self.end_col + 1
            )
        } else {
            format!(
                "{}:{}-{}:{}",
                self.start_line + 1,
                self.start_col + 1,
                self.end_line + 1,
                self.end_col + 1
            )
        }
    }

    pub fn contains(&self, line: usize, col: usize) -> bool {
        if line < self.start_line || line > self.end_line {
            return false;
        }
        if line == self.start_line && col < self.start_col {
            return false;
        }
        if line == self.end_line && col > self.end_col {
            return false;
        }
        true
    }
}

impl fmt::Display for SourceRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_human_string())
    }
}

/// Generation tracker for semantic validity and invalidation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SemanticGeneration {
    pub workspace_generation: u64,
    pub provider_generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_digest: Option<String>,
}

impl SemanticGeneration {
    pub fn new(workspace_generation: u64, provider_generation: u64) -> Self {
        Self {
            workspace_generation,
            provider_generation,
            index_digest: None,
        }
    }

    pub fn with_digest(
        workspace_generation: u64,
        provider_generation: u64,
        digest: impl Into<String>,
    ) -> Self {
        Self {
            workspace_generation,
            provider_generation,
            index_digest: Some(digest.into()),
        }
    }
}

/// Canonical source location with workspace-relative file path, range, provider, and generation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceLocation {
    pub file: String,
    pub range: SourceRange,
    pub provider: SemanticProviderId,
    pub generation: SemanticGeneration,
}

impl SourceLocation {
    pub fn new(
        file: impl Into<String>,
        range: SourceRange,
        provider: SemanticProviderId,
        generation: SemanticGeneration,
    ) -> Self {
        Self {
            file: file.into(),
            range,
            provider,
            generation,
        }
    }

    pub fn to_human_string(&self) -> String {
        format!("{}:{}", self.file, self.range.to_human_string())
    }
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_human_string())
    }
}

/// Canonical symbol kinds recognized by Omen.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Enum,
    Interface,
    Trait,
    Variable,
    Constant,
    Module,
    Package,
    Class,
    Property,
    Field,
    Constructor,
    TypeParameter,
    Other(String),
}

impl SymbolKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Interface => "interface",
            Self::Trait => "trait",
            Self::Variable => "variable",
            Self::Constant => "constant",
            Self::Module => "module",
            Self::Package => "package",
            Self::Class => "class",
            Self::Property => "property",
            Self::Field => "field",
            Self::Constructor => "constructor",
            Self::TypeParameter => "type_parameter",
            Self::Other(s) => s.as_str(),
        }
    }
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Canonical symbol record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolRecord {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub location: SourceLocation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    pub uri: ResourceUri,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
}

/// Canonical reference / call-site record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceRecord {
    pub symbol_id: SymbolId,
    pub location: SourceLocation,
    pub is_definition: bool,
    pub is_write: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

/// Structural pattern match from syntax analyzers (ast-grep).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralMatch {
    pub file: String,
    pub range: SourceRange,
    pub matched_text: String,
    pub metavariables: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
}

/// Normalized diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl DiagnosticSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Information => "information",
            Self::Hint => "hint",
        }
    }
}

/// Canonical semantic diagnostic normalized from compilers or language servers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticDiagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub location: SourceLocation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_locations: Vec<SourceLocation>,
    pub provider: SemanticProviderId,
}

/// Canonical package metadata record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRecord {
    pub name: String,
    pub version: String,
    pub ecosystem: String,
    pub manifest_path: String,
    #[serde(default)]
    pub dependencies: Vec<PackageDependency>,
    #[serde(default)]
    pub targets: Vec<TargetRecord>,
    #[serde(default)]
    pub tasks: Vec<TaskRecord>,
    pub uri: ResourceUri,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageDependency {
    pub name: String,
    pub req: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetRecord {
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub name: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
