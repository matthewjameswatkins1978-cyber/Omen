# Omen Canonical Roadmap — The Road to 1.0

> **Doctrine**: *Omen is substrate, not sovereign.*  
> **Standards Doctrine**: *Stable canonical Omen semantics internally; protocol adapters at the edges; zero protocol-specific coupling inside the core substrate.*

> For the complete architectural rationale, standards doctrine, research findings, and detailed release charters, see **[Road to 1.0](ROAD_TO_1_0.md)**.

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
- **Theme**: *One Omen reality across processes.*
- **Daemon Architecture (`omend`)**: Small local runtime daemon managing shared physical execution, monitoring, and state.
- **Cross-Process Coordination**: Named pipes (Windows) and Unix domain sockets (Linux/macOS) for shared physical runtime/session coordination only. Resolve remains sovereign for live guards, scope locks, fencing, and conflict admission.
- **Shared State**: Shared Facts with live invalidation, shared subordinate physical history, and a shared hot semantic index cache across concurrent shells.
- **Process & Services**: Process registry, named service lifecycle (`proc://`), session recovery, and daemon disconnect/recovery semantics.
- **Success Criteria**: *Two Omen shells and an agent can look at the same workspace and see the same underlying machine truth.*

### Omen 0.5: Agent Interoperability
- **Theme**: *Stop agents using terminals as eyes.*
- **General Interoperability Boundary**: Edge protocol adapters (MCP, A2A, LSP, repo instructions). MCP is an edge adapter, not Omen's internal architecture.
- **Zero Core Protocol Coupling**: Core Omen substrate remains pure and unpolluted by external protocol quirks.
- **Capabilities**: Tool/resource discovery, execution requests, Fact queries/subscriptions, process inspection, CAS context pagination, structured errors, explicit standards version negotiation.
- **Compatibility Profiles & Interop Sets**: Declarative manifests validating protocol conformance before activation (no authority grant).
- **Success Criteria**: *A capable coding agent using Omen should need raw terminal scraping dramatically less often.*

### Omen 0.6: Physical Maturity
- **Theme**: *Make physical execution genuinely mature.*
- **Pluggable Execution Backends**: Native host, WSL, container, microVM sandbox, remote runtime.
- **Interactive & Process Capabilities**: First-class PTY execution, attachable/resumable sessions, process-tree ownership, service leases.
- **Hardened Containment**: Linux Landlock/pidfd/cgroups v2/namespaces; Windows Job Objects/restricted tokens/AppContainer; macOS Endpoint Security.
- **Truthful Platform Assurance**: Unforgeable reporting (`ENFORCED`, `MEDIATED`, `OBSERVED`, `BEST_EFFORT`, `UNSUPPORTED`).
- **Secrets Handles**: Strict separation of `secret.use` from `secret.expose`.
- **Success Criteria**: *Omen can supervise normal developer workloads, hostile fixtures, services and interactive programs while truthfully describing what the platform actually enforced.*

### Omen 0.7: Semantic Environment
- **Theme**: *Understand more than commands.*
- **Structural Analysis & Metadata**: Ast-grep adapter for AST search/rewrites, compiler metadata extraction.
- **Language Intelligence**: LSP and SCIP integration for symbol and reference navigation (`symbol://crate/auth/refresh_token`).
- **Tool Ecosystem**: Richer domain understanding of Cargo/Rust, npm/pnpm, Python/uv, Go, Docker, GitHub CLI. Descriptive grammar ingestion (Carapace/Fig) without authority grant.
- **Success Criteria**: *Omen increasingly understands what developer actions mean, not merely which executables launched.*

### Omen 0.8: Composition
- **Theme**: *Make repeated work concise without inventing another programming language.*
- **Project Configuration (`Omen.toml`)**: Optional declarative workspace configuration (identity, known services, named checks, common actions, adapter config). No policy; does not replace package manifests.
- **Composition Engine**: Named actions combining checks, typed value routing, large output handling via CAS artifacts and bounded previews.
- **Authority Invariant**: Omen may reconstruct typed execution requests/compositions from recorded subordinate evidence, but all execution is re-admitted under current Tethers authority. Omen does not own authoritative replay.
- **Success Criteria**: *Most everyday developer command chains become clearer and safer without Omen growing loops, classes, modules and another package manager.*

### Omen 0.9: Stabilisation
- **Theme**: *Stop adding clever things and make everything boringly dependable.*
- **Hardening & Verification**: Protocol and wire schema freeze, database migrations, crash recovery, corruption handling, CAS integrity.
- **Security & Torture**: Fuzzing, path traversal, symlink/junction/mount escape, secret leakage, terminal escape injection, malicious workspace metadata, adapter trust, local IPC authentication.
- **Success Criteria**: *We deliberately struggle to break it before allowing it to call itself 1.0.*

### Omen 1.0: Stable Human + Agent Developer Runtime
- **Theme**: *A stable, production-grade commitment.*
- **Production Standard**: Unified substrate for human developers and autonomous AI coding agents across Windows, Linux, and macOS.
- **Qualities**: Human quality (low-friction daily driver), agent quality (semantic operation without scraping), shared reality (identical machine truth without history contamination), cross-platform, AI-optional, and backed by reproducible evidence.


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
