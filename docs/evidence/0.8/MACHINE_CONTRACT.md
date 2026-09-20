# Omen 0.8 — Machine Contract checkpoint

## Scope

This checkpoint adds the static machine contract and its public bootstrap
surfaces. Static contract truth is intentionally separate from live workspace
and provider state.

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
`available` and `admitted` independently. In particular, execution, mutation,
and filesystem write are not admitted merely because Omen can describe them.

Schemas use JSON Schema 2020-12. Built-in recipes are advisory references to
current capability IDs and do not grant permission or execute during discovery.

## Evidence

- `cargo check -p omen-cli` passed.
- `cargo fmt` passed.
- Public smoke calls for orient, semantic capability filtering, describe,
  recipe discovery, and unavailable historical context delta passed on Windows.
- Contract digest observed as stable across repeated orientation calls:
  `sha256:55ef12200b899fd7b1b9722428c39f3ea26c3cf614a4e4eab5199ce269d70cb7`.

This is an implementation checkpoint, not the final 0.8 completion report.
