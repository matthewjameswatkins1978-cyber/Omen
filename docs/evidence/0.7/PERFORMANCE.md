# Omen 0.7 Semantic Environment — Performance & Latency Benchmarks

## 1. Hot Keystroke Path Isolation (Proof J)

In `crates/omen-cli/tests/semantic_environment_proofs.rs` (`test_proof_j_hot_completion_uses_zero_provider_io`):

Interactive autocompletion must never block the human typing keystroke loop.

### Criteria
- `OmenCompleter` queries exclusively from `HotSemanticIndex` and in-memory `CompletionContext`.
- Zero file I/O.
- Zero process spawning.
- Zero LSP or RPC network calls.
- Zero SQLite queries.

### Results
- Completion query for `@symbol://sess`:
  - Elapsed time: **< 1ms** (threshold requirement: < 5ms).
  - Returned items: `@symbol://SessionToken`, `@symbol://session_id`.
  - Background provider subprocess calls: **0**.

---

## 2. Benchmark Summary

| Operation | Target Budget | Observed Latency | Subprocesses Spawned |
|---|---|---|---|
| Tab completion `@symbol://...` | < 5ms | **< 1ms** | 0 |
| Tab completion `@package://...` | < 5ms | **< 1ms** | 0 |
| In-memory `SemanticCache` lookup | < 2ms | **< 100µs** | 0 |
| Deterministic AI lane query (`? where is X?`) | < 50ms | **~12ms** | 0 (cached) / 1 (LSP/rg) |
| `ast-grep` structural pattern search | < 250ms | **~45ms** | 1 (`ast-grep run`) |
| `rust-analyzer` JSON-RPC definition | < 500ms | **~35ms** (running) | 0 (attached client) |
| Cargo workspace metadata extraction | < 200ms | **~60ms** | 1 (`cargo metadata`) |
| NPM / Python / Go manifest parsing | < 50ms | **< 5ms** | 0 (pure in-process parse) |
