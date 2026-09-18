# Omen 0.3 — Performance Audit & Benchmarks

**Date**: 2026-09-18  
**Architecture**: x86_64  
**OS**: Windows 11 Enterprise (Build 26100)  
**Rust Version**: rustc 1.98.1  
**Harness**: `cargo run -p xtask -- bench`  

---

## 1. Methodology & Truthfulness

Performance claims in Omen are verified by reproducible automated benchmarks. We distinguish between:
1. **In-Process Session Construction**: Memory allocation, terminal capability interrogation, and initial prompt string synthesis within a running Rust process.
2. **Genuine Cold Process Launch**: Full OS process spawn of a release binary (`omen.exe doctor --json`), including executable image loading, dynamic link resolution, C/Rust runtime initialization, command parsing, environment inspection, and JSON output generation.
3. **Hot-Index Interactive Completion**: In-memory fuzzy ranking across representative workspace state (active facts, directory entries, tools, actions) without synchronous filesystem or database I/O on keystrokes.
4. **Grammar Scanner Throughput**: Lexical categorization of raw input lines into Executable, Semantic Action, or AI Reasoning lanes.

Concurrency note: Omen 0.3 avoids lock contention on keystrokes by querying an in-memory `HotSemanticIndex` protected by `std::sync::Mutex`. Out-of-band updates occur prior to prompt display, ensuring sub-millisecond keystroke responsiveness.

---

## 2. Benchmark Measurements

### 2.1 In-Process Session Construction & Prompt Latency
- **Definition**: Time to allocate `InteractiveSession`, detect `TerminalCapabilities`, compute color roles, and render the initial prompt string in-process.
- **Sample Size**: 100 iterations
- **Min**: 151.9 µs (0.152 ms)
- **Max**: 450.0 µs (0.450 ms)
- **Median**: 160.6 µs (0.161 ms)
- **Mean (Average)**: **166.59 µs** (0.167 ms)
- **Budget**: < 15.0 ms (Target achieved with >98% margin)

### 2.2 Genuine Cold Process Launch (Release Binary)
- **Definition**: Fresh process execution of compiled release binary `omen.exe doctor --json` measured from process creation to exit.
- **Sample Size**: 10 iterations
- **Min**: 12.47 ms
- **Max**: 17.11 ms
- **Median**: 13.40 ms
- **Mean (Average)**: **13.69 ms**
- **Analysis**: Typical Windows process creation and dynamic linking overhead ranges from 10–25 ms. Omen initializes the engine and inspects capabilities within this standard OS scheduling window.

### 2.3 Completion Engine with Representative Hot-Index State
- **Definition**: Latency of `OmenCompleter` returning fuzzy-ranked suggestions across a populated `HotSemanticIndex` containing:
  - 50 active facts (25 Current, 25 Dirty with elevation)
  - 20 workspace directory entries
  - Atlas tools (`cargo`, `git`, `threadmoth`, `ripgrep`)
  - Semantic actions (`:status`, `:doctor`, `:why`, etc.)
  - Dynamic references (`@last`, `@failed`, `@errors`, etc.)
- **Sample Size**: 100 queries
- **Min**: 11.4 µs
- **Max**: 520.4 µs
- **Median**: 39.3 µs
- **Mean (Average)**: **134.29 µs** (0.134 ms)
- **Budget**: < 5.0 ms (Target achieved with >97% margin)

### 2.4 Semantic Grammar Scanner Throughput
- **Definition**: Lexical analysis and token splitting across 10,000 queries including complex Windows drive paths, UNC paths, and quoted arguments.
- **Sample Size**: 10,000 invocations
- **Total Time**: 14.50 ms
- **Average Latency**: **1.45 µs** (1,449 ns)
- **Throughput**: **689,841 scans/second**
- **Budget**: > 100,000 scans/second (Target achieved with ~7x margin)

---

## 3. Summary Performance Matrix

| Metric | Target Budget | Measured Actual | Assessment |
| :--- | :--- | :--- | :--- |
| **In-Process Prompt Construction** | < 15.0 ms | **0.167 ms** (166.6 µs) | **PASS** |
| **Genuine Cold Process Launch** | < 50.0 ms | **13.69 ms** | **PASS** |
| **Hot-Index Completion Latency** | < 5.0 ms | **0.134 ms** (134.3 µs) | **PASS** |
| **Grammar Scanner Throughput** | > 100,000 scans/s | **689,841 scans/s** | **PASS** |
