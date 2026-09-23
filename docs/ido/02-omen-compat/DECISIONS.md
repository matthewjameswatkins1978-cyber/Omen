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
