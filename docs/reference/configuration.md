# Configuration Reference

## Runtime profiles

Existing Tool Atlas profiles live under:

    profiles/

They are descriptive and do not grant authority.

## Omen.toml

Omen 0.8 planning reads exactly `<workspace-root>/Omen.toml`. It is optional,
strict, bounded, and inert. It is not policy, permission, authority, a script,
or a replacement for ecosystem manifests.

```toml
schema_version = 1

[project]
name = "Omen"

[actions.inspect-auth]
description = "Inspect the SessionToken definition."

[[actions.inspect-auth.steps]]
id = "definition"
capability = "semantic.definition"
input = { symbol = { kind = "literal", value = "SessionToken" } }
```

Each action has a bounded description and ordered steps. Each step has a unique
local `id`, a real Machine Contract capability, and explicit input bindings.
Bindings are exactly `literal` or `step_output`; routing may only target a
prior step's named top-level output field.

There is no shell expansion, environment interpolation, expression syntax,
include/import, conditional, loop, or command substitution. Bounds are 256 KiB
per file, 128 actions, 64 steps per action, 64 input bindings per step, and
16 KiB for bounded strings. Unknown fields and unknown schema versions are
refused. A resolved configuration path outside the selected workspace is
refused.

`omen action list` and `omen action show` inspect configuration only.
`omen action plan` validates and produces a read-only deterministic plan.
`omen action run` is separate from configuration: it requires an explicit
`--expect-plan` digest and repeatable `--execution-contract <step-id>=<path>`
bindings. Authority material is never stored in `Omen.toml`.
