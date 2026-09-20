# Machine Contract Reference

Status: Omen 0.8 development surface.

The Machine Contract is Omen's versioned self-description for machine clients.

## Static contract truth

Static definitions include capability IDs, groups, summaries, input/output schemas, effects, idempotency, reversibility, network characteristics, authority requirements, bounds, timeout semantics, examples and recipes.

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
