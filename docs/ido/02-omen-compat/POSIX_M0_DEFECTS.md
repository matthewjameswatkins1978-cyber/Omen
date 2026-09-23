# POSIX M0 defect ledger (IDO No. 2 M0-D/G)

Canonical location for POSIX ground-truth defects discovered by Compat.
Each entry records evidence only. `repair status = NOT ATTEMPTED` on this
branch — production Omen is never repaired in the measurement tranche.

Omen SHA for entries below: `ce7593d2c5b0169207c5eb01d4f8b1c21ac1db24`
(base foundation; product binary built from this worktree HEAD during
M0-D/G development).

| ID | Invariant | Platform | Omen SHA | Scenario | Evidence grade | Status | Likely subsystem | Repair |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| POSIX-M0-001 | `SHELL_JOB_HAS_DISTINCT_PROCESS_GROUP` | linux-x86_64 | ce7593d (worktree) | A: `--interactive … --posix-report` under calibrated PTY | STRONG | FAIL | `omen-interactive` `ChildHandoff::spawn_interactive` (inherit stdio; no observed `setpgid`/job pgrp split) | NOT ATTEMPTED |
| POSIX-M0-002 | `TERMINAL_SIGINT_TARGETS_FOREGROUND_JOB` | linux-x86_64 | ce7593d (worktree) | E: terminal VINTR while interactive fixture READY | PARTIAL | FAIL | Same missing job pgrp: fg pgrp == shell pgrp when Ctrl-C injected | NOT ATTEMPTED |
| POSIX-M0-003 | `EXIT_STATUS_PRESERVES_SIGNAL_NUMBER` | linux-x86_64 | ce7593d (worktree) | F: foreground SIGINT via interactive handoff | UNAVAILABLE | UNAVAILABLE | Interactive path does not surface grandchild wait-signal identity to Compat without production instrumentation | NOT ATTEMPTED |

## Harness results (not product defects)

- Tier POSIX CONTROL: 7/7 PASS (calibration).
- Tier POSIX OMEN: 9/9 harness-green; product outcomes recorded above
  and in test reports. `SHELL_JOB_SHARES_SHELL_SESSION`,
  `TERMINAL_FOREGROUND_PGRP_IS_JOB`, `SHELL_REGAINS_TTY_AFTER_JOB_*`,
  `WAIT_OBSERVES_STOPPED_STATE`, `SHELL_SURVIVES_FOREGROUND_JOB_SIGINT`,
  `CHILD_SIGNAL_MASK_UNBLOCKED_BEFORE_EXEC`,
  `SIGWINCH_ASYNC_DELIVERED_TO_FOREGROUND_PGRP_ON_RESIZE`,
  `SHELL_TERMIOS_SNAPSHOT_RESTORED_AFTER_ABNORMAL_CHILD_EXIT`,
  `NO_ZOMBIE_CHILDREN_OF_SHELL` observed PASS on Linux WSL for this SHA.

## How to add a defect

1. Make the invariant FAIL or OPEN_DEFECT with reproducible evidence.
2. Add a row above with: ID, invariant, platform, Omen SHA, scenario,
   evidence grade, minimal reproducer path, status, likely subsystem
   (only if justified), confidence, `repair = NOT ATTEMPTED`.
3. Do not diagnose beyond evidence.
4. Do not modify runtime on this branch.

## Hypotheses status (M0-D/G run)

| ID | Hypothesis | Status |
| --- | --- | --- |
| H-POSIX-001 | foreground pgrp handoff missing on interactive path | **REPRODUCED** → POSIX-M0-001 |
| H-POSIX-002 | stopped-child observation / prompt reacquisition | **DISPROVED** for this SHA on Linux (WAIT_OBSERVES_STOPPED + SHELL_REGAINS_TTY_AFTER_JOB_STOP PASS) |
| H-POSIX-003 | signal-faithful exit identity flattened | **PARTIALLY REPRODUCED** → POSIX-M0-003 (UNAVAILABLE surface) |
| H-POSIX-004 | termios not restored after abnormal child exit | **DISPROVED** for this SHA (selected snapshot restored) |
| H-POSIX-005 | SIGWINCH not delivered to distinct job pgrp | **INCONCLUSIVE as distinct-pgrp claim**; async SIGWINCH to foreground **observed PASS** while job remains in shell pgrp (see POSIX-M0-001) |
