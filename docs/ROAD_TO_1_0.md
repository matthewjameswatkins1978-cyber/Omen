# Omen — The Road to 1.0

> **Canonical Architectural & Product Charter**  
> *Authoritative Direction Agreed by Matthew and Lucy for Omen through 1.0*  
> *Supersedes any speculative future direction or roadmap assumptions.*

---

## 1. Core Definition & Architectural Law

### Definition
Omen is:
> **A deterministic execution and state runtime for AI agents, with a low-friction human shell interface. It turns operating-system actions into typed, capability-scoped transactions and returns facts, diffs and references instead of terminal noise.**

Broader formulation:
> **Omen is an agent-native operating environment for developer tools. It gives humans and AI the same workspace through different interfaces, while making execution typed, observable, capability-scoped, reversible and context-efficient.**

Core insights:
> **Do not teach the AI to operate a terminal better. Remove the terminal from the machine-facing side of the relationship.**

> **Omen is substrate, not sovereign.**

Omen must not slowly become the agent, policy system, memory system, scheduler, authority system, IDE, operating system or autonomous workflow brain. Its responsibility is to make physical machine reality legible and executable.

### System Boundaries (Inviolable Division of Responsibility)

```text
                  ┌──────────────┐
                  │   LANTERN    │ Durable memory, long-term context, provenance
                  │   "Knows"    │
                  └──────┬───────┘
                         │
                  ┌──────▼───────┐
                  │   RESOLVE    │ Live guard tokens, scope locks, fencing, conflict admission
                  │ "Coordinates"│
                  └──────┬───────┘
                         │
                  ┌──────▼───────┐
                  │   TETHERS    │ Policy, permission, capability identity, approval, intent, outcome truth
                  │  "Controls"  │
                  └──────┬───────┘
                         │
        ═════════════════╪═════════════════  Boundary Line
                         │
                  ┌──────▼───────┐
                  │     OMEN     │ Physical execution mechanics, containment, typed resources, Facts, CAS
                  │ "Makes Legible│
                  │& Enforceable"│
                  └──────┬───────┘
                         │
                  ┌──────▼───────┐
                  │  THREADMOTH  │ Deterministic bounded structural mutation & refusal
                  │  "Mutates"   │
                  └──────────────┘
```

> **Lantern knows. Resolve coordinates. Tethers controls. Omen makes the machine legible and enforceable. ThreadMoth mutates deterministically.**

#### Invariant Rules:
1. **Resolve coordinates; Resolve admission cannot grant Tethers permission.** Omen shared-runtime coordination (`omend`) must never grow into a competing lock or guard authority.
2. **Tethers controls authority; Omen reports enforceability.** Omen must never create a second permission model, capability trust model, approval system, or authoritative replay mechanism.
3. **Never blur these boundaries.** Never create competing policy, replay, approval, or journaling systems inside Omen.

---

## 2. Engineering Doctrine & Cross-Platform Invariants

### AI-First Engineering Rules
1. Consequential truth must be explicit.
2. One semantic authority for each kind of truth.
3. Reasoning should be bounded.
4. Prefer simple explicit structures over cleverness.
5. Important knowledge must be discoverable.
6. Constraints belong below the model.
7. Prefer deterministic transforms and local reasoning.
8. Use rigid schemas and strong types.
9. Reduce dependency entropy; isolate capabilities.
10. Tests and evidence matter more than confident prose.
11. Non-amplification: uncertain input must not silently become stronger truth.

### Operational Doctrine for Omen
- **Never lie by abstraction.**
- **Infer what has already been decided. Never invent what has not.**
- **Pretty must never make slow.**
- **Omen should be intelligent before it is AI-powered.**
- **Stable inside, adaptable outside.**
- **Translate differences. Never hide them.**

