# Omen 0.7 Semantic Environment — Implementation Report

**Release**: Omen 0.7.0  
**Target**: Semantic Understanding of Developer Environments  
**Status**: VERIFIED & COMPLETE  
**Repository**: `https://github.com/matthewjameswatkins1978-cyber/Omen.git`  
**Branch**: `feature/omen-0.7-semantic-environment`  
**Accepted 0.6 Baseline**: `478021eb55caadb1d83e890ba8b16bcf6ea8ef4b`  

---

## 1. Executive Summary

Omen 0.2 established machine truth: typed resources, structured execution contracts, Fact Registry, dirty/current truth, Tool Atlas, CAS storage, and process containment.  
Omen 0.3 provided an interactive human shell: 3-lane grammar, non-blocking fact completion, subordinate physical history, and semantic blocks.  
Omen 0.4 introduced the shared runtime daemon (`omend`), coordinating shared facts, session recovery, and managed services.  
Omen 0.5 established agent interoperability: standard MCP server, agent-inside-Omen interactive experience, and typed proposed actions without terminal scraping.  
Omen 0.6 established physical execution maturity: pluggable execution backends, first-class daemon PTY engine, process tree ownership via Windows Job Objects and POSIX process groups, physical leases, and truthful platform assurance.  

**Omen 0.7 gives the runtime semantic understanding of developer environments.**

Headline doctrine:
> **Understand more than commands.**

Today Omen no longer just says:
```text
cargo test failed
src/auth.rs changed
this process is running
this Fact is dirty
this artifact contains stderr
```

0.7 truthfully and deterministically says:
```text
this diagnostic belongs to function refresh_token
this symbol is defined at src/auth.rs:12:8
these seven call sites reference it
this structural pattern occurs in four functions across three files
this workspace contains four packages with nineteen runnable tasks
```

Omen 0.7 delivers:
1. **Zero-Model Semantic Queries**: Semantic questions (`? where is refresh_token defined?`, `? what references SessionToken?`, `? what packages exist?`) resolve deterministically via the semantic hierarchy with zero LLM model calls (`provider_calls = 0`).
2. **Deterministic Provider Hierarchy**: `ripgrep -> ast-grep -> LSP (rust-analyzer) / SCIP -> ThreadMoth -> native tool metadata -> fallback execution`. The cheapest, highest-confidence truthful provider is always queried first.
3. **Hot Keystroke Path Isolation**: In-memory completion cache for `@symbol://...` and `@package://...` with zero I/O, zero subprocess spawns, zero LSP requests, and zero network calls, executing in < 5ms.
4. **Targeted Witness Invalidation**: `SemanticWitness` tracks file content hashes (`CURRENT -> DIRTY`), guaranteeing lazy pessimism without silent fallbacks or stale facts.
5. **Structural Pattern Search & ThreadMoth Mutation Bridge**: True AST-aware syntax queries using `ast-grep` that strictly distinguish code from comments and string literals, bridging structural rewrites to ThreadMoth with pre/post hash validation and mutation certificates.
6. **Bounded LSP Substrate**: Stdio JSON-RPC transport for `rust-analyzer` with strict timeouts, cancellation protocol (`$/cancelRequest`), and late-response discarding without thread blocking or leak.
7. **Multi-Ecosystem Package Semantics**: Canonical package metadata, dependency graphs, and task extraction for Cargo, npm, Python uv/pyproject, Go modules, Docker, and GitHub CLI.
8. **First-Class MCP Semantic Tools**: 5 new typed tools exposed over Model Context Protocol (`omen_symbol_search`, `omen_symbol_definition`, `omen_symbol_references`, `omen_structure_search`, `omen_package_query`).

---

## 2. Core Architectural Doctrine

> **Omen is substrate, not sovereign.**

The surrounding division of responsibility remains absolute:
- **Lantern** knows (durable memory, long-term context, and provenance).
- **Resolve** coordinates (live guard tokens and scope locks).
- **Tethers** controls (permission, trusted capability identity, policy, approval, durable intent, replay, and provider outcome truth).
- **Omen** makes the machine legible and enforceable (physical execution, containment, typed resources, machine-readable facts, and artifact CAS evidence).
- **ThreadMoth** deterministically mutates (bounded structural edits with cryptographic pre/post hashes and refusal).

Compact system statement:
> **Lantern knows. Resolve coordinates. Tethers controls. Omen makes the machine legible and enforceable. ThreadMoth mutates deterministically.**

