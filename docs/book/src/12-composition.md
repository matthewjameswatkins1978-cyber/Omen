# 12. Composition without OmenScript

Status: Omen 0.8 design and implementation work.

Repeated developer work deserves a concise form.

That does not mean Omen needs another general-purpose programming language.

## What composition is for

Composition is for named, bounded, repeated developer operations: known checks, known services, semantic inspection and typed combinations of existing Omen capabilities.

Complex logic remains in real programming languages.

## Omen.toml

The planned Omen.toml is workspace configuration, not policy.

It may describe project identity, named checks, known services, important resources, adapter configuration and named compositions.

It must not replace Cargo.toml, package.json, pyproject.toml or other ecosystem authorities.

It must not grant permission.

Reading it must be inert.

Omen 0.8 accepts an optional strict `Omen.toml` at the selected workspace
root. The bounded schema contains `schema_version = 1`, optional project
identity, and named actions made from ordered capability steps. Each input is
either a typed literal or a reference to an output field from an earlier step.
Unknown fields, unsupported schema versions, unknown capabilities, forward
references and incompatible types are refused during planning.

The public planning surfaces are:

```text
omen --machine action list
omen --machine action show <action-id>
omen --machine action plan <action-id>
```

These commands parse, validate and project a plan only. Execution is an
explicitly separate surface:

```text
omen --machine action run <action-id> \
  --expect-plan sha256:... \
  --execution-contract <step-id>=<contract.json>
```

`run` reloads configuration and context, recomputes the plan, and refuses
before step one if the digest changed. Consequential steps re-check their
current external contract immediately before execution. Configuration still
cannot contain policy, authority, shell, interpolation, loop, conditional or
nested-action semantics.

## Plan before run

A composition should expose ordered steps, stable capability IDs, expected effects, typed routing, availability, current admission requirements and bounds before execution.

If an output type cannot feed the next input type, Omen should refuse before execution.

## Re-admit consequences

A stored action is not permission.

Running a named composition tomorrow must still satisfy tomorrow's authority.

Omen can remember what an operation means.

Tethers decides whether this actor may do it now.

Omen consumes the supplied `ExecutionContractWire` as the current external
execution identity. It does not perform live Tethers revocation checking or
create a competing policy or replay journal.

## Why composition belongs beside discoverability

The Machine Contract explains what Omen can do.

Composition lets those capabilities be combined without forcing an agent back into stringly shell glue.
