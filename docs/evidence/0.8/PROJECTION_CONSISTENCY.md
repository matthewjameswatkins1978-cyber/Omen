# Omen 0.8 Projection Consistency Evidence

The CLI, MCP, and interactive surfaces use the same `omen-core` machine
contract, context projection, recipe definitions, and composition planner.
They do not maintain protocol-specific capability registries.

The consistency checks cover:

- stable contract digest independent of live status overlays;
- unique capability identifiers and resolving recipe steps;
- MCP tool presence for every canonical discovery/action projection;
- MCP capability filtering and description of `mutation.threadmoth` from the
  canonical definition;
- MCP action listing and planning from the same `Omen.toml` fixture;
- CLI action listing/planning and existing digest-bound execution tests;
- interactive command wiring for orientation, capability/recipe discovery, and
  read-only action planning.

All projections preserve the authority boundary: planning and discovery never
grant permission, and no projection fabricates a Tethers decision.
