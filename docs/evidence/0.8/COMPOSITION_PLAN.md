# Omen 0.8 — Composition planning evidence

## Scope

This checkpoint adds the inert, strict `Omen.toml` domain and deterministic
planning for named actions. It does not execute actions, spawn processes,
perform network calls, mutate workspaces, or grant permission.

## Configuration contract

The CLI reads only the optional exact path `<workspace-root>/Omen.toml`.
There is no parent search, include mechanism, environment interpolation or
fallback configuration. The file must be UTF-8 TOML, no larger than 256 KiB,
and must use schema version `1`.

The bounded schema is:

```toml
schema_version = 1

[project]
name = "Planning fixture"

[actions.inspect-auth]
description = "Inspect a definition."

[[actions.inspect-auth.steps]]
id = "definition"
capability = "semantic.definition"
input = { symbol = { kind = "literal", value = "SessionToken" } }
```

Action and step identifiers are lower-case and deterministic. The limits are
128 actions, 64 steps per action, 64 input bindings per step, and 16 KiB per
bounded string. Unknown TOML fields are rejected, including attempted policy
or authority-shaped fields.

## Planning semantics

`action list` reports the optional configuration and named actions. `action
show` reports one validated named action. `action plan` resolves each
capability against the static Machine Contract, checks input fields and exact
JSON types, resolves only prior-step output references, projects current
availability and admission snapshots, and returns effect, network,
reversibility, authority, bounds and timeout metadata.

Every successful plan contains the static contract digest, context generation,
ordered steps, warnings, `admission_snapshot_only: true`, and a SHA-256 plan
digest over the canonical serialized plan with its digest field blanked.
Planning errors are structured and fail closed. No planning path has an
execution fallback.

## Proofs

- `cargo test -p omen-core` passed, including deterministic digest and unknown
  capability/type refusal proofs.
- `cargo test -p omen-cli --test composition_planning_proofs` passed with
  optional-config, strict-field, unknown-capability, wrong-type and literal
  shell-looking-value proofs.
- The CLI proof invokes the real `omen` binary and observes machine JSON and
  non-zero refusal status; it does not mock execution.

This is a planning checkpoint. Execution, loops, conditionals, interpolation,
policy and authority remain outside the scope.
