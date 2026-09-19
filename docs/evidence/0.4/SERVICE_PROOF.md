# Omen 0.4 Evidence — Service Truth & Process Supervision Proof

**Document**: `docs/evidence/0.4/SERVICE_PROOF.md`  
**Status**: VERIFIED  
**Crates**: `omen-daemon`, `omen-engine`, `omen-knowledge`

---

## 1. Managed Service Truth Model

Omen strictly distinguishes between child processes it directly owns and processes surviving in the operating system:
- `RUNNING_OWNED`: Process was spawned by the current daemon instance and has a live child handle for monitoring, signal delivery, and containment.
- `OBSERVED`: Process has an active OS PID found in SQLite records from a prior daemon instance, but the current daemon does not hold the parent process handle.
- `STOPPED`: Process was cleanly terminated, reaped, and confirmed dead.
- `CRASHED`: Process unexpectedly exited with non-zero status or signal.

### Platform Truthfulness Invariant
> **If Omen does not hold the process ownership handle, it never claims RUNNING_OWNED.**  
> **If the process is alive in the OS, Omen never falsely claims STOPPED in SQLite.**

---

## 2. Daemon Restart & State Healing

When `omend` restarts with surviving background processes:
1. SQLite records with `state == "running"` are scanned.
2. For each PID:
   - If the PID is dead in the OS: marked as `crashed`.
   - If the PID is alive in the OS: marked as `observed` (NOT `running`).
3. If a client attempts to call `stop_service` on an `observed` process without an active handle:
   - The daemon refuses control with `LocalIpcError::LocalPeerDenied("Daemon lacks process ownership handle...")`.
   - The daemon **refuses** to write `state = "stopped"` into SQLite because the process is still running on the machine.

### Automated Test Evidence
- `crates/omen-daemon/tests/service_truth_tests.rs`:
  - `test_service_restart_reconciliation_distinguishes_observed`: Proves surviving PID is marked `observed`, never `running`.
  - `test_service_restart_reconciliation_never_claims_false_stop`: Proves stop request is refused and SQLite state remains `observed` rather than falsely asserting `stopped`.
- `crates/omen-daemon/tests/shared_services_tests.rs`:
  - `test_shared_services_supervision_and_two_clients`: Client 1 starts service; Client 2 inspects and stops it.
  - `test_service_crash_reconciliation_upon_daemon_start`: Dead PID cleanly marked `crashed` on daemon start.
- `crates/omen-daemon/tests/e2e_shared_runtime_proofs.rs`:
  - `proof_g_shared_managed_services_across_clients`: Cross-process supervision verified.
