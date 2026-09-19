# Omen

**Agent-Native Developer Runtime & Human Shell**

> *Omen is substrate, not sovereign.*

Omen is a deterministic execution and state runtime for AI agents, with a low-friction human shell interface. It turns operating-system actions into typed, capability-scoped transactions and returns facts, diffs, and references instead of terminal noise.

> **Do not teach the AI to operate a terminal better. Remove the terminal from the machine-facing side of the relationship.**

Omen gives humans and AI the same workspace through different interfaces, while making execution typed, observable, capability-scoped, reversible, and context-efficient. It makes the machine legible and enforceable without becoming an agent, policy system, memory system, or autonomous brain.

## Division of Responsibility

| Component | Role | Responsibility |
|---|---|---|
| **Lantern** | *Knows* | Durable memory, long-term context, provenance, and durable contextual knowledge |
| **Resolve** | *Coordinates* | Live coordination, guard tokens, scope locks, fencing, and conflict admission |
| **Tethers** | *Controls* | Policy, permission, trusted capability identity, approval, durable intent, replay, and outcome truth |
| **Omen** | *Makes legible & enforceable* | Physical execution mechanics, containment, typed resources, Facts, artifacts, and execution evidence |
| **ThreadMoth** | *Mutates deterministically* | Bounded structural mutations, syntactic search/rewrite, and pre/post validation |

> **Lantern knows. Resolve coordinates. Tethers controls. Omen makes the machine legible and enforceable. ThreadMoth mutates deterministically.**

*Important*: Resolve admission cannot grant Tethers permission. Tethers controls authority; Omen reports enforceability.

## Core Architectural Invariants

- **Argv-only Execution**: No shell string expansion, quoting bugs, or injection attacks.
- **Closed Stdin by Default**: Commands run non-interactively and receive EOF immediately unless explicitly attached.
- **Bounded Context**: Transcripts are bounded in memory and JSON results (4–8 KiB); full unreduced output is spooled to SHA-256 Content-Addressed Storage (CAS) (`artifact://`).
- **Persistent Fact Registry**: Semantic machine state (git cleanliness, build status, test outcomes) tracked in SQLite with generation tracking and lazy pessimism dirty invalidation.
- **Tool Atlas**: Runtime profiles and probing of developer tools (`threadmoth`, `cargo`, `git`, `ripgrep`). Candidate descriptions are emitted; trusted authority remains with Tethers.
- **Human Interface**: State-aware interactive shell with a 3-lane grammar (`:action`, `@reference`, `? query`), progressive disclosure (Levels 0–3), non-blocking completion via hot semantic index, and blast-radius preflight.
- **Cross-Platform Truthfulness**: First-class Windows, Linux, and macOS support. When a platform cannot enforce a guarantee, Omen truthfully reports `OBSERVED` or `UNSUPPORTED`. It never fakes `ENFORCED`.

## Repository Topology

- `crates/omen-core`: Pure domain types, typed IDs (`ResourceId`, `FactId`, `ArtifactId`), URI grammar, assurance levels, error codes.
- `crates/omen-schema`: Wire serialization, JSON Schemas, strict contract/result schemas (`deny_unknown_fields`).
- `crates/omen-engine`: Process supervisor, stream capture, timeouts, platform containment (Job Objects, Landlock, POSIX).
- `crates/omen-knowledge`: SQLite Fact Registry, resource generations, CAS blob store.
- `crates/omen-atlas`: Tool registry, TOML runtime profiles, probing validator harness.
- `crates/omen-adapters`: Dedicated adapters for ThreadMoth, Cargo, Git, and ripgrep.
- `crates/omen-interactive`: Interactive shell core, 3-lane grammar scanner, hot semantic index, fact-aware completer, session history, blast radius, paste guard.
- `crates/omen-ui`: Terminal capability detection, prompt rendering, progressive diagnostics (Levels 0–3), semantic blocks (OSC 7/8/133).
- `crates/omen-cli`: The unified `omen` CLI and interactive shell binary.
- `crates/omen-test-fixtures`: Hostile testing fixtures (`omen-gremlin`).
- `crates/xtask`: Portable repository automation tasks.

## Release Roadmap — The Road to 1.0

1. **Omen 0.2: Machine Truth** — *Complete*
2. **Omen 0.3: Human Interface** — *Complete*
3. **Omen 0.4: Shared Runtime** — Local daemon (`omend`), physical cross-process coordination, shared facts, shared hot semantic index.
4. **Omen 0.5: Agent Interoperability** — General interoperability boundary, protocol adapters (MCP, A2A, LSP), compatibility profiles.
5. **Omen 0.6: Physical Maturity** — Pluggable execution backends, PTY persistence, hardened containment, truthful platform assurance.
6. **Omen 0.7: Semantic Environment** — Structural search (ast-grep), compiler metadata, language servers, symbol references.
7. **Omen 0.8: Composition** — Typed pipeline composition, `Omen.toml`, deterministic reconstruction re-admitted under Tethers.
8. **Omen 0.9: Stabilisation** — Protocol freeze, backwards compatibility, migrations, torture testing, fuzzing, security audit.
9. **Omen 1.0: Stable Human + Agent Developer Runtime** — Production guarantees across human, agent, and shared reality.

See [Using Omen](docs/USING_OMEN.md) for a walkthrough of daily shell workflows and what actually happens when you open it.
See [Road to 1.0](docs/ROAD_TO_1_0.md) and [Roadmap](docs/ROADMAP.md) for full architectural specifications.

## Getting Started

```sh
# Run full verification suite
cargo xtask verify

# Generate JSON schemas
cargo xtask generate-schemas

# Run performance benchmarks
cargo run -p xtask -- bench
```

