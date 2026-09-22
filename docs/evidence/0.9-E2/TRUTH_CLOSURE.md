# E2 — Truth, Authority & Durable History Closure: semantics record

Status of each item: **implemented + proven** unless marked otherwise.
Proven = Rust and/or Python tests named below, all bounded (outer
watchdog + named phases; a timeout names the wedged phase).

Repair pass (Preview 10): terminal-dedup hardening, pre-dispatch cancel
truth, persistence fail-closed behaviour, and removal of the speculative
Omen authority ontology. Proofs in `e2_repair_tests` (7). The repair
changes runtime bytes, so Preview 9 (rejected candidate) is superseded by
Preview 10; machine contract stays `0.8` (additive only).

## E2.1 Durable history: read-after-write point

- **Visibility point**: `ExecutionHistory::record_execution` transaction
  commit. After commit, the CLI/MCP read-only path
  (`canonical_workspace_db_path_readonly` + `Database::open_read_only` +
  `query_history`) observes the execution while the daemon is still alive.
  Daemon shutdown is never required to reveal completed work.
- **Root cause fixed**: readers used `immutable=1`, freezing them at the
  last checkpoint and hiding the daemon's WAL — including the schema
  migration itself (`no such table` on a live database). Readers now open
  read-only without `immutable` (SHARED locks, committed WAL frames
  visible) with a bounded 5s `busy_timeout`; they still create/migrate
  nothing (fresh state still reports `PersistenceFailure`, genuinely
  unknown, not false absence).
- **Durability contract**: WAL + `synchronous=NORMAL` = durable against
  process crash (kill -9 safe; proven by restart/crash tests). NOT
  contracted against OS crash / power loss inside the checkpoint window —
  that uncertainty is documented here, not hidden.
- Proofs: `e2_durable_history_tests` (live visibility, 3-execution
  order/identity), `db::tests::read_only_reader_sees_live_writer_commits`.

## E2.2 Recovery guarantees

- **Restart**: new `WorkspaceState` epoch reconciles `Running` AND
  `CancellationRequested` receipts to `Unknown`; completed receipts and
  history survive with identical execution IDs; resubmission with the same
  dedup key replays the recorded summary without re-executing.
- **Crash**: a `Running` receipt with no live task stays `Unknown` —
  uncertainty, never a phantom completion (proven: zero history rows for
  the abandoned ID) and never a silent retry (resubmit refuses with
  `ExecutionStatusUnknown`).
- **Response lost + reconnect + retry**: same dedup key returns the
  original execution; history holds exactly one record for it.
- **Replay truth**: the broker stamps every history record with an
  explicit status envelope (`RuntimeStatus::history_label`, one semantic
  authority); dedup replay reproduces the recorded runtime (including
  `TimedOut`), not a hardcoded `Completed`.
- Proofs: `e2_recovery_tests` (3), `consequential_identity_and_restart_tests`
  (pre-existing, still green).

## E2.3 Cancellation / stop / revocation truth

States (never collapsed):

| Layer | States |
|---|---|
| Intent (receipt) | `Running` → `CancellationRequested` (fire) |
| Physical (RuntimeStatus) | `Completed` / `TimedOut` / `Cancelled` (death observed after kill) / `OutcomeUnknown` (kill unconfirmed) / `SpawnFailed` / `ContainmentFailed` / `IoFailed` |
| Receipt terminal | `Completed` / `Failed` / `Cancelled` / `Unknown` |
| Cancel report | `TerminationConfirmed` / `DispatchPrevented` / `AlreadyFinished{terminal}` / `OutcomeUnknown` / `NotFound` |
| Status query | `... / CancellationRequested / Cancelled / ...` (new codes) |

- `intent to stop != proof of stop`: the flag fires intent; the wait path
  kills the tree (job object / process group, same enforcement as the
  timeout path) and observes death within a bounded 5s grace. Unconfirmed
  death = `OutcomeUnknown`, never rewritten.
- Cancel-before-dispatch never spawns (proven with a spawn-counting stub
  backend: 0 spawns, `CANCELLED_BEFORE_DISPATCH`).
- Provider disconnect mid-flight or mid-cancel: Omen keeps owning the
  work; terminal truth is recorded and observable after reconnect.
- Restart around cancel: intent without a surviving handle reconciles to
  `Unknown`; cancel across the boundary reports `OutcomeUnknown`.
- Race discipline: a live task holds its `in_flight` broadcast until after
  its terminal receipt write, so a missed broadcast implies terminal
  receipt — re-read, don't trust transients (this exact race was caught by
  the cancel-storm test and fixed).
- Panic guard: a dying broker task releases coalesced subscribers with an
  error instead of stranding them.
- Surfaces: daemon IPC (`CancelExecution`/`CancelResult`), `OmenClient`,
  MCP `omen_cancel_execution` (brokered; standalone refuses explicitly),
  CLI `omen cancel <id> [--machine]`, Python `cancel_execution()`.
  Interactive issues no cancels (out of scope, documented); it projects
  cancel truth through shared history.
- Proofs: engine `engine_tests` (3 new), `e2_cancellation_tests` (6),
  `e2_hostile_tests` (3), MCP (4 new), CLI live test, Python (3 new).

## Repair 1 — terminal dedup (Preview 10)

- Governing invariant: **a consequential request ID may create physical
  work only when no durable receipt for that ID exists.** Once a receipt
  exists, the identity is consumed — no state silently falls through to
  dispatch. The receipt check runs AFTER claiming the in-flight slot, so a
  terminal write landing concurrently resolves (replay/refuse) instead of
  racing into a duplicate dispatch; coalesced subscribers receive the same
  resolution.
