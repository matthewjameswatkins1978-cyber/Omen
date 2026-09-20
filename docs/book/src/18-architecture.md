# 18. Architecture and boundaries

The compact mnemonic is:

> Lantern knows. Resolve coordinates. Tethers controls. Omen makes the machine legible and enforceable. ThreadMoth mutates deterministically.

## Lantern

Owns durable memory, long-term context and provenance.

## Resolve

Owns live coordination such as guards, fencing and conflict admission.

## Tethers

Owns permission, policy, approval, trusted capability identity, durable intent and authoritative replay/outcome semantics.

## Omen

Owns physical developer runtime truth: execution mechanics, typed resources, observations and Facts, process/service state, evidence artifacts, semantic developer state, platform assurance and human/machine projections.

> Omen is substrate, not sovereign.

## ThreadMoth

Owns deterministic bounded structural mutation and refusal.

## Internal layers

Important crates include omen-core, omen-schema, omen-engine, omen-knowledge, omen-atlas, omen-semantic, omen-adapters, omen-interactive, omen-agent, omen-mcp, omen-daemon/client/ipc, omen-ui and omen-cli.

## Stable meaning, adaptable edges

External standards will change.

Omen keeps canonical meaning inside and translates at edges:

    Omen canonical semantics
            ↓
    protocol adapter
            ↓
    transport

MCP, CLI, terminal and future projections should not each invent their own meaning.
