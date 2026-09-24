# IDO No. 2 decisions

## D2-001 — Same repository, separate subsystem

Omen Compat lives in the Omen monorepo, with implementation at
`crates/omen-compat/`, corpus at `compat/`, and design/history here. Git keeps
the corpus and evidence policy aligned with the Omen revision under test.

## D2-002 — Production isolation

The production runtime never depends on `omen-compat`. Compat is an observing
test subsystem, not a library carried by shipped Omen binaries.

## D2-003 — Evidence stays bounded

Small reproducers and committed scenarios are canonical. Large generated
traces and diagnostic recordings are CI artifacts; confirmed failures are
reduced before any permanent corpus addition.

## D2-004 — Architecture before implementation

The crate began as a scaffold. The authorized M0 portable tranche (core
types, bounded runner, portable invariants, gremlin fixtures) is approved
measurement machinery only. Application adapters, PTY/ConPTY probes, and
automatic repair remain blocked until separately decided.

## D2-005 — Observations before judgments

Fixtures and the runner emit observations (facts). Only the invariant layer
produces PASS/FAIL/… results. A fixture never returns “PASS” about Omen
behaviour.

## D2-006 — Timeout is a failure bound

Deadlines bound failure. They are not sequencing primitives. Readiness uses
explicit flushed fixture tokens when needed, not “sleep and hope.”

## D2-007 — Evidence is graded, never upgraded

`Strong` requires an appropriate independent mechanism (for example OS wait
status for exit codes). Fixture prose alone is `Partial`/`Weak`. Unobservable
platform facts are `Unavailable`.

## D2-008 — Nothing in the harness may wait forever

Process wait, stdin write, stdout/stderr drain, cleanup, and task join are
all bounded. Tokio cancellable I/O is the mechanism. Detached blocked reader
threads are forbidden. `BOUNDED_WAIT_NO_HANG` checks elapsed wall-clock time,
not merely that a status flag exists.

## D2-009 — Durable evidence is secret-safe by construction

Execution input and durable evidence are separate. `CommandSpec`, env values,
stdin bytes, and arbitrary argv are not auto-persisted. Callers explicitly
choose safe replay content and record `ReplayFidelity`. Stream summaries omit
raw previews by default.

## D2-010 — Cleanup initiation is not termination; one shared I/O grace

Timeout cleanup uses non-waiting `start_kill()` then
`timeout(CLEANUP_BOUND, wait())`. `Child::kill().await` is forbidden on that
path because it may wait for process exit before the bound starts.
`CleanupOutcome` keeps `kill_attempted`, `kill_initiated`, `root_reaped`,
and `root_only` distinct — never collapsing kill-request into terminated, or
root-reaped into descendant-tree-gone.

Post-root stdin/stdout/stderr completion and abort acknowledgement share
**one** absolute `IO_COMPLETION_GRACE` window (`tokio::join!` against a single
deadline). Per-task grace clocks are forbidden; `declared_max_wall_ms =
deadline + CLEANUP_BOUND + IO_COMPLETION_GRACE + tolerance` matches the
implementation with no hidden additive waits.

## D2-011 — Exact replay is a factual claim

`ReplayFidelity::Exact` may only be produced by `try_exact_fixture` after
mechanical validation (M0: `EnvPolicy::Clear`, `StdinSpec::Closed`,
`cwd == None`, `safe_argv == argv`). Any omitted execution-affecting value
blocks Exact. No heuristic secret scanning; explicit safe classification
remains the model. The safe generic projection is `Redacted`.

## D2-012 — Exact replay has one authority

`ReplayFidelity::Exact` may only be produced by the mechanically validated
`try_exact_fixture` path against original `CommandSpec` execution state.
Secret-safe `CommandEvidence` projections cannot independently claim Exact:
they never retain original argv, so same-length false argv cannot mint Exact.
Evidence projection is `redacted_from_evidence` only (no caller-supplied
fidelity argument). No second Exact constructor.

## D2-013 — Exact is sealed at the type boundary

`ReplayDescriptor` fields are private: public callers cannot set `fidelity`
or forge a descriptor with a struct literal. Generic deserialization cannot
create Exact — raw bytes are untrusted; persistence is not semantic
authority. Serialization records a claim. Deserialization does not prove it.
`try_exact_fixture` remains the single trusted Exact constructor. Redacted
(and Partial, if ever used) may still round-trip generically. A future
trusted Exact restore path would need explicit revalidation against
execution evidence — out of M0 scope.

## D2-014 — POSIX truth is measurement, not repair

M0-D/G adds a bounded PTY harness, POSIX fixture modes, typed POSIX
observations, and job-control invariants. Production Omen is never modified
in this branch. A failing Omen invariant with reproducible evidence is
successful Compat work: record FAIL / OPEN_DEFECT in the defect ledger,
keep the harness green, and leave `repair = NOT ATTEMPTED`.

## D2-015 — Controlling terminal is established, then observed

Opening a PTY slave is not proof of controlling-terminal ownership. The
harness establishes session (`setsid`), controlling terminal
(`TIOCSCTTY`), and foreground pgrp (`tcsetpgrp`) in the child before exec,
then independently observes via `tcgetpgrp` / `tcgetsid` / Linux `/proc`.
Calibration against a known fixture must pass before judging Omen.

