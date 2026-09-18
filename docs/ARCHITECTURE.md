# Omen Architecture

## Overview

Omen is an agent-native developer runtime designed as a substrate for intelligent systems.
Its architecture is divided into clean, decoupled layers:

1. **Domain Primitives (`omen-core`)**: Strongly-typed resource identifiers, URI parsers, assurance enumerations, and canonical error codes. Independent of I/O and async runtimes.
2. **Wire Specification (`omen-schema`)**: Strict JSON Schemas for execution contracts and results, enforcing closed models (`deny_unknown_fields`) and strict semantic versions (`omen.execution/0.2`, `omen.result/0.2`).
3. **Execution Engine (`omen-engine`)**: Argv-only process execution, closed stdin, asynchronous stream capture, timeout supervisors, and platform-specific containment backends (Windows Job Objects, Linux Landlock/pidfd/cgroups, macOS portable).
4. **Knowledge System (`omen-knowledge`)**: SQLite-backed Fact Registry, resource generation counters, lazy pessimism dirty propagation, and Content-Addressed Storage (CAS) for execution evidence artifacts.
5. **Tool Atlas (`omen-atlas`)**: Structured tool catalog, TOML runtime profiles, fingerprinting, and a controlled probing harness.
6. **Tool Adapters (`omen-adapters`)**: High-fidelity integrations for Tier-1 developer tools: ThreadMoth, Cargo, Git, and ripgrep.
7. **Client Interfaces (`omen-cli`)**: Thin CLI interface for humans and automation.
