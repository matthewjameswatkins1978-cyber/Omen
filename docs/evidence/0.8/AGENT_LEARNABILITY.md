# Omen 0.8 Agent Learnability Evidence

The cold-start path is discoverable from the executable and requires no
private architecture prompt:

```text
omen orient --machine
omen capabilities [group] --machine
omen describe <capability> --machine
omen how <recipe> --machine
omen context --since <generation> --machine
omen action list --machine
omen action show <action> --machine
omen action plan <action> --machine
```

An agent can begin with `orient`, follow its `next` operations, inspect a
capability or recipe, then list/show/plan a workspace action. Plans explicitly
report that they are admission snapshots only. The MCP server exposes the same
sequence through `omen_orient`, `omen_capabilities`, `omen_describe`,
`omen_recipe`, `omen_context`, `omen_action_list`, `omen_action_show`, and
`omen_action_plan`.

The learnability boundary is intentionally non-executing. There is no MCP
action-run tool and no interactive `:run`; consequential composition execution
stays behind the existing exact-plan and current execution-contract gates.

ThreadMoth is visible as a canonical capability with Tethers as its authority,
but composition execution reports the truthful deferred state:

`THREADMOTH_COMPOSITION = BLOCKED_BY_LIVE_TETHERS_AUTHORITY_BOUNDARY`.

## Fresh-agent acceptance trials

These are separate fresh Codex task contexts, not the deterministic CLI test.
The trials did not receive the Omen README, Book, `docs/`, command cheat
sheet, architecture description, or an instruction to call `orient`.

### First attempts — preserved history

These first attempts remain part of the evidence and are not replaced by the
replacement-fixture trials below.

#### Trial A — live-workspace read-only symbol investigation

**Prompt:** “Find the definition of refresh_token, show where it is referenced,
run the relevant verification, tell me what changed, and do not modify the
workspace.”

| Field | Observed result |
| --- | --- |
| Model | Fresh Codex task; exact configured model was not exposed to the parent task |
| Task | Read-only definition/reference/verification investigation |
| Discovery calls | MCP initialize, `tools/list`, canonical capability discovery; semantic definition/reference probes |
| Invalid calls | Missing-symbol parameter probe; raw execution with an unavailable program |
| Raw shell calls | Recorded by the trial; no shell bypass occurred |
| Repeated observations | `refresh_token` was not resolved by the live semantic provider |
| Authority errors | No fabricated authority; execution remained separately admitted |
| Model/tool calls | 0 model calls; 18 recorded executable/MCP/shell calls |
| Tokens | Not exposed |
| Elapsed | 432 ms focused verification; 171.3 s total fresh-task context |
| Workspace mutations | None; clean repository before and after; no fixture created |
| Correct | **No — partial discovery pass only** |
| Notes | Omen discovery and verification surfaces were found and used correctly, but the live workspace did not provide the target symbol, so the full task criterion could not pass. |

#### Trial B — live-workspace failure investigation

**Prompt:** “Something failed in this workspace. Find the failure, identify the
relevant evidence and code, and tell me the next justified action without
changing anything.”

| Field | Observed result |
| --- | --- |
| Model | Fresh Codex task; exact configured model was not exposed to the parent task |
| Task | Read-only failure/evidence/code investigation |
| Discovery calls | Omen task was dispatched; executable/MCP discovery was reported as started |
| Invalid calls | The first turn inspected parent-task state instead of the supplied workspace task |
| Raw shell calls | None reported for the actual failure task |
| Repeated observations | The task returned dispatch/pending prose rather than failure evidence |
| Authority errors | None reported |
| Model/tool calls | Not acceptance-valid; task-state MCP calls dominated the first turn |
| Tokens | Not exposed |
| Elapsed | 211.9 s initial context, then 4.2 s follow-up |
| Workspace mutations | None observed |
| Correct | **No — invalid/pending trial, not a pass** |
| Notes | The first turn inspected parent-task state rather than executing the supplied failure task. A follow-up was sent to diagnose that failure, but it returned dispatch/pending prose only. The follow-up is diagnostic evidence and does not turn Trial B into a pass. |

### Replacement fixture preflight