Omen is **not** an IDE and **not** a language server. Omen consumes external language and structural substrates (`rust-analyzer`, `ast-grep`, `scip`, `cargo`, `npm`, `uv`, `go`) and normalizes their outputs into typed, bounded, structured facts that AI agents and human operators can inspect with zero hallucination risk.

---

## 3. Implemented Deliverables by Slice

### Slice A: Vocabulary & Resource Model (`crates/omen-core`, `crates/omen-schema`)
- Extended `ResourceKind` with `ResourceKind::Symbol` (`@symbol://...`) and `ResourceKind::Package` (`@package://...`).
- Added strongly typed identifiers: `SemanticProviderId` and `SymbolId`.
- Updated wire schema models and unit tests ensuring forward and backward serialization stability.

### Slice B: Semantic Subsystem & Caches (`crates/omen-semantic`)
- Core models: `SourceRange`, `SourceLocation`, `SymbolKind`, `SymbolRecord`, `ReferenceRecord`, `StructuralMatch`, `SemanticDiagnostic`, `PackageRecord`, `PackageDependency`, `TargetRecord`, `TaskRecord`.
- `SemanticLookupResult<T>`: Strict variants `Resolved(T)`, `NotFound`, `Ambiguous(Vec<SymbolRecord>)`, `Stale(T)`, `Unsupported`. Enforces that ambiguous lookups never collapse to an arbitrary first match.
- `SemanticWitness` & `SemanticCache`: Generational caching (`SemanticGeneration`) with file SHA-256 witness validation. Modifying an underlying source file flips witness state from `CURRENT` to `DIRTY`.
- `SemanticProviderRegistry`: Routing layer prioritizing Live (`LSP`) -> Indexed (`SCIP`) -> Fallback providers, with bounded execution timeouts (`tokio::time::timeout`).

### Slice D: Semantic Adapters (`crates/omen-adapters`)
- `AstGrepAdapter`: Structural pattern matching and rewrite candidate derivation via `ast-grep run --json`.
- `ThreadMothBridge`: Validates pre-image hashes, applies deterministic structural mutations via `threadmoth_exact_replace` or `threadmoth_set_value`, and records post-image cryptographic certificates.
- `LspClient` + `RustAnalyzerProvider`: Async JSON-RPC stdio client with `$/cancelRequest`, bounded request timeouts, automatic `textDocument/didOpen` notification, and unblocked late-response discard.
- `ScipProvider`: SCIP index parser verifying index freshness against source files.
- Multi-Ecosystem Package Providers:
  - `CargoSemanticProvider`: Uses `cargo metadata --format-version 1 --no-deps`.
  - `NpmSemanticProvider`: Parses `package.json` dependencies and npm scripts.
  - `PythonUvSemanticProvider`: Parses `pyproject.toml` dependencies and entrypoint scripts.
  - `GoSemanticProvider`: Parses `go.mod` module identity and dependencies.
  - `DockerSemanticProvider`: Inspects `Dockerfile` and `compose.yaml` targets and services.
  - `GitHubCliSemanticProvider`: Inspects repository workflow files (`.github/workflows/*.yml`).

### Slice E: Interactive Experience & Zero-Model AI Lane (`crates/omen-interactive`, `crates/omen-agent`)
- `HotSemanticIndex`: In-memory symbol, package, and task cache populated on startup and background refresh.
- Zero I/O Tab Completion: `OmenCompleter` serves `@symbol://` and `@package://` suggestions in < 5ms without disk reads or network calls.
- Colon Actions: Added `:symbol <query>`, `:def <sym>`, `:refs <sym>`, `:structure <pattern> <lang>`, `:packages`, and `:tasks`.
- AI Lane Deterministic Dispatch: Queries matching `? where is <symbol> defined?`, `? what references <symbol>?`, and package inspection route to `DeterministicClassifier` and execute via `SemanticProviderRegistry`, returning typed responses with `provider_calls = 0`.

### Slice F: Model Context Protocol Semantic Tools (`crates/omen-mcp`)
Registered 5 typed MCP tools:
1. `omen_symbol_search`: Symbol lookup across LSP and SCIP.
2. `omen_symbol_definition`: Precise definition resolution.
3. `omen_symbol_references`: Reference list across workspace files.
4. `omen_structure_search`: AST-based pattern matching.
5. `omen_package_query`: Multi-ecosystem package and task discovery.

---

## 4. Verification & Proof Suite

All 13 proofs in `crates/omen-cli/tests/semantic_environment_proofs.rs` pass completely:

