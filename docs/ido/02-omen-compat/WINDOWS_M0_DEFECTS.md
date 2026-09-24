# Windows M0 defect ledger (IDO No. 2 M0-W)

Canonical location for Windows ground-truth defects discovered by Compat.
Each entry records evidence only. `repair status = NOT ATTEMPTED` on this
branch — production Omen is never repaired in the measurement tranche.

| ID | Invariant | Platform | Omen SHA | Scenario | Evidence grade | Status | Likely subsystem | Repair |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| WIN-M0-001 | `WINDOWS_ENGINE_SHUTDOWN_BOUNDED` / product handle lifecycle | windows-x86_64 | 66af83e (worktree start) | E6/E7: isolated driver invokes `NativePtyHandle::terminate()` then exits | STRONG (observed `STATUS_HEAP_CORRUPTION` 0xC0000374 in driver and in-process test host after product `terminate()` / `WinConPty` Drop) | FAIL / OPEN_DEFECT — product `terminate()` closes `hpcon`/`process_handle` then `WinConPty::Drop` closes again | `omen-engine` `NativePtyHandle::terminate` + `WinConPty` Drop (double-close of HPCON/process handle) | NOT ATTEMPTED |
| WIN-M0-002 | `WINDOWS_INTERACTIVE_CHILD_CONSOLE_HANDLES_VALID` | windows-x86_64 | 66af83e (worktree start) | B: Omen under Compat ConPTY `--interactive gremlin --windows-report` | PARTIAL | OPEN_DEFECT — fixture `windows-report` not observed within bound (interactive handoff did not surface child report under outer ConPTY) | `omen-interactive` `ChildHandoff` (likely) — evidence does not prove cause beyond missing report | NOT ATTEMPTED |
| WIN-M0-003 | `WINDOWS_OUTER_CONPTY_RESIZE_VISIBLE_TO_INTERACTIVE_CHILD` | windows-x86_64 | 66af83e (worktree start) | F: outer resize while interactive `--windows-resize-report` active | PARTIAL | OPEN_DEFECT — resize fixture did not reach `OMEN_COMPAT_RESIZE_WAIT` barrier | interactive handoff / fixture start path | NOT ATTEMPTED |

## Harness results (not product defects)

- Tier WINDOWS CONTROL: Compat-owned ConPTY calibration (create, handles,
  Ctrl-C receipt, resize, mode dirty, raw exit, Job Object, bounded shutdown).
- Tier WINDOWS OMEN INTERACTIVE: product outcomes recorded after control green.
- Tier WINDOWS ENGINE CONPTY: product `NativePtyHandle` outcomes recorded
  separately from interactive handoff (D2-020).

## How to add a defect

1. Make a Windows invariant FAIL or OPEN_DEFECT with reproducible evidence.
2. Add a row above with: ID, invariant, platform, Omen SHA, scenario,
   evidence grade, minimal reproducer path, status, likely subsystem only if
   evidence supports it, `repair = NOT ATTEMPTED`.
3. Do not diagnose beyond evidence.
4. Do not modify runtime on this branch.
5. One primary defect must not cascade into fake independent FAILs (POSIX
   lesson): dependent targeted-control claims become INCONCLUSIVE when the
   topology never established a targetable group.

## Hypotheses status (M0-W run)

| ID | Hypothesis | Status |
| --- | --- | --- |
| H-WIN-001 | Interactive ChildHandoff may not create a distinct Windows console control group | **INCONCLUSIVE** — no behavioral targeting probe; `CREATE_NEW_PROCESS_GROUP` not asserted from configuration alone → `WINDOWS_INTERACTIVE_CHILD_HAS_TARGETABLE_CONTROL_GROUP` INCONCLUSIVE |
| H-WIN-002 | Ctrl-C may affect a shared shell/child console topology rather than a separately targetable child group | **INCONCLUSIVE** — interactive Ctrl-observe fixture did not announce ARMED under outer ConPTY (related to WIN-M0-002); shell survival PASS independently |
| H-WIN-003 | Engine ConPTY exit reporting may preserve only a raw exit-code bit pattern without semantic cause identity | **REPRODUCED as design** — `WINDOWS_ENGINE_EXIT_STATUS_PRESERVES_RAW_BITS` PASS STRONG for `0x80000007`; cause context remains `Unknown` (D2-021: bits ≠ cause) |
| H-WIN-004 | Engine ConPTY terminate/Drop handle lifecycle may have shutdown edge cases | **REPRODUCED** → **WIN-M0-001** (STATUS_HEAP_CORRUPTION after product `terminate()` double-close) |
| H-WIN-005 | Blocking ConPTY reader/writer workers may create cleanup or boundedness questions | **NOT OBSERVED as hang** — drop-path create/close cycles bounded (`WINDOWS_ENGINE_SHUTDOWN_BOUNDED` PASS); terminate path blocked by WIN-M0-001 before worker shutdown can be judged cleanly |
| H-WIN-006 | Console mode restoration after a hostile interactive child is unknown | **INCONCLUSIVE** — dirty mutation not verified (interactive child report missing; see WIN-M0-002) → restoration blocked |
| H-WIN-007 | Engine Job Object containment may already work correctly | **PARTIALLY SUPPORTED** on drop-path / isolated observation where both PIDs seen; terminate-path containment **INCONCLUSIVE** until WIN-M0-001 fixed (driver aborts before after-state JSON); Compat control Job Object calibration **PASS** |
