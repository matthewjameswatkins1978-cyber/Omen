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