### Cross-Platform by Default
Unless Matthew explicitly designates an item as Windows-only, all ordinary Omen code, grammar parsing, and tests are **cross-platform by default**.
Do not hard-code:
- PowerShell;
- Windows drive assumptions;
- Bash;
- shell quoting conventions;
- platform-specific process semantics;
into ordinary portable code and tests. Platform-specific behaviour belongs strictly behind explicit platform boundaries.

---

## 3. Sequence of Releases

```text
0.2  Machine Truth           (Complete)
      ↓
0.3  Human Interface         (Complete)
      ↓
0.4  Shared Runtime          (Next)
      ↓
0.5  Agent Interoperability
      ↓
0.6  Physical Maturity
      ↓
0.7  Semantic Environment
      ↓
0.8  Composition
      ↓
0.9  Stabilisation
      ↓
1.0  Stable Human + Agent Developer Runtime
```

The numbers are not a countdown. Each release earns the next.

---

## 4. Standards & Interoperability Doctrine

The AI ecosystem will not converge on a single universal standard. It is already stratifying into distinct layers:
- **MCP** for agent/tool/resource interaction;
- **A2A** for agent-agent communication;
- **AGENTS.md** for repository instructions;
- Emerging asset, skill, and model exchange standards;
- Unforeseen future standards.

Therefore:
> **Interoperate at the edges. Keep semantics sovereign in the core.**

External protocol types must never leak throughout `omen-core`. The internal model remains strictly protocol-neutral.

### 4.1 Canonical Interoperability Shape

```text
Omen canonical semantics
        ↓
protocol binding / adapter
        ↓
transport
```

Examples:
```text
Omen Tool/Resource semantics
        ↓
MCP binding
        ↓
stdio / IPC / HTTP
```

```text
Omen task/event projection
        ↓
A2A binding
        ↓
HTTP
```

- Changing transport must not change capability meaning.
- Changing protocol must not redefine Omen internal meaning.

> **Omen owns the meaning. Adapters speak the dialect. Transports carry the message. Profiles declare what combination is wanted.**

### 4.2 Tethers-Derived Adapter Architecture

Omen adapts the proven structural pattern of Tethers Plugs while respecting the strict authority boundary:

- **Adapter Package**: Portable distribution unit. Declares protocol family, versions, schemas, translation surfaces, transport support, and conformance fixtures.
- **Installed Adapter**: Machine-local admitted instance with exact version, digest, and runtime state.
- **Binding**: Exact live relationship between installed adapter, protocol version, extensions, transport, configuration, and session.
- **Compatibility Profile / Interop Set**: Declares exact interoperability requirements (e.g. `agent-interop-2026` requiring `mcp.tools 2026-07-28`, `mcp.resources 2026-07-28`, `a2a.tasks 1.0`).

> **A profile does NOT grant authority.** It only declares what interoperability surface is required. Tethers still owns trust and permission.

### 4.3 Adapter Lifecycle

```text
inspect ──> validate ──> conform ──> install ──> bind ──> active
```

Non-active states:
- `stale`
- `incompatible`
- `degraded`
- `unavailable`
- `disabled`
- `quarantined`

Conformance proves protocol and translation behavior. It does not establish authority.

### 4.4 Independent Version Axes

Never collapse versioning into a vague "Omen supports latest MCP". Track each axis independently:
1. Omen product version
2. Canonical Omen schema/SPI version
3. Adapter package format
4. Adapter version
5. External protocol version
6. Extension version
7. Transport-binding version

No silent "latest". Version negotiation must be explicit. Unsupported required semantics must fail clearly.

### 4.5 The 10-Step Adapter Conformance Crucible

Before claiming support for an external standard:
1. **Inspect** package without executing it.
2. **Validate** schema and version claims.
3. **Run** controlled conformance fixtures.
4. **Prove** translation into canonical Omen types.
5. **Prove** unsupported semantics refuse explicitly.
6. **Preserve** identity and provenance.
7. **Prove** unknown extension data cannot grant authority.
8. **Test** output projection.
9. **Test** round-trip where promised.
10. **Record** exact package, protocol, and version evidence.

