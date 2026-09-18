# AGENTS.md — Operational Doctrine for AI Agents

Welcome to Omen.

Omen is an agent-native developer runtime.
It is not primarily a shell and it is not another coding agent.
Its purpose is to make the operating system, developer tools, processes, workspace state, and execution evidence typed, structured, inspectable, enforceable, and economical for AI agents to consume, while remaining useful to humans.

## 1. Architectural Doctrine

> **Omen is substrate, not sovereign.**

The surrounding division of responsibility is absolute:
- **Lantern** knows (durable memory, long-term context, and provenance).
- **Resolve** coordinates (live guard tokens and scope locks).
- **Tethers** controls (permission, trusted capability identity, policy, approval, durable intent, replay, and provider outcome truth).
- **Omen** makes the machine legible and enforceable (physical execution, containment, typed resources, machine-readable facts, and artifact CAS evidence).
- **ThreadMoth** deterministically mutates (bounded structural edits with cryptographic pre/post hashes and refusal).

Compact system statement:
> **Lantern knows. Resolve coordinates. Tethers controls. Omen makes the machine legible and enforceable. ThreadMoth mutates deterministically.**

Do not blur these boundaries. Never create competing policy, replay, approval, or journaling systems inside Omen. Resolve admission cannot grant Tethers permission. Tethers controls authority; Omen reports enforceability.

## 2. Rules for Machine Reasoning

1. **Consequential truth must be explicit**: No heuristic guessing or silent fallbacks.
2. **Never parse human-readable output internally**: Always consume structured JSON interfaces (`--json`).
3. **Closed stdin by default**: Background and automated processes receive EOF immediately. Never allow processes to block on interactive prompts.
4. **Bounded context**: Console and stdout/stderr transcripts are bounded (4–8 KiB). Full unreduced output is stored in the Content-Addressed Storage (CAS) and referenced via `artifact://` URIs. Request bounded slices when needed.
5. **Lazy pessimism for Facts**: When an underlying resource dependency changes, the associated Fact transitions from `CURRENT` to `DIRTY`. Omen **refuses** to silently revalidate or rerun commands when `CURRENT` is required. The caller must explicitly dispatch revalidation.
6. **Strongly typed identities**: All resources, tools, executions, actions, facts, and artifacts have dedicated types (e.g. `ResourceId`, `FactId`, `ArtifactId`), not interchangeable strings.
7. **Platform truthfulness**: If a platform cannot enforce a constraint (such as sandboxing), it reports `OBSERVED` or `UNSUPPORTED`. It never fakes `ENFORCED`.
8. **Cross-platform by default**: Unless explicitly designated as platform-specific (such as Windows Job Objects or Linux Landlock), all ordinary Omen code, grammar parsing, and tests are portable across Windows, Linux, and macOS. Do not hard-code PowerShell, Bash, shell quoting, or Windows drive assumptions into portable code.
