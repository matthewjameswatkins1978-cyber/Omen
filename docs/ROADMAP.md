# Omen Canonical Roadmap — The Road to 1.0

> **Doctrine**: *Omen is substrate, not sovereign.*  
> **Standards Doctrine**: *Stable canonical Omen semantics internally; protocol adapters at the edges; zero protocol-specific coupling inside the core substrate.*

---

## 1. Sequence of Releases

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  Omen 0.2    │ ──> │  Omen 0.3    │ ──> │  Omen 0.4    │ ──> │  Omen 0.5    │
│Machine Truth │     │Human Interf. │     │Shared Runtime│     │Agent Interop │
└──────────────┘     └──────────────┘     └──────────────┘     └──────────────┘
       │                    │                    │                    │
       ▼                    ▼                    ▼                    ▼
┌──────────────┐     ┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  Omen 0.6    │ ──> │  Omen 0.7    │ ──> │  Omen 0.8    │ ──> │  Omen 0.9    │
│Phys. Maturity│     │Semantic Env. │     │ Composition  │     │Stabilisation │
└──────────────┘     └──────────────┘     └──────────────┘     └──────────────┘
                                                                      │
                                                                      ▼
                                                               ┌──────────────┐
                                                               │   Omen 1.0   │
                                                               │Stable Runtime│
                                                               └──────────────┘
```

### Omen 0.2: Machine Truth (Complete)
- [x] M0: Workspace bootstrap, toolchain pinning, CI scaffolding.
- [x] M1: Core domain types, typed IDs (`ResourceId`, `FactId`), URI grammar, strict wire schemas.
- [x] M2: Knowledge engine, SQLite WAL persistence, Fact Registry, lazy pessimism, CAS artifact store.
- [x] M3: Execution engine, argv-only spawning, closed stdin, bounded context, Job Objects containment, `omen-gremlin`.
- [x] M4: Tool Atlas, TOML runtime profiles, controlled probing validator.
- [x] M5: Machine-readable Git and ripgrep adapters.
- [x] M6: ThreadMoth adapter with pre/post-image hashing and structural change detection.
- [x] M7: Cargo adapter with JSON diagnostic streaming.
- [x] M8: Full End-to-End proofs (auth fixture & gremlin torture).
- [x] M9: Tethers execution contract fixture.
- [x] M10: CLI subcommands and 0.2 Implementation Report.

### Omen 0.3: Human Interface (Complete)
- [x] H0: Architecture & dependency freeze (Reedline, Crossterm, nu-ansi-term, fuzzy-matcher).
- [x] H1: Interactive shell core, terminal lifecycle, prompt rendering, invocation-specific child handoff.
- [x] H2: Semantic grammar (`:action`, `@reference`, `? query`), cross-platform Windows path parsing.
- [x] H3: Fact-aware non-blocking completion, hot semantic index, ghost hinter.
- [x] H4: Subordinate physical history, unique `InteractiveSessionId`, reference resolution (`@last`, `@failed`).
- [x] H5: Progressive diagnostics (Levels 0–3), causal `:why` fact provenance trees.
- [x] H6: Semantic blocks & terminal integration (OSC 7, OSC 8, OSC 133 / FTCS).
- [x] H7: Blast radius preflight & multiline Paste Guard.
- [x] H8: Services UX, managed processes, `proc://` URIs.
- [x] H9: Optional AI reasoning lane boundary with deterministic local fallback.
- [x] H10: Human/Agent shared-reality proof (dirty fact awareness without session history contamination).
- [x] H11: Performance benchmarks (genuine cold-start + hot completion), golden snapshots, evidence package.

