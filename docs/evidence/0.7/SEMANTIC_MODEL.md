# Omen 0.7 Semantic Environment — Semantic Model & Type Inventory

## 1. Overview

Omen 0.7 introduces typed semantic resources to make code symbols, syntactic structures, packages, and workspace tasks first-class citizens in Omen's machine-readable runtime.

## 2. Resource Kind Extensions

In `crates/omen-core/src/types.rs`:
```rust
pub enum ResourceKind {
    File,
    Directory,
    Process,
    Artifact,
    Fact,
    Execution,
    Service,
    PtySession,
    Symbol,   // Added in 0.7: @symbol://<name> or symbol://<ecosystem>/<path>#<symbol>
    Package,  // Added in 0.7: @package://<ecosystem>/<name>
}
```

### URI Schemes
- **Symbol URI**: `@symbol://<name>` or `symbol://<crate_or_module>/<file>#<symbol>`
  - Example: `@symbol://refresh_token`
  - Example: `symbol://omen_auth/src/auth.rs#refresh_token`
- **Package URI**: `@package://<ecosystem>/<name>`
  - Example: `package://cargo/omen-core`
  - Example: `package://npm/omen-web-dashboard`
  - Example: `package://python/omen-ml-pipeline`
  - Example: `package://go/github.com/omen/engine`

## 3. Strongly Typed Identifiers

- `SemanticProviderId`: Strongly typed identifier for semantic providers (`"rust-analyzer"`, `"ast-grep"`, `"scip"`, `"cargo"`, `"npm"`, `"uv"`, `"go"`).
- `SymbolId`: Strongly typed identifier for symbol records, preventing raw string interchanging.

## 4. Semantic Subsystem Types (`crates/omen-semantic`)

### Source Range and Location
```rust
pub struct SourceRange {
    pub start_line: usize,  // 0-indexed
    pub start_col: usize,   // 0-indexed
    pub end_line: usize,    // 0-indexed
    pub end_col: usize,     // 0-indexed
}

pub struct SourceLocation {
    pub file: String,
    pub range: SourceRange,
    pub provider_id: SemanticProviderId,
    pub generation: SemanticGeneration,
}
```

### Symbol Records & Kinds
```rust
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Interface,
    Trait,
    Module,
    Variable,
    Constant,
    TypeAlias,
    Field,
    Package,
    Other(String),
}

pub struct SymbolRecord {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub location: SourceLocation,
    pub container_name: Option<String>,
    pub signature: Option<String>,
    pub uri: ResourceUri,
    pub documentation: Option<String>,
}
```

### References & Structural Matches
```rust
pub struct ReferenceRecord {
    pub symbol_id: SymbolId,
    pub location: SourceLocation,
    pub is_definition: bool,
    pub is_write: bool,
    pub snippet: Option<String>,
}

pub struct StructuralMatch {
    pub file: String,
    pub range: SourceRange,
    pub matched_text: String,
    pub metavariables: HashMap<String, String>,
    pub rule_id: Option<String>,
}
```

### Packages, Targets & Tasks
```rust
pub struct PackageRecord {
    pub name: String,
    pub version: String,
    pub ecosystem: String,
    pub manifest_path: String,
    pub dependencies: Vec<PackageDependency>,
    pub targets: Vec<TargetRecord>,
    pub tasks: Vec<TaskRecord>,
    pub uri: ResourceUri,
}

pub struct PackageDependency {
    pub name: String,
    pub req: String,
    pub optional: bool,
}

pub struct TargetRecord {
    pub name: String,
    pub kind: String,
    pub src_path: String,
}

pub struct TaskRecord {
    pub name: String,
    pub command: String,
    pub description: Option<String>,
}
```

### Semantic Lookup Result
```rust
pub enum SemanticLookupResult<T> {
    Resolved(T),
    NotFound,
    Ambiguous(Vec<SymbolRecord>),
    Stale(T),
    Unsupported,
}
```
**Doctrine**: An ambiguous symbol lookup must *never* silently collapse to the first candidate. It must report `Ambiguous(candidates)` so calling agents or operators can disambiguate.

## 5. Witness & Cache Model

```rust
pub struct SemanticWitness {
    pub relative_path: String,
    pub file_hash: String, // SHA-256
    pub observed_at: DateTime<Utc>,
}

pub enum WitnessValidity {
    Current,
    Dirty,
    Stale,
}
```
- A cached symbol or reference result links to `Vec<SemanticWitness>`.
- If any witness file is modified on disk or invalidated via `:dirty`, the cached fact transitions immediately to `Dirty`.
- When `Current` is required, Omen refuses to serve `Dirty` facts, enforcing explicit re-querying.
