# Omen 0.8 — Composition execution evidence

## Scope

This checkpoint adds `action run` for accepted deterministic plans. Execution
is sequential, bounded, non-interactive, fail-closed, and orchestration over
existing Omen capabilities. It adds no workflow language, policy, approval,
retry, rollback, parallelism, loops, conditions, or nested actions.

## Architecture

`omen-core` owns typed action-run reports, state-change classification, and
conservative effect metadata. `omen-adapters` owns the reusable
`CapabilityExecutor` boundary and `execute_plan`; the CLI only loads,
recomputes, digest-checks, binds authority files, and renders the report.
Physical process execution remains in `ProcessSupervisor`. Semantic execution
uses the shared workspace semantic registry. Omen writes bounded physical
evidence and output artifacts to CAS; it does not create a replay journal.

## Capabilities

Supported: `semantic.definition`, `semantic.references`,
`semantic.diagnostics`, `structure.search`, `filesystem.read`, and
`execution.run`. `mutation.threadmoth`, `filesystem.write`,
`composition.plan`, and `composition.run` are refused as composition child
steps unless a future canonical authority-bound executor is added.

`filesystem.read` is workspace-scoped, symlink-aware, and bounded. Semantic
providers are used directly; there is no model or grep fallback.

## Authority and digest gate

`action run` requires `--expect-plan`. It reloads `Omen.toml`, the current
machine contract and current generation, recomputes the plan, and returns
`PLAN_CHANGED` with both digests before any child step if they differ. Repeatable
`--execution-contract <step-id>=<path>` bindings are parsed as
`ExecutionContractWire`; contracts are not configuration.

At each consequential step, resolved `argv` must exactly equal
`ExecutionContract.intent.args`. Missing, duplicate, unrelated, invalid, stale,
or mismatched bindings refuse before spawn. `network_denied` refuses when the
active backend cannot enforce it. Unsupported lease enforcement and interactive
or inherited composition stdio also refuse; these semantics are not silently
claimed.

## Results and failure semantics

Reports contain the executed plan digest, before/after generation when known,
explicit `COMPLETED`, `FAILED`, `REFUSED`, `TIMED_OUT`, or `PARTIAL` status,
per-step bounded output, process exit including signal, enforcement, artifacts,
duration, and conservative `NO`, `YES`, or `POSSIBLE` state change. A process
that starts is `POSSIBLE` even when its exit is non-zero. Earlier completed work
followed by a later refusal or failure is `PARTIAL`; there is no rollback or
automatic retry.

Large stdout/stderr is stored in CAS and referenced from the report. One
immutable bounded JSON action evidence artifact is written at the end.

## Proof matrix

- Core planning/effect tests: passed, including `execution.run` external
  network semantics and consequential-step aggregation.
- Adapter tests: passed, including fake sequential routing, stop-on-failure
  partial status, runtime output-schema refusal, and no hidden retry.
- CLI black-box tests: passed for plan digest binding, missing authority,
  exact contract mismatch, stale plan refusal, network-denial refusal before
  spawn, real process execution, bounded report, and evidence artifact.
- Direct `exec --contract` now uses canonical result classification and preserves
  process signal/enforcement fields.

## Known limitations

Live Tethers revocation is not implemented in this slice; Omen consumes the
current externally supplied contract. Native network denial is unavailable when
the backend reports only observed network control. ThreadMoth and filesystem
write composition execution remain truthful unsupported capabilities.

`THREADMOTH_COMPOSITION = BLOCKED_BY_LIVE_TETHERS_AUTHORITY_BOUNDARY`.
The current public Tethers host seam does not provide Omen with a
capability-specific, current admission for a structured ThreadMoth mutation.
Omen therefore keeps the existing ThreadMoth request/certificate machinery and
does not calculate, serialize, or manufacture Tethers permission.

MCP now exposes read-only canonical projections for orientation, capabilities,
recipes, context, and action planning. Interactive discovery exposes the same
contract through `:orient`, `:capabilities`, `:describe`, `:how`, `:actions`,
and `:plan`. Neither surface exposes an unsafe composition `:run` path.
