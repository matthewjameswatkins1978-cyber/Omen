# Omen Architecture

> **Doctrine**: *Omen is substrate, not sovereign.*  
> **Compact Boundary**: *Lantern knows. Resolve coordinates. Tethers controls. Omen makes the machine legible and enforceable. ThreadMoth mutates deterministically.*

---

## 1. Overview

Omen is an agent-native developer runtime and low-friction human shell. Its purpose is to bridge AI coding agents and human developers to physical operating systems. Rather than forcing agents to scrape terminal output or parse ANSI text, Omen turns operating-system actions into typed, capability-scoped transactions and returns facts, diffs, and references.

> **Do not teach the AI to operate a terminal better. Remove the terminal from the machine-facing side of the relationship.**

---

## 2. Engineering Doctrine

Omen follows the Matthew/Lucy AI-first engineering rules:
1. **Consequential truth must be explicit.**
2. **One semantic authority for each kind of truth.**
3. **Reasoning should be bounded.**
4. **Prefer simple explicit structures over cleverness.**
5. **Important knowledge must be discoverable.**
6. **Constraints belong below the model.**
7. **Prefer deterministic transforms and local reasoning.**
8. **Use rigid schemas and strong types.**
9. **Reduce dependency entropy; isolate capabilities.**
10. **Tests and evidence matter more than confident prose.**
11. **Non-amplification: uncertain input must not silently become stronger truth.**

Additional Omen-specific operational rules:
- **Never lie by abstraction.**
- **Infer what has already been decided. Never invent what has not.**
- **Pretty must never make slow.**
- **Omen should be intelligent before it is AI-powered.**
- **Stable inside, adaptable outside.**
- **Translate differences. Never hide them.**
- **Cross-platform by default**: Unless explicitly platform-specific, code and tests are portable across Windows, Linux, and macOS. Do not hard-code shell or OS conventions into portable layers.

---

## 3. Repository Topology & Subsystem Layers

The Omen workspace is cleanly layered:

1. **Domain Primitives (`omen-core`)**: Strongly typed resource identifiers (`ResourceId`, `FactId`, `ArtifactId`), logical URI grammar (`workspace://`, `tool://`, `fact://`, `proc://`, `artifact://`), assurance levels (`DETERMINISTIC`, `VERIFIED`, `ENFORCED`, `OBSERVED`, `CLAIMED`, `INFERRED`, `UNKNOWN`), and canonical error codes. Pure domain logic, independent of I/O.
2. **Wire Specification (`omen-schema`)**: Strict JSON Schemas for execution contracts and results, enforcing closed models (`deny_unknown_fields`) and strict semantic versions (`omen.execution/0.2`, `omen.result/0.2`).
3. **Execution Engine (`omen-engine`)**: Argv-only process execution, closed stdin by default, asynchronous stream capture, timeout supervisors, and platform-specific containment backends (Windows Job Objects, Linux Landlock/pidfd/cgroups, macOS portable).
4. **Knowledge System (`omen-knowledge`)**: SQLite-backed Fact Registry with WAL persistence, resource generation counters, lazy pessimism dirty invalidation, and Content-Addressed Storage (CAS) for execution evidence artifacts.
5. **Tool Atlas (`omen-atlas`)**: Structured tool catalog, TOML runtime profiles, fingerprinting, and a controlled probing validator harness. Emits candidate capability descriptions; trusted authority remains with Tethers.
6. **Tool Adapters (`omen-adapters`)**: Dedicated high-fidelity integrations for ThreadMoth, Cargo, Git, and ripgrep.
7. **Interactive Substrate (`omen-interactive`)**: 3-lane grammar scanner (Executable, Semantic Action `:`, AI Reasoning `?`), non-blocking fact-aware completer via in-memory `HotSemanticIndex`, subordinate physical history with unique `InteractiveSessionId`, reference resolution (`@last`, `@failed`), blast-radius preflight, multiline paste guard, and service management (`proc://`).
8. **User Interface (`omen-ui`)**: Terminal capability detection (Truecolor, Unicode, OSC support with graceful degradation), prompt rendering, progressive diagnostics (Levels 0–3), causal `:why` fact provenance trees, and semantic blocks (OSC 7, OSC 8, OSC 133 / FTCS).
9. **Unified CLI & Shell (`omen-cli`)**: The compiled `omen` binary supporting headless CLI execution, automation subcommands, and the interactive human REPL.
10. **Test Fixtures (`omen-test-fixtures`)**: Hostile and torture testing fixtures (`omen-gremlin`).
11. **Repository Automation (`xtask`)**: Portable repo tasks (`verify`, `generate-schemas`, `verify-schemas`, `bench`).

---

## 4. Semantic Preference Hierarchy

When fulfilling operations, Omen always prefers the highest semantic layer that truthfully handles the operation:

```text
text occurrence        -> ripgrep
syntax structure       -> ast-grep
symbol/reference       -> LSP / SCIP (future)
structural mutation    -> ThreadMoth
explicit file actions  -> filesystem primitives
raw shell              -> fallback
```

Use the highest semantic layer that truthfully handles the operation.

---

## 5. Knowledge & Execution Invariants

- **A Fact is never true merely because Omen remembers it. A Fact is true while its witnesses remain valid.**
- When dependencies change, Facts transition from `CURRENT` to `DIRTY`. Omen refuses to silently re-run commands when `CURRENT` is required.
- Transcripts are bounded (4–8 KiB); complete evidence is stored in CAS (`artifact://`).
- Platform assurance is reported truthfully (`ENFORCED`, `OBSERVED`, `UNSUPPORTED`). Omen never simulates sandboxing.
- Personal interaction history (`@last`) is strictly isolated to the current interactive session, while machine reality (Facts, workspace generations) is shared across humans and agents.

