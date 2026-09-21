# Omen agent discovery

Omen's machine-facing discovery is progressive. A client should load only the
next decision-sized representation:

```text
orient
  -> capabilities [group]
  -> describe <capability>
  -> invoke the selected operation
```

For a multi-step task, use the advisory recipe path:

```text
orient
  -> how <recipe>
  -> execute the named steps explicitly
  -> history / evidence
```

The equivalent MCP tools are `omen_orient`, `omen_capabilities`,
`omen_describe`, `omen_recipe`, `omen_context`, and the action discovery tools.
The `next_actions` field in `orient` supplies both the CLI operation and the
MCP tool name; clients do not need to guess command strings.

## Payload roles

`orient` is the map: identity, workspace, contract version, static contract
digest, capability groups, recipe IDs, dynamic generation status, and structured
next actions.

`capabilities` is the catalogue: capability ID, group, compact summary,
availability, admission, provider, reason, related capability IDs, and a
`describe_ref`. It intentionally does not repeat input/output schemas.

`describe` is the operational detail: arguments, result shape, effects,
authority requirement, bounds, timeout, examples, and the current runtime
overlay. Its `contract_digest` tells a client whether cached static discovery
is still valid.

`how` returns short advisory recipes. Recipes link to capability IDs and grant
no authority; execution and Tethers policy remain separate decisions.

`context` is dynamic workspace/session state. It is not a second copy of the
static catalogue. Use its generation value and `--since`/`since` support to
avoid redistributing unchanged context.

## Truth boundaries

Availability, admission, provider, and reason are structured fields. An
unavailable provider is not represented as an unsupported capability, and
temporary unavailability does not remove the capability from discovery.

Omen reports that authority is required. Tethers remains the authority and
policy owner; discovery and recipes never mint or imply permission.

Machine Contract remains `0.8`. The Preview 7 discovery additions are
compatible projections and references over the existing contract authority.
