# Omen Reference

The Reference is for exact lookup.

If you are learning Omen, begin with:

../book/README.md

## Sections

- CLI: cli.md
- Interactive actions: interactive.md
- Typed references: references.md
- Errors, validity and assurance: errors-and-states.md
- Configuration: configuration.md
- Machine Contract: machine-contract.md
- Platform behavior: platform.md

## Reference rule

This directory must not become a hand-maintained second implementation.

Future reference generation should consume clap definitions, Machine Contract definitions, JSON Schemas, canonical errors, capability/provider registries and the accepted Omen.toml schema.

## Version status

Accepted baseline: Omen 0.8 Composition & Discoverability.

Deferred boundary: ThreadMoth composition execution requires a current public
Tethers authority seam and is not exposed by Omen 0.8.
