# Omen 0.4 Evidence — Recovery & Event Gap Proof

**Document**: `docs/evidence/0.4/RECOVERY_PROOF.md`  
**Status**: VERIFIED  
**Crates**: `omen-daemon`, `omen-client`, `omen-knowledge`

---

## 1. Event Sequence Gap Detection & Self-Healing

In Omen's shared runtime, event sequence continuity is strictly enforced:
- Every event emitted by a daemon workspace carries a monotonically increasing `sequence` number (1, 2, 3...).
- The client background event loop tracks the last observed sequence number.
- If a sequence gap is detected (`event.sequence > prev_seq + 1`), the client:
  1. Immediately synthesizes a high-priority `ResyncRequired` event explaining the gap (`"Event gap detected: expected X, received Y"`).
  2. Dispatches `ResyncRequired` to internal subscribers.
  3. Re-requests a fresh snapshot of the Hot Semantic Index (`GetSnapshot`) before processing further events.

### Automated Test Evidence
- `crates/omen-daemon/tests/event_gap_resync_tests.rs`:
  - `test_real_sequence_gap_triggers_resync`: Simulates network/pipe packet drop (seq 1 followed by seq 3). Proves `ResyncRequired` is emitted immediately before delivering seq 3.
- `crates/omen-daemon/tests/e2e_shared_runtime_proofs.rs`:
  - `proof_d_reconnect_resync_catchup`: Client reconnects after downtime and recovers hot index state via snapshot catchup.

---

## 2. Daemon Restart & Epoch Invalidation

When `omend` crashes or is restarted:
1. It assigns a new monotonic `epoch` identifier (timestamp millis).
2. Connected or reconnecting clients compare the event's `epoch` against their cached `last_epoch`.
3. If `event.epoch != prev_epoch`, the client invalidates any cached sequence assumptions and triggers `ResyncRequired { reason: "Epoch changed; daemon restarted" }`.
4. The client fetches a fresh authoritative snapshot, ensuring no stale in-memory state lingers across daemon generations.

### Automated Test Evidence
- `crates/omen-daemon/tests/event_gap_resync_tests.rs`:
  - `test_daemon_epoch_change_forces_resnapshot`: Emits epoch 1 event, then epoch 2 event. Proves client intercepts epoch mismatch and forces full resynchronization.

---

## 3. Consequential Execution Identity & Crash Reconciliation

Omen doctrine mandates:
> **Consequential truth must be explicit. Never retry ambiguous operations.**

When an execution request is submitted through the daemon broker:
1. A unique `ExecutionId` is generated server-side.
2. A request receipt is immediately written to SQLite with `status = "Running"` and the assigned `execution_id`.
3. If the daemon crashes while the command is running:
   - Upon daemon restart, `WorkspacePersistence::reconcile_running_receipts` runs during initialization.
   - Any receipt found with `status = "Running"` transitions to `status = "Unknown"`.
4. If a client attempts to retry the same `consequential_request_id` after restart:
   - The broker observes `status == "Unknown"`.
   - The broker **refuses** re-execution with `LocalIpcError::ExecutionStatusUnknown`.
   - Destructive operations are never blindly executed twice.

### Automated Test Evidence
- `crates/omen-daemon/tests/consequential_identity_and_restart_tests.rs`:
  - `test_receipt_execution_id_matches_history_execution_id`: Proves single server-owned execution identity across receipt, database history, and completion response.
  - `test_running_receipt_after_crash_becomes_unknown_not_retried`: Proves running receipt converts to `Unknown` on restart, and re-execution attempt is refused closed.
  - `test_consequential_request_survives_daemon_restart_without_reexecution`: Proves completed receipts survive daemon restart and return cached results without re-executing.