### Omen 0.4: Shared Runtime
- **Daemon Architecture (`omend`)**: Background daemon managing shared physical execution and monitoring.
- **Cross-Process Coordination**: Named pipes (Windows) and Unix domain sockets (Linux/macOS).
- **Shared Hot Semantic Index**: Promotes 0.3 per-session hot index to a high-throughput, lock-free cross-session cache.
- **Cross-Session Subordinate History**: Multi-agent / multi-human concurrency over single repository substrate.
- **Terminal Lifecycle Refinements**: Kitty keyboard protocol support, cursor position reporting.

### Omen 0.5: Agent Interoperability
- **Standards-Based Edge Projections**: Protocol adapters projecting Omen to external agents (e.g. Model Context Protocol / MCP, Language Server Protocol / LSP, Agent-to-Agent / A2A).
- **Zero Core Protocol Coupling**: Core Omen substrate remains pure and unpolluted by external protocol quirks.
- **Compatibility Profiles & Interop Sets**: Declarative manifests validating protocol conformance before activation.
- **Transport Independence**: Stdio, IPC, WebSocket, and HTTP/SSE transports for edge adapters.

### Omen 0.6: Physical Maturity
- **Hardened Containment**: Linux Landlock v3/v4 fine-tuning, cgroups v2 resource controllers.
- **Windows Sandboxing**: AppContainer profiles, restricted tokens, Job Object nested limits.
- **macOS Sandboxing**: Endpoint Security framework integration, sandbox-exec profiles.
- **Truthful Platform Matrix**: Unforgeable platform enforcement reporting (`ENFORCED` vs `OBSERVED`).

### Omen 0.7: Semantic Environment
- **Ast-Grep Adapter**: Structural code search and pattern matching.
- **Carapace Grammar Ingestion**: Dynamic argument completion ingestion for thousands of CLI tools.
- **Rich Semantic Schemas**: Workspace-wide AST and symbol fact generation.

### Omen 0.8: Composition
- **Multi-Tool Pipeline Composition**: Typed pipelining between Tool Atlas profiles.
- **Transactional Workspace Snapshots**: Ephemeral Git worktrees and CAS change sets.
- **Deterministic Workflows**: Replayable execution envelopes.

### Omen 0.9: Stabilisation
- **API & Wire Schema Freeze**: Long-term compatibility guarantees for `v1.0`.
- **Fuzzing & Torture Testing**: High-concurrency Gremlin stress testing across multi-platform matrix.
- **Security Audit**: Memory safety, secret redaction, and symlink escape proofs.

### Omen 1.0: Stable Human + Agent Developer Runtime
- **Production Standard**: The unified substrate for human developers and autonomous AI coding agents.

---

## 2. Standards & Interoperability Doctrine

### 2.1 The Three Independent Axes
Omen strictly separates three independent architectural axes:
1. **Canonical Semantics**: Internal domain types (`ExecutionRequest`, `ExecutionResult`, `FactRecord`, `ResourceUri`).
2. **Protocol Binding**: Translation layer mapping internal semantics to external standards (MCP tools/resources, LSP, POSIX).
3. **Transport**: Physical communication medium (stdio, named pipes, Unix domain sockets, HTTP/SSE).

```
┌─────────────────────────────────────────────────────────────┐
│                    Canonical Semantics                      │
│      (ExecutionRequest, FactRecord, ResourceUri, CAS)       │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                      Protocol Binding                       │
│              (MCP Adapter, LSP Adapter, CLI)                │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                          Transport                          │
│             (Stdio, Named Pipe, UDS, HTTP/SSE)              │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 Adapter Lifecycle Distinction
- **Adapter Package**: Portable distribution unit (manifest, wire schema, executable).
- **Installed Adapter**: Locally verified, uncompressed adapter in workspace storage.
- **Exact Binding**: Active running instance bound to a specific session and transport channel.

### 2.3 Compatibility Profiles / Interop Sets
- External consumers declare exact required capabilities (e.g. `interop: [tools.v1, facts.read.v2]`).
- Interop sets do **not** grant trust or authority (trust is governed strictly by Tethers).
- Conformance is verified before protocol activation.
