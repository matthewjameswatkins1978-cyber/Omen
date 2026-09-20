# Omen 0.8 — Machine Contract checkpoint

## Scope

This checkpoint adds the static machine contract and its public bootstrap
surfaces. Static contract truth is physically separate from live workspace and
provider state. `MachineContract` contains only capability and recipe
definitions. `MachineContext` contains the existing `fs:workspace` generation
and runtime status overlay; discovery projects the two together.

## Public surfaces

```text
omen orient --machine
omen orient --machine --since sha256:<digest>
omen capabilities [group] --machine
omen describe <capability> --machine
omen how <recipe> --machine
omen context --since <generation> --machine
```

`orient` is bounded and contains the contract version, Omen version, stable
contract digest, context generation, workspace-relative identity, platform,
backend, capability groups, references, recipe names, and next discovery
operations. It does not enumerate every schema.

Capability entries have separate `definition` and `status` objects. Definitions
describe inputs, outputs, effects, bounds, timeout, idempotency, reversibility,
network behaviour, authority requirements, and bounded examples. Status reports
typed `availability` and `admission` independently. Until a real provider or
authority surface supplies those facts, discovery reports `unknown` rather than
inventing availability or permission.

The contract also exposes the inert `composition.plan` capability. It validates
and projects a named `Omen.toml` action; it does not execute the action or grant
admission.

Schemas use JSON Schema 2020-12. Built-in recipes are advisory references to
current capability IDs and do not grant permission or execute during discovery.

## Evidence

- `cargo check -p omen-cli` passed.
- `cargo fmt --check` passed.
- Workspace Clippy passed with `--all-targets --all-features -D warnings`.
- Public smoke calls for orient, semantic capability filtering, describe,
  recipe discovery, and unavailable historical context delta passed on Windows.
- Contract digest observed as stable across repeated orientation calls:
  `sha256:e3721dffac0f0a3c2350aa1386ccb6d8e49b5935d8001c4f8dcfa88e15a434d4`.
- Runtime status changes are covered by a static digest proof and do not alter
  that digest; changing a static definition does alter it.
- An unknown digest returns `changed: true`, `delta_available: false`, the
  current `contract_digest`, and `next_actions: ["orient"]`; it does not emit
  invented empty change lists.
- Context generation is read from the existing FactRegistry generation named
  `fs:workspace`. A newly opened canonical database reports the real initial
  generation `0` with `generation_status: "known"`; no second counter exists.

## Closeout status

The contract is complete for the 0.8 discoverability boundary. MCP and
interactive projections are read-only views over the same canonical contract
and action planner. Composition execution remains separately gated by current
external execution contracts.

ThreadMoth composition execution is intentionally deferred:

`THREADMOTH_COMPOSITION = BLOCKED_BY_LIVE_TETHERS_AUTHORITY_BOUNDARY`

No Omen-owned permission, admission, approval, or grant representation is
introduced to bridge that gap.
