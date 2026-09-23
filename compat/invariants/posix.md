# POSIX job-control / TTY invariants (M0-D/G)

Stable machine-readable IDs. Implementations may use idiomatic Rust names;
serialized identity must match these IDs or clearly preserve their meaning.

| ID | Meaning | Evidence expectation |
| --- | --- | --- |
| `SHELL_JOB_SHARES_SHELL_SESSION` | Foreground interactive job remains in the shell's session where normal shell job-control semantics require it. | Strong: fixture `/proc` or syscall sid == shell sid. |
| `SHELL_JOB_HAS_DISTINCT_PROCESS_GROUP` | Foreground job has a process group distinct from the shell's own pgrp. | Strong: shell pgrp ≠ job pgrp. |
| `TERMINAL_FOREGROUND_PGRP_IS_JOB` | While a **distinct** foreground job owns the terminal, terminal fg pgrp == job pgrp. Raw equality `fg == job == shell` is recorded but is **not** distinct job ownership (INCONCLUSIVE). | Strong: `tcgetpgrp` with `job_pgrp != shell_pgrp`. |
| `SHELL_REGAINS_TTY_AFTER_JOB_STOP` | When the foreground job stops, the shell regains terminal foreground before presenting a prompt. Requires a proven prior distinct handoff (shell/job/fg-during chain). | Strong: proven prior distinct handoff + `tcgetpgrp` after stop == shell pgrp. Without prior handoff: INCONCLUSIVE (dependency blocked). Raw `fg_after == shell` alone is current ownership, not reacquisition. |
| `SHELL_REGAINS_TTY_AFTER_JOB_EXIT` | When the foreground job exits, the shell regains terminal foreground before the next prompt. Same handoff dependency as stop reacquisition. | Strong: proven prior distinct handoff + `tcgetpgrp` after exit == shell pgrp. Without prior handoff: INCONCLUSIVE. |
| `WAIT_OBSERVES_STOPPED_STATE` | Omen's wait/job-control path observed STOPPED (not only process state). | Strong: wait-path source (`waitpid`/`WUNTRACED`) reporting stopped. Linux `/proc` state `T` is a separate `JobStoppedObserved` process fact — never sufficient alone for PASS. |
| `TERMINAL_SIGINT_TARGETS_FOREGROUND_JOB` | Ctrl-C / terminal-generated SIGINT reaches the **distinct** foreground job rather than being mistaken for shell termination. | Strong: job pid/pgrp observed; `job_pgrp != shell_pgrp`; `tcgetpgrp` == job pgrp; **SIGINT delivery independently observed** (receipt marker); shell alive. VINTR injection alone is not delivery. Without delivery: INCONCLUSIVE/UNAVAILABLE. Distinguish from programmatic `kill(pid)`. |
| `SHELL_SURVIVES_FOREGROUND_JOB_SIGINT` | Foreground SIGINT termination does not incorrectly kill Omen itself. | Strong: shell process still alive after VINTR. |
| `EXIT_STATUS_PRESERVES_SIGNAL_NUMBER` | A process terminated by signal retains signal identity (SIGINT ≠ SIGTERM ≠ SIGKILL). | Strong: wait status `WIFSIGNALED` + exact signo. Never collapse to anonymous failure. |
| `CHILD_SIGNAL_MASK_UNBLOCKED_BEFORE_EXEC` | Relevant inherited blocked signal state does not leak into the exec'd foreground job. | Partial: fixture-reported `/proc/self/status` `SigBlk` names on Linux; empty set on platforms without that source. Only assert measured signals. |
| `SIGWINCH_ASYNC_DELIVERED_TO_FOREGROUND_PGRP_ON_RESIZE` | Resize while a foreground job owns the terminal produces both new size and async SIGWINCH delivery. | Strong: `tcgetwinsize` change + fixture `OMEN_COMPAT_SIGWINCH` barrier. |
| `SHELL_TERMIOS_SNAPSHOT_RESTORED_AFTER_ABNORMAL_CHILD_EXIT` | After a foreground child dirties selected terminal modes and exits abnormally, Omen restores the shell's pre-job selected canonical snapshot (ICANON/ECHO/ISIG). | Strong: pre vs post `TermiosSnapshot` equality. Never compare against a hard-coded fantasy. |
| `NO_ZOMBIE_CHILDREN_OF_SHELL` | After the tested lifecycle reaches its documented terminal state, Omen does not leave a reapable zombie direct child. | Strong on Linux (`/proc` children state `Z`); UNAVAILABLE elsewhere when not independently observable. |

## Evidence grades

- Direct kernel/syscall observation: STRONG
- wait status showing stopped/signaled (wait path): STRONG for `WAIT_OBSERVES_STOPPED_STATE`
- Linux `/proc` state `T`: STRONG for process-stopped fact only — never upgrades wait-path claims
- `tcgetpgrp`-style terminal observation: STRONG when paired with distinct pgrp chain
- fixture raw syscall values: STRONG for its local fact when deterministic; corroborate consequential topology where practical
- fixture SIGINT receipt marker after VINTR: STRONG for delivery (routing still needs distinct pgrp + fg)
- VINTR write alone (no receipt): not delivery evidence
- missing/malformed fixture identity: missing evidence — never invent pid/pgrp/sid
- terminal text implying foreground: WEAK / PARTIAL
- reference Bash behaviour: WITNESS ONLY (not authority)

## Tiers

- TIER 0 — pure schema/invariant state
- TIER 1 — portable fixture/process tests
- TIER POSIX CONTROL — PTY harness calibration without Omen
- TIER POSIX OMEN — real Omen shell under PTY

Judgment lives in `omen-compat`. Fixtures report facts only.
