# OMEN 0.7 — ARCHITECTURE DOCTRINE & STANDING AI-FIRST ENGINEERING RULES

## Status
```text
STATUS: CANONICAL DOCTRINE
SCOPE: OMEN ARCHITECTURE & FUTURE DEVELOPMENT
INCORPORATED: AGENTS.md, docs/evidence/0.7/ARCHITECTURE_DOCTRINE.md
```

---

## 1. System Architectural Boundary

> **Lantern knows. Resolve coordinates. Tethers controls. Omen makes the machine legible and enforceable. ThreadMoth mutates deterministically.**

- **Lantern** knows (durable memory, long-term context, and provenance).
- **Resolve** coordinates (live guard tokens and scope locks).
- **Tethers** controls (permission, trusted capability identity, policy, approval, durable intent, replay, and provider outcome truth).
- **Omen** makes the machine legible and enforceable (physical execution, containment, typed resources, machine-readable facts, and artifact CAS evidence).
- **ThreadMoth** deterministically mutates (bounded structural edits with cryptographic pre/post hashes and refusal).

---

## 2. The 30 Standing AI-First Engineering Rules

### Rule 1: Nothing May Wait Forever
Any operation involving an external boundary or asynchronous coordination must be bounded.
This includes: subprocesses, LSP servers, network/provider calls, IPC, channels, task joins, locks, filesystem watches, plugin/tool calls, model calls, background workers, initialization, shutdown.
Every operation must define:
1. Hard deadlines / timeouts with cancellation paths.
2. Useful failure modes with structured error reporting.
3. Explicit resource cleanup (killing processes, aborting reader tasks, closing channels).
4. Diagnostic evidence identifying where execution ceased.
Tests must additionally have an outer watchdog. Production timeout protects Omen; test timeout protects development. Both are mandatory.

### Rule 2: Failure Must Return Control
Failure is not allowed to strand the caller. After a timeout, malformed response, crashed provider, dead subprocess, invalid tool result, or interrupted operation:
- Control must return to the caller.
- Resources must be released.
- Subsequent operations must remain usable.
- Corrupted or ambiguous state must not be silently retained.
"Error returned but subsystem remains poisoned" is still a failure.

### Rule 3: Failures Must Name the Phase
Errors should identify where the operation failed. Prefer `semantic.lsp.definition.request.timeout` over generic `operation failed`.
Useful phase boundaries include: `discover`, `resolve`, `spawn`, `initialize`, `request`, `receive`, `validate`, `commit`, `shutdown`, `join`.
An AI agent should not need to reconstruct the execution path from prose and stack traces when Omen already knows it.

### Rule 4: Machine-Readable Truth Before Human-Friendly Presentation
Where Omen knows structured facts, preserve them structurally. Human text is a presentation layer, not canonical state.
Commands and subsystems expose stable structured representations for status, errors, capabilities, paths, symbols, actions, provenance, timings, resource usage, and diagnostics.

### Rule 5: Do Not Make the Model Rediscover Known Facts
If Omen can determine something deterministically, do so.
Examples: current working directory, repository root, Git branch/state, workspace packages, symbol definitions, file identity, language/runtime metadata, tool availability, environment facts, known capabilities.
Model reasoning is reserved for ambiguity, synthesis, and human interpretation — never as an expensive fallback for missing substrate queries.

### Rule 6: Zero-Model Paths Must Really Be Zero-Model
When a task is classified as deterministic, no hidden fallback may silently invoke a model.
This applies to retries, error handling, completion helpers, semantic fallback, formatting, and recovery paths. If a deterministic path cannot complete deterministically, return a bounded explicit result describing what is missing.

### Rule 7: One Semantic Authority Per Fact
A consequential fact must have one canonical owner. Avoid multiple parts of Omen independently deciding repository root, language, workspace membership, active project, symbol identity, capability, action result, or execution state.

### Rule 8: Make State Transitions Explicit
Important work must move through named states rather than scattered booleans (e.g. `discovered`, `admitted`, `resolving`, `executing`, `completed`, `failed`, `cancelled`, `timed_out`). Illegal transitions must fail visibly.

### Rule 9: Partial Success Must Be Representable
Do not collapse "8 things succeeded, 1 failed" into a binary outcome. Represent partial completion explicitly: completed work, failed work, unresolved work, retryability, and evidence.

### Rule 10: Operations Should Be Resumable Where Practical
Long or multi-stage operations should leave enough durable evidence to continue after interruption. Expose checkpoints, CAS artifacts, receipts, or stable intermediate results.

### Rule 11: Retries Must Be Intentional
Never hide infinite or uncontrolled retry loops. Every retry policy must define retryability criteria, maximum attempts, backoff, idempotency, retained evidence, and final failure behaviour.

### Rule 12: Mutations Should Be Idempotent or Identified
Repeated execution of the same intended action should either produce the same state or be recognised through a stable action ID.

### Rule 13: Reads and Writes Must Be Distinguishable
A tool/action schema must clearly communicate whether it reads, computes, mutates local state, mutates external state, spawns a process, or performs network IO.

### Rule 14: Capabilities Should Be Narrow and Composable
Prefer small explicit capabilities (`fs.read`, `fs.write`, `process.spawn`, `git.read`) over broad ambient authority (`system.access`).

### Rule 15: Context Is a Resource
Omen returns the smallest complete representation required for the task: summaries plus references, structured metadata, stable IDs, lazy detail retrieval, and pagination.

