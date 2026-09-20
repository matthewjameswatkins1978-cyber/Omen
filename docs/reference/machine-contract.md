# Machine Contract Reference

Status: Omen 0.8 development surface.

The Machine Contract is Omen's versioned self-description for machine clients.

## Static contract truth

Static definitions include capability IDs, groups, summaries, input/output schemas, effects, idempotency, reversibility, network characteristics, authority requirements, bounds, timeout semantics, examples and recipes.

`execution.run` is potentially network-capable and consequential because it
starts an arbitrary process. `composition.run` is likewise consequential and
requires a current externally supplied execution contract; composition cannot
contain `composition.plan` or `composition.run` child steps.

## Live status overlay

Runtime status is separate and may include availability, admission, provider status, assurance, degradation reason, context generation, workspace and backend state.

Changing live status must not redefine the static contract.

## Discovery

    omen --machine orient
    omen --machine capabilities
    omen --machine capabilities <group>
    omen --machine describe <capability>
    omen --machine how <recipe>
    omen --machine context --since <generation>

## Contract digest

The digest fingerprints static semantics.

A different digest does not justify an invented change list. Omen must possess both known contracts before claiming a precise delta.

## Schemas

Capability input/output schemas use JSON Schema 2020-12.