## D2-016 — POSIX waits and PTY reads are bounded hostile I/O

Every PTY poll/read uses an explicit slice and scenario deadline. Every
process wait, stop/continue observation, SIGWINCH wait, and cleanup is
bounded. Sleeps are not sequencing primitives; readiness uses fixture
barriers (`OMEN_COMPAT_READY`, `OMEN_COMPAT_STOPPING`, …) and OS wait
state. A PTY remaining open after process transitions is treated as hostile
I/O, never an unbounded read.

## D2-017 — Dependent claims never exceed observed evidence

Measurement-integrity repair for M0-D/G (IDO No. 2):

1. **No fabricated identity.** Fixture identity is either parsed
   (`pid`/`pgrp`/`sid` present) or missing evidence
   (`ParseEvidenceError`). Shell values are never copied into a job
   identity. Missing/malformed identity yields INCONCLUSIVE / harness
   failure — never STRONG topology PASS/FAIL.
2. **Reacquisition requires proven prior handoff.**
   `SHELL_REGAINS_TTY_AFTER_JOB_{STOP,EXIT}` PASS only with
   `HandoffEvidence`: shell pgrp known, job pgrp known, distinct, and
   terminal fg during the job == job pgrp. When `job_pgrp == shell_pgrp`,
   reacquisition is dependency-blocked / INCONCLUSIVE. Raw
   `fg_after == shell_pgrp` alone is current ownership, not reacquisition.
3. **Distinct fg ownership for `TERMINAL_FOREGROUND_PGRP_IS_JOB`.**
   Raw equality `fg == job == shell` is recorded but does not prove a
   distinct job-control handoff (INCONCLUSIVE).
4. **VINTR ≠ delivery.** SIGINT targeting PASS requires independently
   observed receipt (`OMEN_COMPAT_SIGINT` marker or equivalent), distinct
   job pgrp, fg == job, and shell survival. Injection-only is never
   hard-coded as delivery. Shell survival remains a separate invariant.
5. **`/proc` stopped ≠ Omen wait observation.**
   `JobStoppedObserved { pid, source: procfs }` is a STRONG process fact.
   `WAIT_OBSERVES_STOPPED_STATE` PASS requires a wait-path source
   (`waitpid`/`WUNTRACED`). Control-tier waitpid evidence for Compat's own
   child is never transferred to product Omen judgments.

Do not reward a plausible story; reward an observed fact.

## D2-018 — UNAVAILABLE IS NOT A VALUE

Absence of evidence is not an empty measurement.

1. **Harness construction ≠ observed topology.** Creating a PTY session
   (setsid / TIOCSCTTY / tcsetpgrp) does not grant product judgment
   permission to assume resulting `pgrp`/`sid`. Shell topology is observed
   via `getpgid`/`getsid` (with Linux `/proc` corroboration where present)
   or remains missing (`None`). Failed observation never invents
   `pgrp = pid` / `sid = pid`. There is no
   `harness_session_leader_fallback` (or equivalent) source in product
   evidence. Missing shell topology cannot produce STRONG topology results.
2. **Signal-mask availability is explicit.** Fixture JSON carries
   `signal_masks_available` and `signal_masks_source`. Unavailable masks
   serialize as `blocked/ignored/caught = null`, never `[]`. The judge
   returns UNAVAILABLE when evidence is unavailable; measured-empty is a
   distinct PASS/PARTIAL state. macOS does not fake symmetry with Linux —
   without an allowed primitive the result is UNAVAILABLE.
3. **Controlling-terminal status is observed or unknown.**
   `is_controlling_terminal` is `Some(true)` only when terminal-session
   and process-session observations succeed and match; otherwise `None`.
   Never hard-code `Some(true)` because the harness launched under a PTY.

## D2-019 — WINDOWS CONSOLE EVENTS ARE NOT POSIX SIGNALS

Distinguish terminal/user-like Ctrl-C (ConPTY input `0x03` → `CTRL_C_EVENT`),
programmatic `GenerateConsoleCtrlEvent`, `CTRL_BREAK_EVENT`,
`TerminateProcess`, and `TerminateJobObject`. Do not map Unix `killpg` /
foreground pgrp reasoning onto Windows. Controlled targeting requires
behavioral receipt evidence; configuration flags alone are not targeting
proof.

## D2-020 — CONTROL HARNESS AND PRODUCT CONPTY ARE SEPARATE AUTHORITIES

Compat owns an independent ConPTY control harness (`CreatePseudoConsole` via
windows-sys) that calibrates the mechanism. Omen engine `NativePtyHandle` is
the product under test via a Windows-only **dev-dependency**. A Windows-only
dev-dependency from Compat → engine is acceptable; production must never
depend on omen-compat. Control-harness PASS never becomes product PASS.

## D2-021 — WINDOWS EXIT CODE DOES NOT PROVE CAUSE

Raw DWORD / bit-preservation and semantic cause context remain separate
facts. `WindowsExitObservation` records `raw_status`, `product_code`, and an
explicit `cause_context` (`NormalExit`, `CtrlCObserved`, `ProductTerminateRequested`,
`TimeoutTerminationRequested`, `Unknown`, …). Never infer cause from the
integer alone. Ctrl-C receipt PASS and exit-cause INCONCLUSIVE can coexist.
