# IDO No. 2 — Omen Compat

Status: **M0 portable measurement machinery in progress (M0-A/B/C tranche);
not M0-complete; V1 foundation incomplete**.

Omen Compat is Omen's bounded compatibility laboratory. It is kept in the
Omen repository so each Omen revision carries the compatible corpus and
evidence policy intended for that revision, while remaining a distinct
subsystem with a one-way observation boundary.

## Canonical filing

```text
crates/omen-compat/             portable harness (M0 core/runner/invariants)
compat/scenarios/               declarative cases
compat/golden/                  approved real-tool targets
compat/invariants/              behavioural truth definitions (see portable.md)
compat/fixtures/                small probe programs and inputs
docs/ido/02-omen-compat/       design, history, decisions, roadmap
CI artifacts                    large traces and generated diagnostics
```

`crates/omen-compat/` is implementation space. `compat/` is corpus space.
`docs/ido/02-omen-compat/` is design/history space. None of these is a
production runtime dependency or a replacement for Tethers, Resolve, Lantern,
or Omen's existing authorities.

## Non-negotiable guardrails

1. Production Omen crates must not depend on `omen-compat`.
2. Compat observes Omen through explicit test boundaries; it does not redefine
   Omen semantics or grant authority.
3. Do not add app-specific runtime hacks or compatibility shims to make a
   target pass. Fix Omen's general contract, record a reference difference,
   or stop for review.
4. Keep committed cases minimal and deterministic. Large traces, recordings,
   matrices, and dumps are CI artifacts.
5. Application-facing harness expansion beyond the authorized M0 portable
   tranche still requires an explicit IDO decision and matching evidence plan.
   PTY/ConPTY, signal/job-control probes, golden apps, and automatic repair
   remain out of scope until separately approved.

## V1 and deferred scope

V1 is the bounded foundation: explicit invariants, small fixtures, declarative
scenarios, structured results, deterministic evidence, and a conservative
failure classification. Milestone 0 establishes runtime ground truth before
real applications are used as witnesses.

Deferred: automatic repair, arbitrary-tool learning, unrestricted fuzzing,
production shims, broad golden-target expansion, and any claim of universal
application compatibility. These require separate approval and evidence.

See [Architecture](ARCHITECTURE.md), [Decisions](DECISIONS.md), and
[Roadmap](ROADMAP.md).
