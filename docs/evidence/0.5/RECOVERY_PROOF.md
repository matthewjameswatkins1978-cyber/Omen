# Omen 0.5 — Recovery & Degraded Mode Proof

## 1. Principles

1. **Every blocking operation gets a deadline**: No unbounded waits.
2. **Graceful Degraded Mode**: When the daemon is offline, Omen shell and MCP server remain functional in standalone mode.
3. **Bounded Agent Timeouts**: If a model provider hangs or network drops, Agent requests time out after 30 seconds, returning cleanly with Omen state intact.

---

## 2. Verification Scenarios

### Scenario 1: Standalone Shell Fallback
- When `omend` is not running, `InteractiveSession::new` connects cleanly in `[standalone]` mode.
- Local SQLite database and local supervisor handle direct execution and facts without failure.
- Verified in `shell_ux_degraded_and_shared_tests.rs`.

### Scenario 2: Agent Timeout Ceilings
- `TimeoutProvider` wraps any provider with `DEFAULT_AGENT_TIMEOUT` (30 seconds).
- When a provider takes longer than the allocated duration:
  - Returns `AgentError::Timeout`.
  - Dispatches message: `"Agent unavailable: request exceeded 30s. Omen state is unchanged."`.
  - Shell prompt remains responsive; no orphan threads or locked resources remain.

### Scenario 3: Malformed & Hostile JSON-RPC
- `McpServer` handles syntax errors, invalid params, and unknown methods gracefully.
- Returns proper JSON-RPC error frames (`-32700`, `-32601`, `-32602`) without panicking or dropping the connection.
- Verified in `mcp_tests::test_mcp_malformed_json_rpc_handling`.
