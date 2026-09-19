# Omen 0.4 — Shared Runtime Proof Matrix (Proofs A–J)

**Release**: Omen 0.4.0  
**Harness**: `crates/omen-daemon/tests/e2e_shared_runtime_proofs.rs`  
**Status**: 10 / 10 PASSED  

---

## 1. Overview

Omen 0.4's central guarantee is:
> **One Omen reality across processes.**

To prove this without hand-waving or mocking out transport layers, the hostile test suite in `e2e_shared_runtime_proofs.rs` executes 10 end-to-end proofs across separate client connections, verifying concurrent state synchronization, crash recovery, deduplication, and isolation boundaries.

All proofs enforce a strict 5-second per-test timeout to prevent any test from stalling or hanging indefinitely.

---

## 2. The 10 Core Proofs

### Proof A: Two Shells, One Fact
- **Scenario**: Two independent human shells connect to the same workspace.
- **Verification**:
  - Fact published in the workspace is immediately broadcast and received by both shells.
  - Fact invalidation (`new_validity: DIRTY`) is broadcast and observed simultaneously on both shells.
- **Result**: `proof_a_two_shells_one_fact ... ok`

### Proof B: Background Mutation Marks Dirty
- **Scenario**: A human interactive shell and a background agent client occupy the same workspace.
- **Verification**:
  - The agent executes a file mutation touching workspace files.
  - The human shell's hot index receives a `FactInvalidated` push event, elevating the fact to `DIRTY` on the prompt without requiring manual polling.
- **Result**: `proof_b_background_mutation_marks_dirty ... ok`

### Proof C: External Watcher Invalidation
- **Scenario**: An external tool or editor modifies files on disk outside of Omen's process tree.
- **Verification**:
  - The filesystem watcher (`notify`) detects the change and dispatches an invalidation event across the local IPC event bus.
  - All connected clients observe the transition from `CURRENT` to `DIRTY`.
- **Result**: `proof_c_external_watcher_invalidates_facts ... ok`

### Proof D: Event Gap Handling and Resync
- **Scenario**: Network congestion or queue overflow causes an event sequence gap.
- **Verification**:
  - Server emits a `ResyncRequired` event with reason.
  - Client catches the gap and automatically re-fetches a fresh `GetSnapshot` to restore consistent state.
- **Result**: `proof_d_event_gap_handling_and_resync ... ok`

### Proof E: Consequential Request Deduplication & Cached Outcome
- **Scenario**: An agent or script submits a consequential execution with a idempotency key (`consequential_request_id`).
- **Verification**:
  - First execution runs to completion and records exit code and output.
  - Second execution with the identical key immediately returns the cached outcome with identical `execution_id` without re-running the command.
  - Querying status by request ID verifies `ExecutionStatusCode::Completed`.
- **Result**: `proof_e_consequential_request_deduplication_and_cached_outcome ... ok`

### Proof F: Daemon Restart & State Healing
- **Scenario**: Daemon terminates and restarts while sessions are active.
- **Verification**:
  - Workspace state directory (`.omen`) persists SQLite history and metadata.
  - On restart, the new daemon instance reads persisted state, increments workspace epoch, and client queries for history successfully return previous session records.
- **Result**: `proof_f_daemon_restart_and_state_healing ... ok`

### Proof G: Shared Managed Services Across Clients
- **Scenario**: Client 1 starts a long-running service (`proc://workspace/worker_proc`).
- **Verification**:
  - Client 2 queries `list_services()` and observes the service in `running` state with PID.
  - Client 2 issues `stop_service("worker_proc")`.
  - Client 1 queries `list_services()` and observes the service transitioned to `stopped`.
- **Result**: `proof_g_shared_managed_services_across_clients ... ok`

### Proof H: Large Output Spools to CAS
- **Scenario**: A command outputs 128 KiB of data exceeding the inline transcript limit (4–8 KiB).
- **Verification**:
  - Daemon truncates the inline transcript and returns a Content-Addressed Storage URI: `artifact://sha256/<hash>`.
  - The actual blob file exists on disk in `.omen/cas/sha256/<prefix>/<hash>` with full unreduced byte length (>= 131,072 bytes).
- **Result**: `proof_h_large_output_spools_to_cas ... ok`

### Proof I: Protocol Mismatch Refusal
- **Scenario**: A client with an unsupported protocol version (e.g. `999`) attempts connection.
- **Verification**:
  - Daemon inspects `ClientHello` and refuses connection by closing the stream without sending `DaemonHello`.
- **Result**: `proof_i_protocol_mismatch_refusal ... ok`

### Proof J: Strict Workspace Isolation
- **Scenario**: Two distinct workspaces (`Workspace A` and `Workspace B`) are managed concurrently by the daemon.
- **Verification**:
  - Workspaces receive distinct deterministic IDs (`ws-<sha256>`).
  - Facts published in Workspace A never appear in snapshots or event streams of Workspace B.
  - Services started in Workspace A never appear in service listings of Workspace B.
- **Result**: `proof_j_strict_workspace_isolation ... ok`

---

## 3. Test Execution Log

```text
running 10 tests
test proof_i_protocol_mismatch_refusal ... ok
test proof_a_two_shells_one_fact ... ok
test proof_d_event_gap_handling_and_resync ... ok
test proof_b_background_mutation_marks_dirty ... ok
test proof_c_external_watcher_invalidates_facts ... ok
test proof_f_daemon_restart_and_state_healing ... ok
test proof_h_large_output_spools_to_cas ... ok
test proof_e_consequential_request_deduplication_and_cached_outcome ... ok
test proof_g_shared_managed_services_across_clients ... ok
test proof_j_strict_workspace_isolation ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.13s
```
