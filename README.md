# Omen

**Agent-Native Developer Runtime**

> *Omen is substrate, not sovereign.*

Omen bridges the gap between AI coding agents and physical operating systems. Rather than forcing agents to scrape terminal output, parse ANSI escape sequences, or guess the side effects of shell commands, Omen makes developer tools, workspace states, execution constraints, and runtime evidence typed, structured, inspectable, and enforceable.

## Division of Responsibility

| Component | Responsibility |
|---|---|
| **Lantern** | Durable memory, long-term context, and cross-session provenance |
| **Resolve** | Live coordination, concurrency control, and scope locks |
| **Tethers** | Policy, permission, approval, capability identity, intent, and outcome truth |
| **Omen** | Physical machine mechanics, execution containment, typed resources, facts, and CAS evidence |
| **ThreadMoth** | Deterministic bounded structural mutations and pre/post-image validation |

## Core Principles

- **Argv-only Execution**: No shell expansion, quoting bugs, or injection attacks.
- **Closed Stdin by Default**: Commands run non-interactively and receive EOF unless explicitly attached.
- **Bounded Context**: Command outputs are bounded in memory and JSON results; complete transcripts are spooled to SHA-256 Content-Addressed Storage (CAS).
- **Persistent Fact Registry**: Local machine and workspace state (git cleanliness, build status, test outcomes) tracked in SQLite with generation tracking and lazy pessimism dirty invalidation.
- **Tool Atlas**: Runtime profiles and active probing of developer tools (`threadmoth`, `cargo`, `git`, `ripgrep`).

## Repository Topology

- `crates/omen-core`: Pure domain types, typed IDs, URI grammar, assurance levels, error codes.
- `crates/omen-schema`: Wire serialization, JSON Schemas, strict contract/result schemas.
- `crates/omen-engine`: Process supervisor, stream capture, timeouts, platform containment.
- `crates/omen-knowledge`: SQLite Fact Registry, resource generations, CAS blob store.
- `crates/omen-atlas`: Tool registry, runtime profiles, probing validator harness.
- `crates/omen-adapters`: Dedicated adapters for ThreadMoth, Cargo, Git, and ripgrep.
- `crates/omen-cli`: The `omen` CLI binary.
- `crates/omen-test-fixtures`: Hostile testing fixtures (`omen-gremlin`).
- `crates/xtask`: Portable repository automation tasks.

## Getting Started

```pwsh
# Run full verification
cargo xtask verify

# Generate JSON schemas
cargo xtask generate-schemas
```
