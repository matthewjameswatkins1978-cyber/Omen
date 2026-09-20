# Omen 0.8 Final Acceptance Evidence

## Status

**FINAL CANDIDATE — PENDING INDEPENDENT ACCEPTANCE**

Candidate branch: `feature/omen-0.8-closeout`  
Candidate SHA: `21efdda4db2ac657527b5a4e32b2e94ae8516f16`  
Hosted CI: `35515885963` (green for the candidate)

Lucy performs final acceptance and merge. This document records the evidence
without promoting the candidate to accepted status.

## Implemented contract

- Machine Contract exposes progressive `orient`, capability listing,
  capability descriptions, typed runtime status, recipes, and context deltas.
- Contract digest is deterministic and separate from live context generation.
- Context generation comes from the existing `fs:workspace` Fact Registry
  generation; no second counter is introduced.
- Recipes are advisory references and do not grant permission.
- Optional `Omen.toml` defines bounded named actions and typed input routing.
- Planning is read-only, validates against the canonical contract, and emits a
  deterministic plan digest.
- Composition execution is sequential, bounded, fail-closed, and preserves
  `COMPLETED`, `FAILED`, `REFUSED`, `TIMED_OUT`, and `PARTIAL` truthfulness.
- Existing execution-contract and plan-digest gates remain in force.

## Authority boundary

Omen does not calculate Tethers authority. It does not define permission,
approval, admission, grant, or token DTOs. The workspace `.tethers/config.json`
contains only the host project schema, config version, and workspace root.

`THREADMOTH_COMPOSITION = BLOCKED_BY_LIVE_TETHERS_AUTHORITY_BOUNDARY`

ThreadMoth request/certificate machinery remains intact. Generic
`filesystem.write` composition remains unsupported. No mutation trial is
fabricated.

## Public projections

- MCP exposes canonical orientation, capabilities, descriptions, recipes,
  context, and action list/show/plan projections.
- Interactive exposes `:orient`, `:capabilities`, `:describe`, `:how`,
  `:actions`, and `:plan`.
- Neither surface exposes an unsafe composition run command.

## Agent learnability

The deterministic cold-start test passes. The first live-workspace read-only
trial and the first failure trial remain recorded as partial and
invalid/pending respectively; a follow-up to the latter is diagnostic only and
does not convert it into a pass. Independently preflighted replacement
fixtures separated the causes: Omen's semantic provider still returns no
result for a known symbol in the valid fixture, while a fresh agent correctly
diagnosed the intentional Cargo failure fixture without mutation. Full
independent product acceptance therefore remains pending; see
`docs/evidence/0.8/AGENT_LEARNABILITY.md`.

## Verification

Local formatting, Clippy, package suites, MCP/interactive/CLI tests, workspace
all-target tests, schema verification, and `xtask verify` passed for the
candidate. Hosted CI run `35515885963` passed check-and-lint plus Windows,
Ubuntu, and macOS matrix jobs.

## Known limitations and 0.9 handoff

- Live Tethers revocation/admission remains external to this Omen slice.
- ThreadMoth composition execution waits for a suitable public/current Tethers
  host seam.
- Generic filesystem writes remain unsupported.
- 0.9 hardening, protocol freeze, fuzzing, and broader integration work remain
  future scope.
