# Omen 0.2 Implementation Report

## 1. Executive Summary

Omen 0.2 is the first working implementation of the agent-native developer runtime.
It enforces the core architectural doctrine:

> **Omen is substrate, not sovereign.**  
> **Lantern** knows (durable memory, long-term context, and provenance).  
> **Resolve** coordinates (live guard tokens and scope locks).  
> **Tethers** controls (permission, trusted capability identity, policy, approval, durable intent, replay, and provider outcome truth).  
> **Omen** makes the machine legible and enforceable (physical execution, containment, typed resources, machine-readable facts, and artifact CAS evidence).  
> **ThreadMoth** deterministically mutates (bounded structural edits with cryptographic pre/post hashes and refusal).

All milestones (M0 through M10) have been implemented, tested, and validated with zero clippy warnings, full schema verification, and 100% test pass rate across the workspace.

---

## 2. Milestone Accomplishments

### M0: Repository & Workspace Bootstrap
- Rust 1.98.1, Edition 2024, resolver "3".
- Rigorous configuration: `rust-toolchain.toml`, `clippy.toml`, `rustfmt.toml`, `deny.toml` (v0.19.7 compatible), `justfile`.
- Full doctrine documentation suite: `AGENTS.md`, `README.md`, `ARCHITECTURE.md`, `BOUNDARIES.md`, `PROTOCOL.md`, `KNOWLEDGE_MODEL.md`, `TOOL_ATLAS.md`, `ASSURANCE.md`, `TETHERS_BOUNDARY.md`, `PLATFORM_BACKENDS.md`, `REFERENCES.md`, `ROADMAP.md`.
- Automated tooling via `xtask`: `verify`, `generate-schemas`, `verify-schemas`.
- CI workflow in `.github/workflows/ci.yml`.

### M1: Typed Core Primitives & Wire Schemas
- Strongly-typed ID wrappers: `ResourceId`, `ToolId`, `ExecutionId`, `ActionId`, `FactId`, `ArtifactId`, `ProcessId`.
- Strictly bounded `ResourceUri` parser: enforces closed scheme enumeration (`workspace`, `tool`, `fact`, `observation`, `inference`, `artifact`, `proc`, `secret`, `net`, `actor`, `trace`), path safety, backslash rejection, traversal rejection (`..`), and artifact SHA-256 validation.
- Closed wire schemas (`deny_unknown_fields`): `ExecutionContractWire` (`omen.execution/0.2`) and `ExecutionResultWire` (`omen.result/0.2`).
- Generated schemas in `schemas/v0.2/`.

### M2: Knowledge Model, Fact Registry, & CAS
- SQLite WAL session persistence (`omen-knowledge`), versioned schema migrations (`V1__initial_schema.sql`).
- Fact Registry: active facts, superseded historical chains, witness tracking, and provenance querying (`why_fact`).
- Resource generation counters & dependency tracking: changing an underlying resource generation invalidates dependent facts.
- **Lazy pessimism doctrine**: when a fact is dirty, `require_current = true` refuses with `CoreError::FactDirty`. Omen **refuses to silently re-execute or guess**.
- Content-Addressed Storage (`ContentAddressedStore`): two-level fanout under `cas/sha256/ab/...`, byte-accurate hashing, bounded slice reads, and retention classes (`ephemeral`, `referenced`, `pinned`).
- Ephemeral GC: dry-run and live eviction, updating artifact blob state to `EVICTED` while preserving database audit metadata.

