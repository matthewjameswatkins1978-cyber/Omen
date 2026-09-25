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

## 3. Standing AI-First Engineering Rules

Runtime-facing fixes require Preview Conveyor installation and proof before
external acceptance. Do not hand-roll release, install, or proof PowerShell;
use `cargo xtask preview` unless debugging the conveyor itself.

These rules apply to all future Omen development unless a component explicitly documents why an exception is necessary.

### 1. Nothing May Wait Forever
Any operation involving an external boundary or asynchronous coordination must be bounded.
This includes:
- subprocesses, LSP servers, network/provider calls, IPC, channels, task joins, locks, filesystem watches, plugin/tool calls, model calls, background workers, initialization, shutdown.
Every such operation must have:
- a defined deadline or cancellation path
- a useful failure mode
- cleanup behaviour
- enough diagnostics to identify where it stopped
Tests must additionally have an outer watchdog. A broken Omen component should fail in seconds with evidence. It must never silently become immortal. Production timeout protects Omen. Test timeout protects the development process. Both are required.

### 2. Failure Must Return Control
Failure is not allowed to strand the caller.
After a timeout, malformed response, crashed provider, dead subprocess, invalid tool result, or interrupted operation:
- control must return to the caller
- resources must be released
- subsequent operations must remain usable
- corrupted or ambiguous state must not be silently retained
"Error returned but subsystem remains poisoned" is still a failure.

### 3. Failures Must Name the Phase
Errors should identify where the operation failed. Prefer `semantic.lsp.definition.request.timeout` over `operation failed`.
Useful phase boundaries include:
- `discover`, `resolve`, `spawn`, `initialize`, `request`, `receive`, `validate`, `commit`, `shutdown`, `join`.
An AI should not need to reconstruct the execution path from prose and stack traces when Omen already knows it.

### 4. Machine-Readable Truth Before Human-Friendly Presentation
Where Omen knows structured facts, preserve them structurally. Human text is a presentation layer, not canonical state.
Commands and subsystems must expose stable structured representations for:
- status, errors, capabilities, paths, symbols, actions, provenance, timings, resource usage, diagnostics.
An agent should not have to scrape terminal prose to discover something Omen already knows.

### 5. Do Not Make the Model Rediscover Known Facts
If Omen can determine something deterministically, do so.
Examples: current working directory, repository root, Git branch/state, workspace packages, symbol definitions, file identity, language/runtime metadata, tool availability, environment facts, known capabilities.
Do not spend inference on facts available from deterministic systems. Model reasoning is for ambiguity, judgement, synthesis, and interpretation — not a substitute for `stat()`, Git, ASTs, LSP, SCIP, or Cargo metadata.

### 6. Zero-Model Paths Must Really Be Zero-Model
When a task is classified as deterministic, no hidden fallback may silently invoke a model.
This includes: retries, error handling, completion helpers, semantic fallback, formatting, recovery paths.
If a deterministic path cannot complete deterministically, return a bounded explicit result describing what is missing. Do not quietly convert deterministic work into probabilistic work.

### 7. One Semantic Authority Per Fact
A consequential fact should have one canonical owner.
Avoid multiple parts of Omen independently deciding: repository root, language, workspace membership, active project, symbol identity, capability, action result, execution state.
Other components may cache or project that fact, but they must not invent competing definitions.

### 8. Make State Transitions Explicit
Important work should move through named states rather than vague booleans or implicit behaviour (e.g. `discovered`, `admitted`, `resolving`, `executing`, `completed`, `failed`, `cancelled`, `timed_out`). Agents reason better about explicit state machines than scattered flags. Illegal transitions should fail visibly.

### 9. Partial Success Must Be Representable
Do not collapse "8 things succeeded, 1 failed" into `failed` or `success`.
Represent partial completion explicitly:
- completed work, failed work, unresolved work, retryability, evidence.
This lets an agent continue intelligently instead of repeating an entire operation.

### 10. Operations Should Be Resumable Where Practical
Long or multi-stage operations should leave enough durable evidence to continue after interruption. Avoid requiring an agent to restart expensive work simply because a process died, context was lost, a tool call timed out, or the harness restarted. Where possible, expose checkpoints, artifacts, receipts, or stable intermediate results.

### 11. Retries Must Be Intentional
Never hide infinite or uncontrolled retry loops. Every retry policy must define:
- what is retryable, maximum attempts, backoff where relevant, whether the operation is idempotent, what evidence is retained, final failure behaviour.
A retry must not accidentally duplicate a mutation.

### 12. Mutations Should Be Idempotent or Identified
Where practical, repeated execution of the same intended action should either produce the same state or be recognised as the same action through a stable operation/action ID. Agents retry things; the runtime must assume this.

### 13. Reads and Writes Must Be Distinguishable
Omen should make consequential mutations obvious. A tool/action schema should clearly communicate whether it reads, computes, mutates local state, mutates external state, starts a process, or performs network IO. Do not make an agent infer side effects from command names.

### 14. Capabilities Should Be Narrow and Composable
Prefer small explicit capabilities over broad ambient authority (e.g. `fs.read`, `fs.write`, `process.spawn`, `network.request`, `git.read`, `git.mutate` rather than `system.access`).

