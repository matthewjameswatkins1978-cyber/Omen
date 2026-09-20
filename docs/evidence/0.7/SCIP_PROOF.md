# Omen 0.7 Semantic Environment — SCIP Index & Stale Detection Proof

## 1. Objective

Prove that:
1. `ScipProvider` parses Source Code Intelligence Protocol (SCIP) indexes into structured `SymbolRecord` models.
2. The provider validates file hashes against index metadata, accurately flagging `is_stale()` when source files drift from the index state.

---

## 2. SCIP Query & Stale Detection (Proof E)

In `crates/omen-cli/tests/semantic_environment_proofs.rs` (`test_proof_e_scip_symbol_query_and_stale_index_detection`):

### Fixture Setup
- Workspace source file `src/service.rs`: `pub fn process_order(id: u64) -> bool { true }`.
- SCIP index `index.scip` containing symbol definition `scip-rust cargo omen-core 0.1.0 service/process_order().`.
- Recorded source file hash matching initial `src/service.rs`.

### Verification Steps
1. Initial Query:
   - `scip_provider.symbol_search("process_order", 10)` resolves the symbol to `src/service.rs:0:7`.
   - `scip_provider.is_stale()` reports `false`.
2. Source Code Mutation:
   - `src/service.rs` is modified to `pub fn process_order(id: u64) -> bool { false }`.
   - Re-evaluating `scip_provider.is_stale()` returns `true`.
   - Provider refuses to report stale index data as fresh `CURRENT` truth without re-indexing.