### M3: Execution Engine & Containment Backend
- `ProcessSupervisor`: argv-only command dispatch (no raw shell parsing or shell string injection).
- Closed stdin by default (`StdioMode::Closed`): background/automated processes immediately receive EOF.
- Bounded transcript projection: inline stdout/stderr strictly bounded (4–8 KiB), with complete unreduced stream spooled to CAS.
- Timeout supervisor: terminates processes exceeding execution limits.
- **Assurance-dependent containment**:
  - Windows: Windows Job Objects with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` strongly guaranteeing descendant termination (`Enforced`).
  - Linux: Landlock/pidfd/cgroups where available; process groups report `Observed` / `DescendantsUnverified` without faking enforcement.
- Hostile test executable `omen-gremlin` exercising stdout flooding, stdin hangs, child process loops, and timeout termination.

### M4: Tool Atlas & Runtime Profiles
- Structured TOML runtime profiles in `profiles/`: `threadmoth.toml`, `cargo.toml`, `git.toml`, `ripgrep.toml`.
- Binary SHA-256 fingerprinting with change detection transitioning instances to `STALE_VALIDATION`.
- `ToolValidator` harness running non-interactive probes in isolated temporary sandboxes.
- Carapace/Fig command syntax integration interfaces.

### M5: Tier-1 Machine Adapters (Git & ripgrep)
- `GitAdapter`: executes `git status --porcelain=v2 --branch`, parses branch/head OID/cleanliness, and publishes `fact://git/branch` and `fact://git/clean`.
- `RipgrepAdapter`: executes `rg --json`, streams matches, bounds in-memory preview, and spools complete match NDJSON into CAS.

### M6: Tier-1 Machine Adapter (ThreadMoth)
- Implements `ThreadMothAdapter` targeting current ThreadMoth CLI (1.10.0).
- Probes: `threadmoth doctor --json` and `threadmoth capabilities --json`.
- Deterministic mutation: constructs canonical `ThreadMothRequest` (schema 1.3.1), dispatches `threadmoth mutate --request <file>`, and parses the returned JSON certificate (`outcome`, `pre_hash`, `post_hash`, `reason_code`, `changed_ranges`).
- Verified refusal when expected `pre_hash` does not match.

### M7: Tier-1 Machine Adapter (Cargo)
- Structured metadata discovery: `cargo metadata --format-version 1 --no-deps`.
- Diagnostic parsing: `cargo check --message-format=json`, captures compiler diagnostics, spools raw ndjson into CAS, and publishes `fact://compiler/errors`.
- Test execution: `cargo test`, captures execution output into CAS, cleanly publishes `fact://test/status` (distinct from compiler diagnostics).

### M8: End-to-End Proofs & Evidence
- Real fixture package `tests/fixtures/auth_project`.
- Proof 1 (`e2e_proofs.rs`): Cargo verify passing -> ThreadMoth mutate to break auth -> generation increments -> query `require_current = true` refuses with `CoreError::FactDirty` -> explicit Cargo run publishes `failing` -> `why_fact` traces history & CAS slice evidence -> ThreadMoth restore code -> fact remains `DIRTY` -> explicit Cargo re-verification publishes `passing`.
- Proof 2: Gremlin torture suite passing all containment and boundary assertions.
- Recorded evidence in `docs/evidence/0.2/END_TO_END_PROOFS.md`.

### M9: Tethers Boundary Contract Fixture
- `tests/tethers_contract_fixture.rs`: demonstrates Omen receiving external execution identity (`exec-7f8e9d0a1b2c`) and actor (`actor://tethers/agent/ci-builder`) from Tethers.
- Executes physical mechanics, captures artifacts into CAS, reports physical runtime status and containment levels, and returns `ExecutionResultWire`.
- Demonstrates zero boundary blurring: Omen does not create competing policy, approvals, or replay.

### M10: CLI Polish & Implementation Closeout
- Complete CLI subcommands in `omen-cli`:
  - `omen doctor [--json]`
  - `omen describe [--json]`
  - `omen tool [list|inspect|validate] [--json]`
  - `omen fact [get|why] [--json]`
  - `omen exec [--contract <file>] [--json] -- <argv>`
  - `omen artifact [inspect|read] [--json]`
  - `omen gc [--dry-run] [--json]`
- Zero parsing of human-readable output internally.
- Clean JSON output on all machine paths.

---

## 3. Verification Summary

Every check run via `cargo run -p xtask -- verify` succeeded:
1. `cargo fmt --check`: Passed.
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings`: Passed (0 warnings).
3. `cargo test --workspace`: Passed (all unit, integration, gremlin, e2e, and adapter tests).
4. `xtask verify-schemas`: Passed (JSON Schemas match Rust struct definitions).
5. `cargo-deny check`: Passed (advisories ok, bans ok, licenses ok, sources ok).