| Proof | Title | Description | Result |
|---|---|---|---|
| **Proof A** | Structural search distinguishes syntax from text | AST match finds 1 real function, text search finds 3 (comment, fn, string) | `PASSED` |
| **Proof B** | Structural rewrite routes through ThreadMoth | `ast-grep` derives rewrite candidate -> ThreadMoth mutation certificate | `PASSED` |
| **Proof C** | Real LSP symbol definition & references | Live `rust-analyzer` JSON-RPC query resolves definition/references in bounded time | `PASSED` |
| **Proof D** | LSP timeout & late-response are bounded | Hostile LSP fixture times out cleanly, late responses discarded safely, 16 MiB cap | `PASSED` |
| **Proof E** | SCIP genuine Protobuf query & stale detection | Queries real Protobuf wire model; file change detects stale index via SHA-256 witness | `PASSED` |
| **Proof F** | Semantic result invalidates after source change | File edit invalidates cached witness (`CURRENT -> DIRTY`), cache refuses stale fact | `PASSED` |
| **Proof G** | Cargo workspace metadata is canonical | Parses workspace packages, targets, and emits canonical `package://cargo/<name>` | `PASSED` |
| **Proof H** | Two non-Rust ecosystems produce metadata | `package.json` (npm) and `pyproject.toml` (python) produce typed package records | `PASSED` |
| **Proof I** | Deterministic question uses zero model calls | `? where is refresh_token defined?` returns definition with `provider_calls = 0` | `PASSED` |
| **Proof J** | Hot completion uses zero provider I/O | In-memory tab completion completes `@symbol://` items with 0 I/O in < 5ms | `PASSED` |
| **Proof K** | Multilingual coordinate normalization | Normalized UTF-8 bytes to LSP UTF-16 code units across ASCII, Latin, CJK, and emoji | `PASSED` |
| **Proof L** | Product path wiring through def and lane | Shared registry routes `:def` and `? where is SessionToken defined?` to live LSP | `PASSED` |
| **Harness Watchdog** | Hard test deadline & clean phase timeout | Proves harness watchdog triggers in 200ms with exact phase on uncooperative fixture | `PASSED` |

### Acceptance Veto Repairs:
1. **SCIP Genuine Protobuf Ingestion**: Fully migrated `crates/omen-adapters/src/scip.rs` to real Protobuf wire model using `prost`. Replaced mock JSON parsing with genuine binary Protobuf decoder and index encoder. Stale indexes report `SemanticLookupResult::Stale(T)`, never `Resolved(T)`.
2. **Real LSP Provider Product Wiring**: Unified registry builder (`build_workspace_semantic_registry` and `get_workspace_semantic_registry`) shared across CLI, interactive actions, MCP, and tests. Eliminated registry divergence across subsystems.
3. **LSP Hard-Cap & Diagnostics**: Hard-capped maximum message size to 16 MiB (`DEFAULT_MAX_LSP_MESSAGE_BYTES`), discarding hostile payloads before allocation. Real LSP diagnostics ingestion mapping severities and UTF-16 code units to UTF-8 columns.
4. **Coordinate Normalization**: Implemented bidirectional `utf8_to_lsp_utf16_col` and `lsp_utf16_to_utf8_col` conversions verified across 1-byte, 2-byte, 3-byte, and 4-byte surrogate-pair emoji coordinates.
5. **Case-Preserving Symbol Extraction**: `DeterministicClassifier` preserves exact casing from user queries (`SessionToken`), avoiding case mangling on case-sensitive languages.
6. **Test Harness Hardening**: Layered phase timeouts (`run_phase`) and outer watchdogs (`run_with_watchdog`) applied across all integration proof tests. Background task cancellation and child process killing on shutdown and drop prevent orphan processes or Windows locks.
7. **30 Omen AI-First Engineering Rules**: Formally incorporated into `AGENTS.md` and `docs/evidence/0.7/ARCHITECTURE_DOCTRINE.md`.

### Full Verification Results:
- `cargo fmt --check`: PASSED
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASSED
- `cargo test --workspace`: PASSED (all unit and integration tests across 16 crates)
- `OMEN_REQUIRE_SEMANTIC_EXTERNAL_PROOFS=1 cargo test -p omen-cli --test semantic_environment_proofs`: PASSED (all 13 proofs passing)
- `cargo deny check`: PASSED (`advisories ok, bans ok, licenses ok, sources ok`)
- `cargo run -p xtask -- verify`: PASSED