---

## 5. Bleeding-Edge Research Findings & Architectural Validations

Independent developments across the developer tooling landscape validate Omen's core convictions:

1. **Nushell (Explicit Lanes & Structured Results)**:
   Nu's experiments with explicit command namespaces and structured `$env.LAST_EXIT_CODE` / result objects confirm the utility of first-class execution handles. Omen goes further: `@last` is a typed handle encapsulating execution records, resource leases, Facts, CAS artifacts, and platform assurance.
2. **Atuin (History as a Service & Daemon Architecture)**:
   Atuin treats history as a structured service rather than a flat file, experimenting with daemon-backed hot indexes, explicit `?` AI triggers, and PTY proxying. Lessons for Omen:
   - Hot semantic indexes belong between durable storage (SQLite) and interactive keystrokes.
   - Explicit AI invocation (`?`) is vastly superior to ambient, chatty LLM intrusion.
   - Reusable PTY infrastructure must be architected as an explicit capability layer.
3. **Persistent Agent Shells (Session vs Terminal Identity)**:
   Modern agent runtimes model persistent PTY/shell sessions with reconnect semantics. Lesson for Omen: `InteractiveSessionId` must remain independent from terminal attachment/client identity.
4. **Docker Agent Sandboxes / MicroVM Isolation (Pluggable Backends)**:
   Agent execution is shifting toward reusable container and microVM boundaries. Lesson for Omen 0.6: Do not assume execution is always native host kernel. Architect a pluggable execution-backend interface (`native host`, `WSL`, `container`, `microVM sandbox`, `remote runtime`), truthfully reporting the actual assurance each backend provides.
5. **Kitty Keyboard Protocol**:
   Modern terminals support rich, unambiguous modified-key sequences. Lesson: Support Kitty protocol via capability negotiation for reliable shortcuts, while maintaining seamless fallback for legacy terminals.
6. **Fish Consequence Hints (Ghost Blast Radius)**:
   Fish's hints (e.g. redirect overwrite warnings) are generalised in Omen into semantic blast-radius preflight.
7. **Nushell Stream Inspection**:
   Lazy/stream metadata inspection allows examining output without collecting massive buffers into memory. Lesson for Omen 0.8: Typed composition must support lazy/streaming handles, metadata inspection, and bounded CAS previews without loading entire datasets into memory.
8. **Warp Block Model**:
   Warp demonstrates the UX value of command blocks. Lesson: The structured execution block (`cargo test auth · op_8f12`) exists in the domain layer, independently of whether it renders in an ordinary terminal, OSC-aware host, MCP client, or log.

---

## 6. Release Charters: 0.4 through 1.0

### Omen 0.4 — Shared Runtime
- **Theme**: *One Omen reality across processes.*
- **Daemon (`omend`)**: Small local runtime daemon managing shared physical execution, monitoring, and state.
  - Not an autonomous agent.
  - Not a policy server.
  - Not Resolve.
  - Not Tethers.
- **Physical Coordination**:
  - Local IPC: Windows Named Pipes, Linux/macOS Unix Domain Sockets.
  - Shared Facts and live invalidation broadcasts.
  - Shared subordinate physical history.
  - Shared hot semantic index cache across concurrent shells.
  - Process registry and named service lifecycle management (`proc://`).
  - Session recovery and daemon reconnect semantics.
- **Authority Invariant**: Omen cross-process coordination is strictly physical/runtime coordination. Resolve remains sovereign for live guards, scope locks, fencing, and conflict admission.
- **Success Criteria**: *Two Omen shells and an agent can look at the same workspace and see the same underlying machine truth.*

