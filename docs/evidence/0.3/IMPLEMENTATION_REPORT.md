# Omen 0.3 Human Interface — Implementation Report

**Release**: Omen 0.3.0  
**Target**: Human-Facing Agent-Native Developer Runtime  
**Status**: VERIFIED & COMPLETE  
**Repository**: `https://github.com/matthewjameswatkins1978-cyber/Omen.git`  
**Branch**: `feature/omen-0.3-human-interface`  

---

## 1. Executive Summary

Omen 0.2 established the machine substrate: typed resources, structured execution contracts, Fact Registry, dirty/current truth, Tool Atlas, CAS storage, process supervision with Job Object containment, and deterministic ThreadMoth / Cargo / Git adapters.

Omen 0.3 exposes that structured reality to human developers through an interactive shell interface that is calm, discoverable, deterministic, high-performance, and intelligent before AI-powered.

### Key Architectural Boundaries Preserved

> **Omen is substrate, not sovereign.**

The division of responsibility remains inviolable:
- **Lantern** knows (durable memory, long-term context, and provenance).
- **Resolve** coordinates (live guard tokens and scope locks).
- **Tethers** controls (permission, trusted capability identity, policy, approval, durable intent, replay, and provider outcome truth).
- **Omen** makes the machine legible and enforceable (physical execution, containment, typed resources, machine-readable facts, and artifact CAS evidence).
- **ThreadMoth** deterministically mutates (bounded structural edits with cryptographic pre/post hashes and refusal).

Omen 0.3 does not implement sovereign AI agents, autonomous approval bypasses, or long-term cognitive memory. The optional AI lane (`? query`) is an advisory interface that formulates typed Omen commands and references (`:show @failed`, `:why @last`), routing through Tethers/Resolve boundaries if execution is requested.

---

## 2. Hardening and Truthfulness Updates

Following the architectural audit, the following critical repairs and hardening passes were executed:

1. **Hosted Cross-Platform CI Fix**:
   - `child_pid` in `omen-engine/src/supervisor.rs` was cleanly scoped inside `#[cfg(windows)]`, resolving the Ubuntu Clippy failure without blanket lint suppression.
2. **Truly Unique Session Identity**:
   - `InteractiveSession::new()` generates distinct `InteractiveSessionId` using UUID v4 rather than hashing `cwd`. Two shells in the same directory receive unique session identities.
   - `new_with_session_id()` is retained for deterministic fixtures.
3. **Cross-Platform Windows Path Parsing**:
   - `GrammarScanner::split_words()` preserves Windows drive letters (`C:\...`), UNC paths (`\\server\share`), quoted paths with spaces, literal backslashes, trailing backslashes, and handles incomplete quotes without panics or Bash-specific escaping.
4. **Hot Semantic Index for Non-Blocking Completion**:
   - Live REPL wires real state into completion using `HotSemanticIndex`.
   - Out-of-band refreshing at prompt render time populates active facts and workspace entries.
   - Keystroke completion executes purely in-memory over `HotSemanticIndex` with zero SQLite queries or synchronous `read_dir` operations.
5. **Invocation-Specific Interactive Child Classification**:
   - Replaced coarse executable matching with invocation-specific semantic classification.
   - `python` and `node` with script arguments execute under standard supervision; interactive REPLs and Git interactive editors (`git commit` without `-m`, `git rebase -i`, `git add -p`) execute with terminal handoff.
   - Added explicit `--interactive` / `--handoff` override.
   - Interactive executions are recorded in `ExecutionHistory` with exit code and duration.
6. **Rigorous Benchmark Accounting**:
   - Disaggregated in-process session construction from genuine release binary cold start.
   - Benchmarked completion against representative hot-index state (50 facts, 20 workspace files).
   - Removed misleading "zero-lock" claims; documented uncontended `Mutex` access.
7. **Canonical Roadmap to 1.0 & Interoperability Seam**:
   - Adopted canonical 10-milestone sequence to 1.0.
   - Documented the three orthogonal axes (Canonical Semantics, Protocol Binding, Transport) and Adapter Lifecycle distinctions.

---

## 3. Milestone Deliverables (H0 – H11)

- **H0: Architecture & Dependency Freeze**: Pinned Reedline 0.51.0, Crossterm 0.29.0, nu-ansi-term 0.50.3, fuzzy-matcher 0.3.7, unicode-width 0.2.2, uuid 1.15.
- **H1: Interactive Shell Core & Terminal Lifecycle**: `InteractiveSession`, `TerminalCapabilities` (truecolor, unicode, OSC detection with dumb degradation), `PromptRenderer`, and invocation-specific child handoff.
- **H2: Semantic Grammar & Dispatcher**: 3-lane `GrammarScanner` (Executable, Semantic Action `:`, AI Reasoning `?`) and `SemanticDispatcher` for substrate actions.
- **H3: Fact-Aware Completion & Ghost Hinter**: `OmenCompleter` with `HotSemanticIndex` elevating `DIRTY` facts, and non-intrusive `OmenHinter`.
- **H4: Subordinate Physical History & Typed References**: SQLite relational history scoped by unique `InteractiveSessionId`, and `ReferenceResolver` for `@last`, `@failed`, `@errors`.
- **H5: Human Diagnostics & Explanations**: `DiagnosticRenderer` progressive disclosure (Levels 0–3) and causal `:why` fact provenance trees.
- **H6: Semantic Blocks & Terminal Integration**: `SemanticBlock` emitting OSC 7 (cwd sync), OSC 8 (CAS hyperlinks), and OSC 133 / FTCS semantic command/prompt anchors.
- **H7: Blast-Radius Preflight & Paste Guard**: Destructive command blast-radius analysis and multiline paste buffer inspection.
- **H8: Services & Process UX**: `ServiceRegistry`, `ManagedService`, and `proc://` URI tracking.
- **H9: Optional AI Reasoning Lane Boundary**: Advisory `?` lane with deterministic local fallback.
- **H10: Human/Agent Shared-Reality Proof**: Formal proof of background agent mutations making facts dirty and updating human prompt (`! 1 dirty`) without session history contamination.
- **H11: Benchmarks, Golden Snapshots & Evidence**: 4-part benchmark suite, golden snapshot tests in `omen-ui`, and canonical Road to 1.0 roadmap.

---

## 4. Verification Summary

| Verification Gate | Specification | Result | Status |
| :--- | :--- | :--- | :--- |
| `cargo fmt --check` | 100% compliant formatting | Clean | **PASS** |
| `cargo clippy -D warnings` | Zero warnings across all targets & features | Zero warnings | **PASS** |
| `cargo test --workspace` | All unit, integration, and regression suites | 19 test suites passed | **PASS** |
| `cargo xtask verify-schemas` | Wire schemas in sync with Rust wire types | Up to date | **PASS** |
| `cargo-deny check` | Advisories, bans, licenses, sources | All checks ok | **PASS** |
| **In-Process Prompt Construction** | In-process struct allocation & render | **0.167 ms** (166.6 µs) | **PASS** |
| **Genuine Cold Process Launch** | Fresh release binary (`omen.exe doctor`) | **13.69 ms** | **PASS** |
| **Hot-Index Completion Latency** | 50 facts, 20 files, tools, actions | **0.134 ms** (134.3 µs) | **PASS** |
| **Grammar Scanner Throughput** | > 100,000 scans/sec | **689,841 scans/sec** | **PASS** |
