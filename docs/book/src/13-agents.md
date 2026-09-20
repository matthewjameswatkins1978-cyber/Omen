# 13. Omen for agents

An agent-native shell should not merely expose terminal text through an API.

It should remove unnecessary inference.

## Orientation

An unfamiliar agent should begin with a bounded machine-facing map.

In 0.8 development:

    omen --machine orient
    omen --machine capabilities
    omen --machine capabilities semantic
    omen --machine describe semantic.definition

The initial map remains small.

## Machine mode

Machine mode is intentionally boring.

Its target guarantees include structured stdout/errors, no ANSI, no spinner, no pager, no interactive question, explicit exit status and bounded output.

## Stable references

Typed references reduce rediscovery.

Instead of repeatedly searching for the last failed operation, carry @failed.

Instead of copying a huge transcript, carry artifact://...

Stable identities are context compression.

## Context deltas

A major 0.8 goal is to ask what changed since generation N rather than repeatedly collecting pwd, Git state, process lists and history.

When Omen cannot reconstruct a requested historical delta, the correct answer is DELTA_UNAVAILABLE.

## MCP is an edge

Omen exposes agent operations through MCP, but MCP does not define Omen's internal meaning.

Canonical semantics live inside Omen.

Adapters project them outward.

## Agent Learnability

A long-term acceptance test is simple:

Give a capable agent Omen but no Omen manual.

Ask it to find a symbol, inspect callers, run relevant tests and report what changed.

Measure discovery calls, invalid calls, model tokens, raw shell calls, repeated observations, authority mistakes, time and correctness.

That is stronger than counting AI features.
