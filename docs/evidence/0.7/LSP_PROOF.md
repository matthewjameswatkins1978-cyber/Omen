# Omen 0.7 Semantic Environment — Language Server Protocol Proof

## 1. Objective

Prove that:
1. `RustAnalyzerProvider` integrates cleanly with `rust-analyzer` via asynchronous stdio JSON-RPC without blocking runtime threads.
2. The LSP client enforces strict timeouts, request cancellation (`$/cancelRequest`), and safe discarding of late responses without memory leak or corruption.
3. Hostile LSP conditions (slow init, unresponsive server, delayed replies) are bounded.

---

## 2. Real LSP Symbol Definition and References (Proof C)

In `crates/omen-cli/tests/semantic_environment_proofs.rs` (`test_proof_c_real_lsp_symbol_definition_and_references`):

- Tested against real `rust-analyzer` installed in system PATH.
- Dispatched `textDocument/didOpen` notification to guarantee in-memory VFS synchronization.
- Queried definition of `SessionToken` struct at `src/lib.rs:0:11`.
- Resolved location in bounded time with zero deadlocks or panics.
- On environments without `rust-analyzer`, gracefully reports `SemanticLookupResult::Unsupported` without crashing.

---

## 3. Bounded Timeouts & Hostile LSP Cancellation (Proof D)

In `crates/omen-cli/tests/semantic_environment_proofs.rs` (`test_proof_d_lsp_timeout_and_late_response_are_bounded`):

Using `omen-gremlin` in `--lsp-mode`:
1. **Slow-Init Timeout**:
   - `HostileLspServer` configured in `SlowInit` mode (sleeps 500ms before replying to `initialize`).
   - Client timeout configured to 150ms.
   - Result: Client aborts promptly at ~150ms with `CoreError::Timeout`, well under the 600ms safety threshold.
2. **Cancellation & Late-Response Discarding**:
   - `HostileLspServer` configured in `LateResponse` mode (waits 300ms before sending response).
   - Client sends request with 50ms timeout.
   - Upon timeout expiration, client immediately removes the pending request ID from its tracking map and emits `$/cancelRequest` notification to the server.
   - When the server's delayed response eventually arrives at 300ms, the client reader safely drops the orphaned response without error, deadlock, or channel panic.
   - Verifies `client.pending_count() == 0`.