- `Completed` → replay recorded truth (refused, never re-executed, when
  the history is unreadable). `Cancelled` → replay recorded cancellation
  truth (never resurrect after restart). `Failed` → explicit
  `RequestDuplicate` refusal naming the terminal truth (never silently
  retry). `Unknown` → fail closed. `Running` / `CancellationRequested` /
  anything unexpected with no live owner → reconcile to `Unknown`, fail
  closed. Zero spawn on every path.
- Proofs: `e2_repair_tests`: cancelled-after-restart (same ID, one row),
  failed-retry refusal, ownerless Running/CancellationRequested fail-closed.

## Repair 2 — pre-dispatch cancel truth (Preview 10)

- Prevention of execution is not proof of termination:
  `CancelOutcome::DispatchPrevented` (no spawn, no tree-stop, no death
  claimed) is distinct from `TerminationConfirmed`. The typed
  `dispatch_prevented` flag travels engine signal
  (`CANCELLED_BEFORE_DISPATCH`) → summary → history envelope → cancel
  observer → replay — never inferred from prose.
- Proven by driving the actual broker race through the pre-spawn gate
  test seam (production never arms it): parked task + fired cancel +
  release → `DispatchPrevented`, zero spawns, one `CANCELLED` row, same
  execution ID. Live-kill control still reports `TerminationConfirmed`.

## Repair 3 — persistence fail-closed (Preview 10)

- Pre-dispatch: the `Running` receipt MUST persist or nothing spawns
  (`PersistenceFailure` with the execution identity, zero physical work).
- Post-execution: history failure preserves the physical outcome AND the
  durable failure explicitly (`PersistenceFailure{execution_id, stage,
  physical_outcome, detail}`); the receipt stays non-terminal so the
  identity can never dispatch again. Terminal-receipt failure preserves
  the recorded history and likewise never re-arms the identity.
- Deterministic narrow failpoint seam (`PersistenceFailpoint`; production
  always `Off`). Proofs: `e2_repair_tests` (3 persistence tests +
  fail-closed resubmits).

## Repair 4 — Tethers boundary (Preview 10)

- The speculative Omen-owned authority ontology (`CapabilityIdentity`,
  `ScopeIdentity`, `AdmissionRequest`, `AdmissionVerdict`,
  `NotAdmittedReason`, `LiveAdmissionStatus`, `check_live_admission`)
  is removed from `omen-core`. The blocked seam is evidence
  (`TETHERS_SEAM_ASSESSMENT.md`), not an invitation to design Tethers
  inside Omen. Seam remains `BLOCKED_BY_TETHERS` (acceptable per the
  agreed dependency rule).

## Execution identity (E2.5)

- One canonical `exec_<uuid>` minted once per physical execution (daemon
  broker) and fanned out to receipt, history, summary, status, and every
  surface. No surface remints on projection (MCP standalone bug fixed:
  minted ID was discarded and nothing recorded — now records directly
  with the same ID, mirroring interactive standalone).
- CLI direct `exec` remains marker-based (`UNJOURNALED_LOCAL_EXECUTION`,
  intentional, documented); MCP history no longer hides that marker when
  durable entries exist (CLI `--machine` parity, proven).
- History status labels come from the single `RuntimeStatus::history_label`
  authority on every recording surface.

## Authority / Tethers (E2.4, repaired): BLOCKED_BY_TETHERS

- Full assessment: `TETHERS_SEAM_ASSESSMENT.md` (this directory).
- Omen ships NO authority/admission/scope/evidence types: the repair
  removes the speculative Omen-owned ontology. The blocked seam is
  evidence, not an invitation to design Tethers inside Omen; no
  permission/policy/approval/grant/token logic exists anywhere
  (absence verified by scan).
- File-supplied `ExecutionContract` mechanics unchanged (no silent
  substitute, no behavior change).

## Cross-surface contract

For one execution identity, CLI (`history --machine`, `cancel --machine`),
MCP (`omen_history_query`, `omen_cancel_execution`,
`omen_execution_status`), IPC, and Python project the same durable truth
from the same SQLite state through the same read path. Interactive
projects through the same database. Proven by parity tests
(`history_parity`, MCP surface tests, CLI live cancel test).

## Machine contract

- Contract version stays `0.8` (additive only). Digest changes (new
  `execution.cancel` capability): Python treats digest drift as
  diagnostic, not incompatible (proven by the hosted SDK matrix).
- New wire: `RuntimeStatus::OutcomeUnknown`, `HistoryStatus::Cancelled`,
  `ExecutionStatusCode::{CancellationRequested, Cancelled}`,
  IPC `CancelExecution`/`CancelResult` + `CancelOutcome`, MCP
  `omen_cancel_execution`, CLI `cancel`.

## Known limitations (explicit)

1. Durability covers process crash, not OS crash/power loss in the
   checkpoint window (`synchronous=NORMAL`).
2. Cancel observation waits up to 60s (`CANCEL_OBSERVE_TIMEOUT`) before
   reporting `OutcomeUnknown`; termination grace is 5s.
3. Standalone (non-broker) executions cannot be cancelled post-hoc (no
   live handle tracking); MCP/CLI refuse explicitly.
4. Interactive shell issues no cancels; authority seam is
   `BLOCKED_BY_TETHERS`; no JavaScript SDK / Nushell / IDO-1 / grammar
   work (per non-goals).
5. Python `cancel_execution` returns the raw record (no typed
  `CancelOutcome` model — deliberate opaque passthrough).
