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
2. Cleanup bound (root kill/reap after deadline).
3. Stream-drain grace (stdout/stderr EOF after root completion; abort if not proven).

`declared_max_wall_ms = deadline + cleanup + drain grace + scheduling tolerance`.
`BOUNDED_WAIT_NO_HANG` fails if actual elapsed exceeds that maximum.

**Stream truth:** `truncated` (more bytes than retained), `eof_observed`
(pipe EOF), and `drain_timed_out` (cancelled at grace) are distinct.
Descendant-held pipes may leave `eof_observed=false` with
`drain_timed_out=true`; that is not process failure.

**Root cleanup vs descendants:** timeout cleanup is `root_only=true`.
Portable descendant containment is not claimed until POSIX groups / Windows
job-object work.

### Secret-safe durable evidence

Execution may hold env values and stdin bytes transiently. Durable evidence
uses `CommandEvidence` (counts, kinds, key names — no values), explicit
`ReplayDescriptor::{redacted,exact_fixture}` with `ReplayFidelity`,
`Observation::EnvApplied { key }` (no value), and `StreamSummary` without
raw previews. Debug for `CommandSpec`/`EnvPolicy`/`StdinSpec` redacts values.

Fixture JSON reports remain limited to controlled Compat fixtures.

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