### Omen 0.5 — Agent Interoperability
- **Theme**: *Stop agents using terminals as eyes.*
- **General Interoperability Boundary**:
  - Edge adapters for MCP (Model Context Protocol), A2A (Agent-to-Agent), LSP, and repository instruction standards (e.g. AGENTS.md).
  - MCP is an adapter, not Omen's internal architecture.
  - A2A integration does not make Omen pretend to be an autonomous agent.
  - Repository instructions remain advisory guidance, not authority.
- **Capabilities**:
  - Tool and resource discovery.
  - Execution request submission.
  - Fact queries and dirty-state subscriptions.
  - Process and service inspection.
  - Bounded context retrieval with CAS pagination.
  - Structured failure reporting.
  - Actor and session identity projection.
  - Standards version negotiation.
- **Success Criteria**: *A capable coding agent using Omen should need raw terminal scraping dramatically less often.*

### Omen 0.6 — Physical Maturity
- **Theme**: *Make physical execution genuinely mature.*
- **Capabilities**:
  - First-class PTY execution and attachable/resumable interactive sessions.
  - Process-tree ownership and service leases.
  - Hardened OS containment:
    - Linux: Landlock LSM, pidfd, cgroups v2, namespaces where justified.
    - Windows: Job Objects, restricted tokens, AppContainer profiles where practical.
    - macOS: Endpoint Security / sandbox-exec profiles.
  - Truthful platform assurance matrix (`ENFORCED`, `MEDIATED`, `OBSERVED`, `BEST_EFFORT`, `UNSUPPORTED`).
  - Pluggable execution backends: Native host, WSL, container, microVM sandbox, remote runtime.
  - Secrets handles: Strict separation of `secret.use` (execution injection) from `secret.expose` (value revelation).
- **Success Criteria**: *Omen can supervise normal developer workloads, hostile fixtures, services and interactive programs while truthfully describing what the platform actually enforced.*

### Omen 0.7 — Semantic Environment
- **Theme**: *Understand more than commands.*
- **Capabilities**:
  - Ast-grep adapter for structural code search and AST rewrites.
  - Compiler metadata extraction.
  - LSP and SCIP integration for symbol and reference navigation (`symbol://crate/auth/refresh_token`).
  - Richer ecosystem understanding: Rust/Cargo, npm/pnpm, Python/uv, Go, Docker, GitHub CLI.
  - Descriptive grammar ingestion (Carapace/Fig) for descriptive syntax hints (without granting authority).
- **Success Criteria**: *Omen increasingly understands what developer actions mean, not merely which executables launched.*

### Omen 0.8 — Composition
- **Theme**: *Make repeated work concise without inventing another programming language.*
- **Project Configuration (`Omen.toml`)**:
  - Optional declarative workspace configuration: Project identity, known services, named checks, common semantic operations, adapter configuration, important resources.
  - Contains zero policy; does not replace Cargo.toml, package.json, or pyproject.toml.
- **Composition Engine**:
  - Named actions combining multiple checks while explicitly exposing underlying invocations.
  - Typed routing between Omen values and operation results.
  - Large output handling: CAS artifacts, lazy handles, bounded previews, metadata inspection.
  - Complex logic remains in real programming languages (Python, Rust, Nushell, Bash).
- **Authority Invariant**: Omen may deterministically reconstruct typed execution requests and compositions from recorded subordinate evidence, but all consequential execution must be re-admitted under current Tethers authority. Omen does not own authoritative replay.
- **Success Criteria**: *Most everyday developer command chains become clearer and safer without Omen growing loops, classes, modules and another package manager.*

### Omen 0.9 — Stabilisation
- **Theme**: *Stop adding clever things and make everything boringly dependable.*
- **Focus Areas**:
  - Candidate protocol and wire schema freeze.
  - Strict backward compatibility and migration paths.
  - Database schema migrations with rollback support.
  - Crash recovery and corruption handling across SQLite and CAS.
  - CAS integrity verification and GC retention policies.
  - High-volume history stress testing.
  - Rigorous security review: Path traversal, symlink/junction/mount escape, secret leakage, terminal escape injection, malicious workspace metadata, adapter trust, local IPC authentication.
  - Cross-platform fuzzing with hostile Gremlin torture testing.
  - Accessibility and packaging refinement.
