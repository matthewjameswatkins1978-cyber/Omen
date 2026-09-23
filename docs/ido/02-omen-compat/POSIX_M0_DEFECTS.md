# POSIX M0 defect ledger (IDO No. 2 M0-D/G)

Canonical location for POSIX ground-truth defects discovered by Compat.
Each entry records evidence only. `repair status = NOT ATTEMPTED` on this
branch — production Omen is never repaired in the measurement tranche.

Omen SHA for entries below: `ce7593d2c5b0169207c5eb01d4f8b1c21ac1db24`
(base foundation; product binary built from this worktree HEAD during
M0-D/G development).

Measurement-integrity regrade applied at worktree head
`ae452c4888fc77bc5a33e94b822247feef6927db` (D2-017): dependent claims no
longer exceed observed evidence (no fabricated identity; reacquisition needs
prior handoff; VINTR ≠ delivery; `/proc` ≠ Omen wait path).

| ID | Invariant | Platform | Omen SHA | Scenario | Evidence grade | Status | Likely subsystem | Repair |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| POSIX-M0-001 | `SHELL_JOB_HAS_DISTINCT_PROCESS_GROUP` | linux-x86_64 | ce7593d (worktree) | A: `--interactive … --posix-report` under calibrated PTY; fixture identity parsed (not fabricated) | STRONG | FAIL | `omen-interactive` `ChildHandoff::spawn_interactive` (inherit stdio; no observed `setpgid`/job pgrp split) | NOT ATTEMPTED |
| POSIX-M0-002 | `TERMINAL_SIGINT_TARGETS_FOREGROUND_JOB` | linux-x86_64 | ce7593d (worktree) | E: terminal VINTR while interactive fixture READY; delivery observed only when fixture emits receipt | PARTIAL | **INCONCLUSIVE** (dependency-blocked on distinct job pgrp; not a proven routing failure) | Topology blocked by POSIX-M0-001 (shell pgrp == job pgrp); routing to a distinct job never established | NOT ATTEMPTED |
| POSIX-M0-003 | `EXIT_STATUS_PRESERVES_SIGNAL_NUMBER` | linux-x86_64 | ce7593d (worktree) | F: foreground SIGINT via interactive handoff | UNAVAILABLE | UNAVAILABLE (**observability gap**, not counted as a production defect unless the missing wait-signal surface itself is tracked) | Interactive path does not surface grandchild wait-signal identity to Compat without production instrumentation | NOT ATTEMPTED (surface gap only) |

## Harness results (not product defects)

- Tier POSIX CONTROL: 8/8 PASS (calibration; includes SIGINT receipt control).
- Tier POSIX OMEN: harness-green; product outcomes recorded above.
  After integrity repair, the following are **not** claimed as product PASS
  without the required evidence chain:
  - `TERMINAL_FOREGROUND_PGRP_IS_JOB` — INCONCLUSIVE when `fg == job == shell`.
  - `SHELL_REGAINS_TTY_AFTER_JOB_{STOP,EXIT}` — INCONCLUSIVE without proven
    prior distinct handoff.
  - `WAIT_OBSERVES_STOPPED_STATE` — INCONCLUSIVE when only `/proc` shows
    stopped (process fact recorded separately as STRONG `JobStoppedObserved`).
  - `TERMINAL_SIGINT_TARGETS_FOREGROUND_JOB` — INCONCLUSIVE without observed
    SIGINT receipt and/or distinct job pgrp.
- Independent product facts still observed on Linux WSL for this SHA:
  `SHELL_JOB_SHARES_SHELL_SESSION` PASS, `SHELL_JOB_HAS_DISTINCT_PROCESS_GROUP`
  FAIL STRONG, `SHELL_SURVIVES_FOREGROUND_JOB_SIGINT` PASS,
  `CHILD_SIGNAL_MASK_UNBLOCKED_BEFORE_EXEC` PASS,
  `SIGWINCH_ASYNC_DELIVERED_TO_FOREGROUND_PGRP_ON_RESIZE` PASS,
  `SHELL_TERMIOS_SNAPSHOT_RESTORED_AFTER_ABNORMAL_CHILD_EXIT` PASS,
  `NO_ZOMBIE_CHILDREN_OF_SHELL` PASS.

## How to add a defect

1. Make the invariant FAIL or OPEN_DEFECT with reproducible evidence.
2. Add a row above with: ID, invariant, platform, Omen SHA, scenario,
   evidence grade, minimal reproducer path, status, likely subsystem
   (only if justified), confidence, `repair = NOT ATTEMPTED`.
3. Do not diagnose beyond evidence.
4. Do not modify runtime on this branch.
5. Do not promote INCONCLUSIVE / UNAVAILABLE to FAIL without the missing
   evidence (D2-017).

## Hypotheses status (M0-D/G run, regraded)

| ID | Hypothesis | Status |
| --- | --- | --- |
| H-POSIX-001 | foreground pgrp handoff missing on interactive path | **REPRODUCED** → POSIX-M0-001 (STRONG FAIL retained) |
| H-POSIX-002 | stopped-child observation / prompt reacquisition | **INCONCLUSIVE** (not DISPROVED): `fg_after == shell` alone does not prove reacquisition without a prior distinct handoff; process-stopped fact may be STRONG via `/proc` while wait-path remains unproven |
| H-POSIX-003 | signal-faithful exit identity flattened | **PARTIALLY REPRODUCED** → POSIX-M0-003 (UNAVAILABLE surface; observability gap, not auto-counted as product defect) |
| H-POSIX-004 | termios not restored after abnormal child exit | **DISPROVED** for this SHA only to the level observed: selected ICANON/ECHO/ISIG snapshot pre==post after abnormal child exit |
| H-POSIX-005 | SIGWINCH not delivered to distinct job pgrp | **INCONCLUSIVE as distinct-pgrp claim**; async SIGWINCH to foreground **observed** while job remains in shell pgrp (see POSIX-M0-001) |