### Rule 16: Large Results Should Become References
When output exceeds bounded limits, store it in the Content-Addressed Storage (CAS) and return an `artifact://` URI plus concise metadata.

### Rule 17: Provenance Should Travel With Important Facts
Results must identify how they were obtained (`cargo_metadata`, `lsp`, `scip_index`, `filesystem`, `git`, `model:...`, `user`).

### Rule 18: Uncertainty Must Not Be Disguised as State
Omen explicitly distinguishes `known`, `inferred`, `unknown`, `unavailable`, `stale`, and `conflicting`.

### Rule 19: Stale Data Must Be Identifiable
Caches, semantic indexes, and derived state require freshness semantics and witness digests. Stale indexes report `Stale(T)`, never `Resolved(T)`.

### Rule 20: Observability Without Log Soup
Operations expose concise structured telemetry (`operation_id`, `phase`, `duration_ms`, `source`, `cache_hit`, `provider_calls`, `model_calls`). Detailed traces are available on demand.

### Rule 21: Performance Regressions Are Semantic Regressions When They Hurt Agents
Excessive latency, model calls, provider calls, context growth, or unnecessary IO constitute correctness failures. Tests assert `model_calls == 0`, `provider_calls == 0`, and strict latency bounds.

### Rule 22: Test Invariants, Not Implementation Accidents
Tests prove externally meaningful behaviour and contractual invariants, not ephemeral internal refactor targets.

### Rule 23: Tests Must Themselves Be Engineered Systems
Tests own their resources, bound their waits, clean up background tasks and child processes, produce diagnostic evidence, and behave deterministically. A hanging test is a defect in the test harness.

### Rule 24: Cross-Platform Is the Default
Unless explicitly declared platform-specific, code and tests must be portable across Windows, Linux, and macOS without hard-coding shell syntax, drive letters, `/tmp`, or path separators.

### Rule 25: The Harness Must Not Become the Product
Every new abstraction must earn its existence by improving determinism, safety, agent reasoning, portability, observability, performance, maintainability, or resumability.

### Rule 26: Optimise for Recovery, Not Perfection
Assume tools fail, providers crash, context truncates, and machines reboot. The runtime ensures failure destroys as little work and information as possible.

### Rule 27: The Agent Should Never Have to Guess What Omen Knows
Expose system knowledge explicitly; do not bury consequential state in prose, terminal logs, or hidden globals.

### Rule 28: Consequences Must Be Visible Before Execution
Expose anticipated side effects (files read, files written, processes spawned, network calls, model calls) before committing actions.

### Rule 29: Prefer Boring Determinism Below Intelligent Reasoning
Lower layers (parsing, routing, state transitions, permissions, mutation, caching, evidence) are deliberately deterministic. Intelligence belongs above that foundation.

### Rule 30: Every Important Claim Should Be Provable
Guarantees must be turned into executable tests and proofs. Architecture claims without executable evidence decay into folklore.

---

## 3. Concrete 0.7 Proof Mapping

| Rule | Implementation / Proof Mechanism | Test File / Proof |
|:-----|:---------------------------------|:------------------|
| **Rule 1: Nothing May Wait Forever** | `DEFAULT_LSP_TIMEOUT`, 16 MiB message hard-cap, layered phase timeouts | `semantic_environment_proofs::test_proof_d_lsp_timeout_and_late_response_are_bounded` |
| **Rule 2: Failure Must Return Control** | Late response discard, clean child process kill on shutdown and Drop | `semantic_environment_proofs::test_proof_d_lsp_timeout_and_late_response_are_bounded` |
| **Rule 3: Failures Must Name Phase** | Layered phase runners with structured error tagging | `semantic_environment_proofs::test_harness_watchdog_triggers_and_cleans_up_on_stalled_dependency` |
| **Rule 5: Avoid Model Rediscovery** | Deterministic symbol routing, package semantics, workspace manifests | `semantic_environment_proofs::test_proof_g_cargo_workspace_metadata_is_canonical` |
| **Rule 6: Zero-Model Paths** | `DeterministicClassifier`, `provider_calls == 0`, `semantic_provider_calls >= 1` | `semantic_environment_proofs::test_proof_i_deterministic_symbol_question_uses_zero_model_calls` |
| **Rule 7: One Semantic Authority** | Shared `SemanticProviderRegistry`, unified builder for interactive & MCP | `semantic_environment_proofs::test_proof_l_product_path_wiring_through_def_action_and_lane` |
| **Rule 19: Stale Data Identifiable** | SCIP Protobuf witness digests, `SemanticLookupResult::Stale(T)` | `semantic_environment_proofs::test_proof_e_scip_symbol_query_and_stale_index_detection` |
| **Rule 21: Performance as Correctness** | In-memory hot completion < 5ms with zero provider I/O | `semantic_environment_proofs::test_proof_j_hot_completion_uses_zero_provider_io` |
| **Rule 23: Engineered Test Harness** | Outer watchdogs, layered timeouts, child process reaping, no orphan tasks | `semantic_environment_proofs::test_harness_watchdog_triggers_and_cleans_up_on_stalled_dependency` |
| **Rule 24: Cross-Platform Default** | PathBuf URI normalization, multilingual UTF-8/UTF-16 span conversions | `semantic_environment_proofs::test_proof_k_multilingual_coordinate_normalization` |
| **Rule 30: Provable Claims** | All 12 proof suites (A through L) verified in CI and local verification | `semantic_environment_proofs.rs` (13/13 passing) |