- **Success Criteria**: *We deliberately struggle to break it before allowing it to call itself 1.0.*

### Omen 1.0 — Stable Human + Agent Runtime
- **The 1.0 Promise**: A stable, production-grade commitment across:
  - Canonical resource semantics (`workspace://`, `tool://`, `fact://`, `proc://`, `artifact://`).
  - Machine-facing protocol and wire schemas.
  - Fact Registry lifecycle and lazy invalidation invariants.
  - Execution results and CAS artifact integrity.
  - 3-lane human grammar and reference resolution.
  - Interoperability SPI and adapter lifecycle contracts.
  - Truthful platform assurance matrix.
  - Strict system boundaries (Lantern, Resolve, Tethers, ThreadMoth).
- **Core Qualities**:
  - *Human quality*: Low-friction, calm, fast, daily-driver interactive environment.
  - *Agent quality*: Semantic operation without terminal scraping.
  - *Shared reality*: Humans and agents observe identical machine truth without session history contamination.
  - *Cross-platform*: Windows and Linux first-class; macOS documented truthfully.
  - *AI-optional*: 100% functional without an LLM; AI provides optional reasoning.
  - *Evidence-backed*: Every major guarantee supported by reproducible automated proofs.

---

## 7. What Omen 1.0 is NOT Required to Become

Omen must not expand into:
- an operating system;
- an IDE;
- a coding agent;
- a model router;
- autonomous workflow orchestration;
- a policy language;
- an approval system;
- durable cognitive memory;
- a container platform;
- a general-purpose scripting language;
- a theme or plugin marketplace;
- a replacement for existing developer tools.

Those make Omen larger without making it better.

---

## 8. Product Tests for Future Ideas

For every proposed feature, evaluate:
> **Does this make the machine easier for a human or agent to understand or operate truthfully?**  
> If yes, it may belong. If the answer is "No, but Omen could also become responsible for this...", it belongs elsewhere.

For UI changes, evaluate:
> **Does this improve orientation, communicate state, improve parsing, or prevent a meaningful mistake?**  
> If none apply, remove it.

For AI features, evaluate:
> **Can this be answered from structured state, schemas, history or provenance?**  
> If yes, answer it deterministically. Use AI only where human judgement is genuinely required.

---

## 9. Architecture Traps to Guard Permanently

Do not allow future work to:
1. Create an Omen policy system.
2. Create an Omen approval system.
3. Create a competing trusted capability manifest.
4. Create authoritative replay separate from Tethers.
5. Steal Resolve locks or guards.
6. Silently promote inference to Fact.
7. Re-run expensive verification behind the user's back.
8. Treat ANSI/UI text as machine truth.
9. Claim OSC creates semantics it does not.
10. Assume Windows paths behave like Bash.
11. Hard-code PowerShell into cross-platform tests.
12. Make `@last` global across agents.
13. Introduce a second long-term memory system.
14. Make MCP internal architecture.
15. Invent vendor-specific branches in `omen-core`.
16. Run network, process, or filesystem discovery synchronously on keystrokes.
17. Let fuzzy matching decide consequential meaning.
18. Call something cross-platform based on a single-machine test.
19. Overstate benchmark meaning.

---

## 10. Final Character of Omen

Omen is not trying to make the old shell incrementally smarter.

It is trying to expose enough structured machine reality that **humans and AI no longer need to reconstruct the computer from strings**.

- The human interface remains warm, fast, and low-friction.
- The machine interface remains explicit, typed, and truthful.
- The AI layer remains optional.
- The authority boundaries remain boringly clear.
- The architecture is open: when the AI ecosystem invents something important, Omen adds a new adapter rather than needing a new identity.
