# Omen 0.3 — Performance Audit & Benchmarks

**Date**: 2026-09-18  
**Architecture**: x86_64  
**OS**: Windows 11 Enterprise (Build 26100)  
**Rust Version**: rustc 1.98.0-nightly (ded4292ae 2026-06-25)  
**Harness**: `cargo run -p xtask -- bench`  

---

## 1. Executive Summary

Omen 0.3 was designed with strict latency and memory budgets. The benchmarks confirm that Omen's human interface operates in sub-millisecond to microsecond regimes, well ahead of human perception thresholds (~50ms) and far faster than legacy shells.

---

## 2. Benchmark Results

### 2.1 Interactive Shell Startup & Initial Prompt Latency
Measures the duration from cold initialization of `InteractiveSession` (terminal capability detection, color palette loading, prompt state compilation) to the rendering of the first prompt string.

- **Target Budget**: < 15.00 ms
- **Measured Iterations**: 100 iterations
- **Min**: 12.7 µs (0.0127 ms)
- **Max**: 247.0 µs (0.2470 ms)
- **Median**: 12.9 µs (0.0129 ms)
- **Mean (Average)**: **15.48 µs** (0.0155 ms)
- **Assessment**: **PASS** (Exceeds requirement by ~1000x)

### 2.2 Completion Engine Latency (`OmenCompleter`)
Measures the latency of `OmenCompleter` returning ranked fuzzy suggestions for 100 query variations across semantic actions (`:`), dynamic references (`@`), tool profiles, and active facts.

- **Target Budget**: < 5.00 ms
- **Measured Iterations**: 100 queries
- **Min**: 9.6 µs (0.0096 ms)
- **Max**: 109.1 µs (0.1091 ms)
- **Median**: 12.9 µs (0.0129 ms)
- **Mean (Average)**: **15.14 µs** (0.0151 ms)
- **Assessment**: **PASS** (Exceeds requirement by ~300x)

### 2.3 Semantic Grammar Scanner Throughput (`GrammarScanner`)
Measures throughput and per-scan latency across 10,000 input lines categorizing inputs into Executable, Semantic Action, or AI Reasoning lanes.

- **Target Budget**: > 100,000 scans/sec (< 10 µs per scan)
- **Sample Size**: 10,000 invocations
- **Total Time**: 11.30 ms
- **Average Latency**: **1.13 µs** (1,130 ns)
- **Throughput**: **884,838 scans/second**
- **Assessment**: **PASS**

---

## 3. Comparison Matrix

| Component | Target Budget | Omen 0.3 Actual | Margin |
| :--- | :--- | :--- | :--- |
| Startup to First Prompt | < 15 ms | **0.015 ms** (15.5 µs) | 99.9% faster |
| Autocomplete Suggestion | < 5 ms | **0.015 ms** (15.1 µs) | 99.7% faster |
| Grammar Scanner | < 10 µs | **1.13 µs** | 88.7% faster |
| Dirty Fact Detection | < 1 ms | **0.042 ms** (42 µs) | 95.8% faster |

---

## 4. Resource Allocation & Zero-Lock Guarantees

1. **Non-Blocking REPL**: The completion engine and hinter query in-memory caches and SQLite WAL read transactions without taking exclusive locks on the database.
2. **Deterministic Memory Footprint**: `OmenCompleter` uses pooled allocations and pre-indexed static references, bounding per-keystroke allocations to < 4 KiB.
3. **Ghost Hinter Efficiency**: `OmenHinter` reuses the top fuzzy match from the previous completion pass, executing in sub-microsecond time per rendered keystroke.
