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

CLI initialization computes state paths without creating them. Existing
generation reads use `Database::open_read_only` with SQLite immutable URI mode;
planning never opens the writable database, migrates, changes journal mode,
or creates WAL/SHM state. When no canonical database exists,
`context_generation` is `null` and `generation_status` is `unavailable`.

Every successful plan contains the static contract digest, context generation,
ordered steps, warnings, `admission_snapshot_only: true`, and a SHA-256 plan
digest over the canonical serialized plan with its digest field blanked.
Planning errors are structured and fail closed. No planning path has an
execution fallback.

## Proofs

- `cargo test -p omen-core` passed, including deterministic digest and unknown
  capability/type refusal proofs, compatible prior-step routing, forward,
  self-reference, missing-source, missing-field, incompatible-type, unknown
  schema, identifier and bound refusal proofs.
- `cargo test -p omen-cli --test composition_planning_proofs` passed with
  optional-config, strict-field, authority-looking-field, unsupported-version,
  oversized-file, unknown-capability, wrong-type, symlink-escape, literal
  shell-looking-value and real black-box inertness proofs.
- `listing_showing_and_planning_do_not_create_any_omen_state` invokes the real
  `omen` binary for list/show/plan and proves process spawns = 0, network calls
  = 0, model calls = 0, workspace mutations = 0 and Omen-state mutations = 0
  by checking the isolated state home, database/WAL/SHM/CAS paths and the
  workspace directory before and after.
- `planning_reads_existing_generation_without_writing_state` creates canonical
  state through the normal writable path, then proves generation observation
  without database-byte, directory-entry, WAL/SHM, fact or generation changes.
- `omen_knowledge::db::tests::immutable_read_only_open_uses_existing_database`
  proves the owned database reader observes existing rows without creating
  SQLite sidecars.

This is a planning checkpoint. Execution, loops, conditionals, interpolation,
policy and authority remain outside the scope.
