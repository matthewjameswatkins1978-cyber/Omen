# Omen Completion Specification

> **Rule**: *Pretty must never make slow.*  
> **Latency Budget**: *Keystroke completion must execute strictly in-memory in under 1 millisecond.*

---

## 1. Core Principles

Interactive auto-completion in Omen is:
- **Fact-Aware**: Elevated suggestions for dirty facts needing revalidation, failed targets, and modified files.
- **Non-Blocking**: Keystroke evaluation performs zero synchronous disk I/O, database queries, or external process spawns.
- **Deterministic**: Fuzzy matching ranks candidates but never decides execution semantics.

---

## 2. Prohibition of Keystroke Latency

The following operations are **strictly prohibited** on the keystroke completion path:
- Process spawning (e.g. executing `cargo --help` or `git status`).
- Recursive filesystem scans.
- Network calls or remote lookups.
- Cargo metadata or package manager invocations.
- Database migrations or SQLite disk reads.
- Fact recomputation or revalidation.

---

## 3. Hot Semantic Index Architecture

Omen separates **state harvesting** from **keystroke ranking** using the `HotSemanticIndex`:

```
┌─────────────────────────────────────────────────────────────┐
│                    Durable Reality                          │
│     (SQLite Fact Registry, Workspace Filesystem, Atlas)     │
└──────────────────────────────┬──────────────────────────────┘
                               │
               (Out-of-band refresh at prompt render)
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                 In-Memory HotSemanticIndex                  │
│       (Active Facts, File Cache, Atlas Tools, History)      │
└──────────────────────────────┬──────────────────────────────┘
                               │
                (Sub-millisecond fuzzy ranking)
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                 OmenCompleter / OmenHinter                  │
│             (Reedline Keystroke Completion Event)           │
└─────────────────────────────────────────────────────────────┘
```

1. **Out-of-Band Refresh**: When an execution finishes and the prompt is re-rendered, Omen pulls active facts and workspace file entries into an in-memory `HotSemanticIndex`.
2. **Lock-Free Read Path**: When the user types, `OmenCompleter` queries the populated `HotSemanticIndex` via fast in-memory string scanning and fuzzy matching.
3. **Measured Latency**: Benchmarked at **0.134 ms** (134 µs) for 100 candidates, consuming <3% of the 5.0 ms budget.

In Omen 0.4, the hot semantic index will be promoted to a daemon-backed shared memory cache (`omend`) across multiple concurrent processes.

---

## 4. Candidate Ranking Hierarchy

When matching completion prefixes, candidates are sorted according to explicit semantic priority:

```text
1. Failed targets / commands         (@failed, :rerun, failing tests)
2. Dirty facts                       (Facts whose witnesses changed, e.g. fact://test:suite [DIRTY])
3. Recently touched files            (Workspace files modified in recent git generations)
4. Workspace relevant items          (Targets declared in project manifests)
5. Recent interactive history        (Commands run recently in current session)
6. Exact prefix matches              (Exact binary or file matches)
7. Remaining valid candidates        (Fuzzy matches across Atlas tools and paths)
```

---

## 5. Ghost Hinter

`OmenHinter` provides inline grayed-out ghost suggestions based on:
- High-confidence history matches for the current session.
- Immediate deterministic semantic actions (e.g. typing `:test` hints `auth` if `auth.rs` was just touched).

**Rule**: Ghost suggestions remain purely visual hints until explicitly accepted via `Tab` or `Right-Arrow`. Pressing `Enter` submits only what the human has visibly typed.
