# Omen Compat architecture boundary

## Dependency direction

```text
compat corpus + omen-compat harness
                 │ observes
                 ▼
           Omen runtime under test
```

The reverse edge is forbidden: production crates must never import
`omen-compat`. Workspace membership is not a runtime dependency.

## M0 portable machinery (implemented)

The crate contains authorized measurement types and a bounded portable
runner (`Platform`, `Capability`, `CommandSpec`, `StdinSpec`, `Deadline`,
`ExitCause`, observations, evidence grades, invariant results, structured
failures, replay descriptors). Platform-specific capabilities remain schema
placeholders until their milestone.

### Bounded process/I/O contract

All external waits use Tokio cancellable I/O with three explicit layers:

1. Scenario deadline (process wait + stdin delivery budget).
2. Cleanup: non-waiting `start_kill()` then bounded root reap within
   `CLEANUP_BOUND` (never `Child::kill().await`, which may wait for exit).
3. **One shared** post-root I/O completion window (`IO_COMPLETION_GRACE`)
   covering stdin, stdout, and stderr task completion and abort
   acknowledgement concurrently — never a fresh full grace per task.

```text
declared_max_wall_ms
  = deadline_ms
  + CLEANUP_BOUND
  + IO_COMPLETION_GRACE
  + scheduling_tolerance
```

The implementation mechanically matches this formula. `BOUNDED_WAIT_NO_HANG`
fails if actual elapsed exceeds that maximum.

**Stream truth:** `truncated` (more bytes than retained), `eof_observed`
(pipe EOF), and `drain_timed_out` (cancelled at grace) are distinct.
Descendant-held pipes may leave `eof_observed=false` with
`drain_timed_out=true`; that is not process failure.
`DRAIN_BOTH_STREAMS_NO_DEADLOCK` means both streams were handled without
deadlock — not that EOF was observed on both.

**Root cleanup vs descendants:** timeout cleanup is `root_only=true` with
distinct `kill_attempted` / `kill_initiated` / `root_reaped` facts. Kill
initiation is not termination. Portable descendant containment is not claimed
until POSIX groups / Windows job-object work.

### Secret-safe durable evidence

Execution may hold env values and stdin bytes transiently. Durable evidence
uses `CommandEvidence` (counts, kinds, key names — no values), explicit
`ReplayDescriptor::{redacted,try_exact_fixture}` with `ReplayFidelity`,
`Observation::EnvApplied { key }` (no value), and `StreamSummary` without
raw previews. Debug for `CommandSpec`/`EnvPolicy`/`StdinSpec` redacts values.

`ReplayFidelity::Exact` is mechanically validated only by
`try_exact_fixture` against original `CommandSpec`:
env must be `Clear`, stdin `Closed`, cwd unset, and safe argv exactly equal
to recorded argv. Omission of any required value yields an error — never a
mislabelled Exact. Evidence projections use `redacted_from_evidence` only
and cannot claim Exact (one authority; see D2-012). The safe generic
projection remains `Redacted`.

**Type seal (D2-013):** `ReplayDescriptor` fields are private; there is no
public fidelity setter or struct-literal forge. Generic `Deserialize`
rejects `Exact` (serialization records a claim; deserialization does not
prove it). Redacted records still round-trip.

Fixture JSON reports remain limited to controlled Compat fixtures.

### POSIX PTY / job-control layer (M0-D/G)

`crates/omen-compat/src/posix/` is `cfg(unix)`-gated measurement code:

- **Harness:** real PTY pair (`openpt`/`grantpt`/`unlockpt`/`ptsname`),
  child establishes `setsid` + `TIOCSCTTY` + `tcsetpgrp` before exec;
  observation slave opened `O_NOCTTY`. Bounded non-blocking master reads
  (`poll` + transcript cap). Cleanup is non-waiting kill + bounded reap.
- **Observations:** `PosixProcessIdentity` (via `observe_shell_identity`
  using `getpgid`/`getsid` + optional Linux `/proc` corroboration — never
  synthesized from pid), `PosixTerminalState` (`is_controlling_terminal`
  derived from observed session match, never hard-coded),
  `PosixWaitState`, `JobStoppedObserved`, `HandoffEvidence`,
  `SigintReceiptObservation`, `SignalMaskEvidence` (explicit availability;
  unavailable ≠ empty), `ParseEvidenceError` — facts only. Fixture identity
  parse failure never becomes a synthetic identity.
- **Judges:** thirteen invariants in `compat/invariants/posix.md`, with
  D2-017 evidence-model constraints (handoff-gated reacquisition, distinct
  fg ownership, observed SIGINT receipt, wait-path-only stopped PASS) and
  D2-018 availability honesty (missing topology / unavailable masks cannot
  become STRONG or PASS).
- **Linux `/proc`:** explicit Linux evidence (`linux_proc`); never claimed
  on non-Linux platforms (UNAVAILABLE instead).
- **Tiers:** POSIX CONTROL calibrates the instrument without Omen; POSIX
  OMEN runs the real shell. Control failures stop product judgment.

Two topologies stay distinct: (A) interactive shell foreground job under
the outer PTY; (B) daemon-created private PTY session. M0-D/G certifies A.

Windows: workspace still builds; POSIX scenarios are `cfg(unix)` or
explicitly UNSUPPORTED — never fake POSIX semantics.

Production Omen is not repaired in this layer (D2-014).

## Evidence layers

- Invariants state what must be true.
- Scenarios state how a bounded case is exercised.
- Fixtures provide the smallest useful probe.
- Golden targets witness selected real tools after the foundation is sound.
- CI artifacts retain large generated output without making Git the trace store.
- Documents record design, decisions, and interpretation; they are not test
  inputs.

Compatibility results must preserve the distinction between observed Omen
behaviour, reference behaviour, known difference, Omen defect, and unresolved
evidence. Compat is diagnostic and evidence-producing, not an authority or an
automatic fixer.
