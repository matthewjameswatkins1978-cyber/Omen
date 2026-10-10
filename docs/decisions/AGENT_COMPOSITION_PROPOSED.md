# Agent composition interface — design proposal (not for RC merge)

Status: PROPOSED. Presents the design and compatibility implications for
acceptance first. No code merged under this proposal; the RC keeps
client-side composition via `execution.run` (see `shell.pipeline` summary).

## Problem

Non-interactive agents (CLI `exec`, MCP `omen_execute`) run ONE external
argv. The portable shell's composition (`shell.pipeline`, `shell.builtin`)
is interactive-only. An agent needing `cat server.log | grep ERROR | sort -u`
today runs three execs and splices bytes client-side. That works, but:

- each stage is a separate brokered execution (3x history/CAS overhead);
- byte-exactness depends on the client's splicing (machine exec output is
  JSON-string bounded, not the shell's `Vec<u8>` chain);
- Omen builtins (`cat`, `grep`, `sort`, …) have no machine route at all, so
  agents cannot use them even when they are the right tool.

## Proposal: structured stages on the existing dispatch

No shell-string parsing, no host-shell fallback, no new authority:

- Input is a structured list of stages: `[{argv: [...], env: {...}}, …]`
  plus stdin bytes. A shell *string* is never accepted — this eliminates
  the injection class rather than sanitising it.
- All-builtin chains reuse `omen_builtins::run_pipeline` directly with a
  constructed `BuiltinContext { cwd: workspace root, env: process env,
  stdin }`. That function is already session-free; the interactive session
  wrappers only add terminal I/O and prompt updates, which a
  non-interactive interface omits.
- Mixed chains (any external stage): run external stages through the
  existing `ProcessSupervisor` and splice `Vec<u8>` between stages
  in-process, preserving byte semantics end to end.
- Semantics parity with the shell: every stage runs, stderr passes in
  order, last stdout wins, last exit wins, same inline budget discipline
  as `exec`.
- Authority unchanged: builtins are read-only (`authority: none`);
  external stages run under current execution contracts, exactly as
  `execution.run` does today. Mutations stay fail-closed under Tethers.

## Compatibility implications

- Machine contract: additive. Either a new capability id (e.g.
  `execution.compose`) with its own `invocation` routes, or an optional
  `stages` field on `execution.run`. New id is cleaner: `execution.run`
  keeps its single-argv contract byte-for-byte. Contract version stays
  `0.8`; the static digest rotates (as it does for any definition text).
- CLI: a new `exec` spelling for stages (structured JSON, not a shell
  string). MCP: a new tool or an optional `stages` array on
  `omen_execute` (optional = backward compatible for existing callers).
- `orient` guidance and the `shell.*` summaries already point at
  `execution.run` as the agent equivalent; they would gain one line
  pointing at the composed route when (and only when) it ships.
- Risk if merged carelessly: two composition languages (shell strings
  vs stage arrays) confusing agents. Mitigation: the stage array is the
  ONLY machine composition shape; shell strings stay interactive-only
  and `exec` keeps refusing them (shipped PR #52).

## Suggested acceptance path (post-RC)

1. Accept this design (or amend it) — no code.
2. Implement builtins-only stages first (pure, no spawn, smallest blast
   radius) behind the new capability id.
3. Mixed external stages only after builtins-only proves out in dogfood.
4. Fresh-agent acceptance: the agent must discover the composed route
   from `orient`/`describe` alone and never mistake `exec` for a shell.

## Residual if declined

Client-side composition remains the documented agent workflow. Its known
limits: per-stage brokered overhead, client-side byte splicing, and no
machine access to Omen builtins. None of these is an RC defect; all are
carried as design decisions.