### 15. Context Is a Resource
Do not dump information simply because it is available. Omen should return the smallest complete representation required for the task.
Prefer summaries plus references, structured metadata, stable IDs, lazy detail retrieval, pagination, and bounded output. Token consumption is an architectural cost.

### 16. Large Results Should Become References
When output becomes large, store it and return a stable reference plus concise metadata (`artifact://` URIs). Do not force massive outputs through the conversational surface.

### 17. Provenance Should Travel With Important Facts
Where useful, a result should say how it was obtained (`cargo_metadata`, `lsp`, `scip_index`, `filesystem`, `git`, `model:gpt-...`, `user`). Especially avoid presenting inferred/model-derived information as though it were deterministic system truth.

### 18. Uncertainty Must Not Be Disguised as State
Omen should distinguish: `known`, `inferred`, `unknown`, `unavailable`, `stale`, `conflicting`. Do not turn "I think this is the workspace root" into `workspace_root = ...` without preserving the uncertainty.

### 19. Stale Data Must Be Identifiable
Caches, semantic indexes, and derived state need freshness semantics. If a source file changed after an index/result was produced, Omen should know that the result may be stale. Prefer explicit invalidation/versioning over hopeful cache reuse.

### 20. Observability Without Log Soup
Important operations should expose concise structured telemetry (`operation_id`, `phase`, `duration_ms`, `source`, `cache_hit`, `provider_calls`, `model_calls`, `bytes_read`, `bytes_written`, `exit_status`). Do not generate enormous diagnostic sludge by default; detail should be available on demand.

### 21. Performance Regressions Are Semantic Regressions When They Hurt Agents
For agent-facing paths, excessive latency, model calls, provider calls, context growth, or unnecessary IO can constitute correctness failures. Tests may assert properties such as `model_calls == 0`, `provider_calls == 0`, `elapsed < bound`, `output_bytes < bound`.

### 22. Test Invariants, Not Implementation Accidents
Tests should prove externally meaningful behaviour (e.g. "late LSP response cannot poison subsequent requests"). Refactoring should not destroy the proof suite merely because internal structure changed.

### 23. Tests Must Themselves Be Engineered Systems
No test may rely indefinitely on sleeps, uncontrolled child processes, ambient services, race timing, shell-specific behaviour, or inherited machine state. Integration tests must own their resources, bound their waits, clean up after themselves, produce diagnostic evidence, and behave deterministically. A hanging test is a bug in the test system.

### 24. Cross-Platform Is the Default
Unless a feature is explicitly declared platform-specific, assume Windows and Linux at minimum. Do not hard-code shell syntax, drive letters, `/tmp`, path separators, process semantics, or executable suffixes.

### 25. The Harness Must Not Become the Product
Omen exists to reduce friction between intent and execution. Do not add layers merely to make the architecture appear sophisticated. Every new abstraction should earn its existence by improving determinism, safety, agent reasoning, portability, observability, performance, maintainability, or resumability.

### 26. Optimise for Recovery, Not Perfection
Assume tools fail, providers disappear, processes crash, context gets truncated, agents retry, users interrupt work, and machines reboot. Omen should make these events boring. A resilient system is one where failure destroys as little work and information as possible.

### 27. The Agent Should Never Have to Guess What Omen Knows
If Omen knows what happened, what changed, what remains, what failed, what capability is available, or what action is safe next, expose it explicitly. Do not bury consequential information in logs, prose, or hidden internal state.

### 28. Consequences Must Be Visible Before Execution
Where Omen can determine likely side effects before performing an action, expose them (e.g. reads, writes, spawns, network, model calls). This lets agents and humans reason about actions before committing to them.

### 29. Prefer Boring Determinism Below Intelligent Reasoning
The lower layers of Omen should be deliberately boring. Parsing, routing, state transitions, permissions, mutation, caching, identities, and evidence should be deterministic wherever possible. Intelligence belongs above that foundation. Do not ask an AI to compensate for an ambiguous runtime.

### 30. Every Important Claim Should Be Provable
When we say Omen guarantees something, turn that claim into a proof/test. Architecture claims without executable evidence tend to decay into folklore.

### 31. Licence Compatibility Is an Engineering Constraint, Not a Rewrite Trigger
Omen is reuse-first. Do not reject mature open-source code merely because it uses MPL-2.0, EUPL, or another copyleft licence.

Before introducing such code, review the concrete integration shape and redistribution obligations. Distinguish dependency/linkage, copying/adaptation, vendoring, and external-tool execution. Preserve upstream provenance and required notices.

A licence that needs review is not the same thing as a forbidden licence. Likewise, open source does not mean automatically compatible.

The canonical policy is [docs/LICENSING_AND_REUSE.md](docs/LICENSING_AND_REUSE.md).

Until Omen's own project licence is formally selected, keep `deny.toml` conservative and use narrow, documented exceptions or policy changes for intentionally accepted dependencies rather than broadly weakening the licence gate.

## Verification discipline

During implementation, use focused formatting, checks, and tests appropriate to the changed package. Do not repeatedly run the complete repository gate as a substitute for targeted feedback. At final handoff, run `cargo run -p xtask -- verify` once after the final source change. Do not call a partial check complete verification; if source changes afterward, rerun the canonical gate.
