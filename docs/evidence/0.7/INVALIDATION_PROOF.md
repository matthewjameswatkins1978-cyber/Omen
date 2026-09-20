# Omen 0.7 Semantic Environment — Witness & Invalidation Proof

## 1. Objective

Prove that:
1. `SemanticCache` associates cached semantic results with cryptographic file witnesses (`SemanticWitness`).
2. Source file modification invalidates the witness (`CURRENT -> DIRTY`).
3. Omen strictly adheres to the lazy pessimism doctrine: it never silently returns stale facts when `CURRENT` is expected, and never silently triggers background re-indexing without explicit caller dispatch.

---

## 2. Invalidation Workflow (Proof F)

In `crates/omen-cli/tests/semantic_environment_proofs.rs` (`test_proof_f_semantic_result_invalidates_after_source_change`):

### Fixture Setup
- Workspace source file `src/auth.rs`.
- `SemanticCache` initialized with generation version 1.
- Initial query resolves `refresh_token` and inserts entry into cache with a witness tracking `src/auth.rs` and its SHA-256 hash.

### Verification Steps
1. Initial Observation:
   - Cache validity reports `WitnessValidity::Current`.
   - Generation version is 1.
2. File Mutation:
   - Content of `src/auth.rs` is modified.
   - Cache receives `invalidate_file("src/auth.rs")`.
3. Post-Mutation State:
   - `cache.is_file_dirty("src/auth.rs")` returns `true`.
   - Subsequent retrieval detects the dirty witness.
   - The cache refuses to present the entry as `Current`.
   - The caller is informed that the fact is `Dirty`, requiring explicit re-querying or provider refresh.