The fixtures were created and verified independently before the replacement
agents received their prompts. The agents were not asked to create fixtures,
were not given Omen documentation or an orient hint, and received only the
task-specific prompt plus the exact fixture path.

| Fixture | Independent preflight | Classification |
| --- | --- | --- |
| `work/acceptance-fixture` | `cargo check` passed. Omen MCP `initialize` passed, but `omen_symbol_definition(refresh_token)` and `omen_symbol_references(refresh_token)` returned `"not_found"`; `omen_symbol_search(refresh_token)` returned `[]`. | Product/provider discoverability failure, not fixture failure. |
| `work/failure-fixture` | `cargo check --manifest-path ... --locked --offline` deterministically exited `101` with no matching package `definitely-not-a-real-omen-dependency`; the source was not reached. | Valid intentional failure fixture. |

### Replacement Trial C — read-only fixture investigation

**Prompt:** “In the workspace at `work/acceptance-fixture`, find the
definition of `refresh_token`, show where it is referenced, run the relevant
verification, tell me what changed, and do not modify any files. Do not create
fixtures. Use only the workspace and its executable/tool surfaces.”

| Field | Observed result |
| --- | --- |
| Model | Fresh Codex task; exact configured model was not exposed to the parent task |
| Task | Read-only definition/reference/verification investigation against preflighted fixture |
| Discovery calls | 0 Omen MCP calls; direct filesystem/source discovery via 6 shell command executions |
| Invalid calls | 2 bounded missteps: non-Git `git status`; compiler metadata output to Windows `NUL` failed before compilation |
| Raw shell calls | 6 command executions; no authority bypass or mutation command |
| Repeated observations | Source definition at `src/lib.rs:5`; in-crate call at `src/lib.rs:10`; Omen MCP preflight remained `not_found`/empty |
| Authority errors | None |
| Model/tool calls | Tool count is observable; model-token count not exposed |
| Tokens | Not exposed |
| Elapsed | 83.7 s |
| Workspace mutations | None; no fixture creation; target fixture unchanged |
| Correct | **Yes for the source investigation; no Omen semantic-provider acceptance** |
| Notes | The agent found the requested evidence using the executable/source surface. `cargo metadata --no-deps --locked` passed. It correctly declined to emit compiler metadata to a temporary file under the no-modification constraint. This does not repair the independent Omen provider preflight failure. |

### Replacement Trial D — failure fixture investigation

**Prompt:** “In the workspace at `work/failure-fixture`, something failed.
Find the failure, identify the relevant evidence and code, and tell me the
next justified action without changing anything. Do not create or modify
fixtures. Use only the workspace and its executable/tool surfaces.”

| Field | Observed result |
| --- | --- |
| Model | Fresh Codex task; exact configured model was not exposed to the parent task |
| Task | Read-only failure/evidence/code investigation against preflighted fixture |
| Discovery calls | 0 Omen MCP calls; direct Cargo/source discovery via 5 shell command executions |
| Invalid calls | None reported |
| Raw shell calls | 5 command executions; no mutation command |
| Repeated observations | `Cargo.toml:7` declares the nonexistent dependency; `cargo check --locked --offline` exits `101`; `src/main.rs:2` is not reached |
| Authority errors | None |
| Model/tool calls | Tool count is observable; model-token count not exposed |
| Tokens | Not exposed |
| Elapsed | 79.1 s |
| Workspace mutations | None; fixture remained unchanged |
| Correct | **Yes — deterministic failure correctly identified** |
| Notes | The justified next action is to preserve the failure if it is the acceptance fixture, or replace the dependency only if compilation is intended. No change was made. |

### Acceptance interpretation

The deterministic learnability proof passes. The first live-workspace attempts
remain recorded as partial and invalid/pending; the follow-up to Trial B is
diagnostic only and cannot convert it into a pass. Independent fixture
preflight then separated the replacement evidence: the read-only fixture is
valid but Omen's semantic provider did not resolve its known symbol, while the
failure fixture reproduced its intended Cargo error. The replacement agents
correctly investigated their respective fixtures without mutation. Full
independent product acceptance therefore remains pending because the semantic
provider/discoverability boundary is unresolved. Mutation learnability is not
trialled because it is explicitly:

`BLOCKED_BY_LIVE_TETHERS_AUTHORITY_BOUNDARY`.
