# Omen MCP surface (Preview 8, contract 0.8) — as seen by omen-shell

Source of truth: `crates/omen-mcp/src/server.rs`, `protocol.rs`.
Transport: newline-delimited JSON-RPC 2.0 on stdio. Pure JSON on
stdout; stderr is OS/setup noise, never parsed. No `Content-Length`
framing. `id` echoes verbatim (number or string); omitted `id` means
*notification* and gets **zero bytes back**. Blank lines ignored.
Unparseable input → `{"id": null, "error": {"code": -32700, ...}}`.
stdin EOF → clean exit 0. No `shutdown`/`exit` method.

## initialize

`{"method": "initialize", "params": {"protocolVersion": "2025-11-25", ...}}`.
Accepted versions: `2024-11-05`, `2025-03-26`, `2025-06-18`, `2025-11-25`
(latest). Missing version → success with latest. Well-formed but
unknown date → negotiated to latest (not an error). Malformed →
`-32602 "Invalid MCP protocol version: '<offered>'"`. Only
`params.protocolVersion` is read; `clientInfo`/`capabilities` ignored.

Result: `{"protocolVersion", "capabilities": {"tools": {"listChanged":
false}, "resources": {"subscribe": false, "listChanged": false}},
"serverInfo": {"name": "omen-mcp", "version": "<cargo version>"}}`.

`notifications/initialized` (or bare `initialized`): swallowed, never
required — the server is stateless per request. `ping` → `result: {}`.

## tools/list → tools/call

`tools/list` takes no params; returns 21 tools in order:
`omen_orient`, `omen_capabilities`, `omen_describe`, `omen_recipe`,
`omen_context`, `omen_action_list`, `omen_action_show`,
`omen_action_plan`, `omen_workspace_status`, `omen_facts_query`,
`omen_execute`, `omen_execution_status`, `omen_history_query`,
`omen_services_list`, `omen_services_control`,
`omen_capabilities_discover`, `omen_symbol_search`,
`omen_symbol_definition`, `omen_symbol_references`,
`omen_structure_search`, `omen_package_query`.

`tools/call`: `{"name", "arguments": {}}` (missing arguments → `{}`).
Unknown tool → **success envelope with `isError: true`** (not a
JSON-RPC error). Result shape:

- success: `{"content": [{"type": "text", "text": "<JSON string>"}]}`
  (`isError`/`error` omitted; the text must be JSON-parsed again).
- domain error: same content (text = bare message only) plus
  `isError: true` and canonical `error`:
  `{schema_version, code, message, category, state_changed,
  retryability, evidence: {available, artifacts}, details}`.
- protocol errors (bad method/params/version/parse): JSON-RPC `error`
  object (`-32700`, `-32600`, `-32601`, `-32602`).

Per-tool arguments (R required / O optional):

| tool | arguments |
|---|---|
| `omen_orient` | none |
| `omen_capabilities` | O `group?: string` (no match → empty list) |
| `omen_describe` | R `capability_id: string` |
| `omen_recipe` | R `recipe_id: string` |
| `omen_context` | O `since?: int >= 0` |
| `omen_action_list` | none. **No `omen_action_run` exists** (CLI-only) |
| `omen_action_show` / `omen_action_plan` | R `action_id: string` |
| `omen_workspace_status` | none |
| `omen_facts_query` | O `filter?: "all"\|"current"\|"dirty"` (default `all`) |
| `omen_execute` | R `argv: string[]` (non-empty); O `tool` (default `"exec"`), `operation` (default `""`), `cwd` (default workspace), `timeout_ms` (default `60000`). **No stdin/budget/env args** — stdin always Closed, `inline_budget=8192` hardcoded |
| `omen_execution_status` | R `request_id: string` (caller idempotency key, max 256; **not** execution_id) |
| `omen_history_query` | O `all_sessions` (default true), `session_id`, `limit` (default 20, enforced 1..100). **No `since`** |
| `omen_services_list` | none (needs daemon or errors) |
| `omen_services_control` | R `action: start\|stop\|restart`, `name`; O `command` (start only) |
| `omen_symbol_search` | R `query`; O `limit` (default 50) |
| `omen_symbol_definition` | R `symbol`; O `file`, `line` (0-indexed), `col` (0-indexed) |
| `omen_symbol_references` | R `symbol`; O `file`, `line`, `col`, `limit` (default 50) |
| `omen_structure_search` | R `pattern` (ast-grep); O `language` (default `"rust"`), `limit` (default 50) |
| `omen_package_query` | O `target?: string` |

## resources/list → resources/read

`resources/list` (no params) returns exactly three descriptors with
`uri`/`name`/`mimeType`: `fact://` (Omen Facts), `artifact://` (CAS
Artifacts), `proc://` (Managed Services).

`resources/read`: `{"uri": string}` required. Only
`artifact://sha256/<hex>` is served (blob truncated to **64 KiB**,
lossy-UTF-8 text): `{"contents": [{"uri", "mimeType": "text/plain",
"text"}]}`. Missing blob → `-32004 "Artifact not found in CAS:
<digest>"`. Anything else (`fact://…`, `proc://…`, bare schemes) →
`-32004 "Resource not found or unsupported scheme: <uri>"`.
omen-shell maps `-32004` to `OmenError` (`ARTIFACT_NOT_FOUND` /
`RESOURCE_NOT_FOUND` / `RESOURCE_ERROR`, category `Evidence`); other
JSON-RPC errors stay `OmenProtocolError`.

## Result shapes (inner `content[0].text` JSON)

- **orient**: `{contract_version, omen_version, contract_digest,
  context_generation, generation_status, workspace: {name, root},
  platform, backend, capability_groups[], surfaces: {cli, mcp,
  interactive}, references, recipes[], next[], next_actions[]}`.
- **execute** (daemon and standalone): `{execution_id,
  runtime_status (COMPLETED/FAILED/TIMED_OUT/…), exit_code (int|null),
  duration_ms, stdout_preview, stderr_preview, stdout_artifact (uri|null),
  stderr_artifact (uri|null)}`.
- **history_query**: bare `HistoryResult{schema_version, entries[],
  limit, all_sessions, ordering, pagination}` unless
  `<state>/local-execution-status.json` exists → wrapped `{history,
  history_status: "UNJOURNALED_LOCAL_EXECUTION", local_execution}`.
  Entry: `{sequence, execution_id, session_id, command, status
  (COMPLETED|FAILED|REFUSED|TIMED_OUT|PARTIAL|UNKNOWN), recorded_at,
  duration_ms, state_changed?, evidence[], error?}`.
- **semantic** (`definition`/`symbol_search`/`references`):
  `SemanticResult{schema_version, operation (snake_case), outcome
  (FOUND|NOT_FOUND|AMBIGUOUS), coverage (COMPLETE|PARTIAL|NONE),
  generation: {workspace_generation, provider_generation, index_digest?},
  data}`.
- **workspace_status**: daemon →
  `{workspace_id, workspace_path, epoch, sequence, active_facts_count,
  dirty_facts_count, services_count, available_tools[]}`; standalone →
  `{workspace_path, workspace_root, mode: "standalone"}`.
- **facts_query**: `FactInfo[]` (`fact_id, resource_uri, value,
  validity, assurance`); standalone → `[]`.
- **execution_status**: `{consequential_request_id, execution_id|null,
  status: NotSeen|Accepted|Running|Completed|Failed|Unknown}`.
- **capabilities_discover**: product/boundaries map incl. `cas.hasher:
  sha256`, `uri_scheme: artifact://sha256/<hash>`.
- **actions**: list → `{config_present, actions: [{action_id,
  description, step_count}]}`; plan → full `ActionPlan` incl.
  `plan_digest`, `planning_valid`, `admission_snapshot_only`.
- **context**: no `since` → snapshot (`delta: CURRENT_SNAPSHOT`); match
  → `{changed: false, ...}`; mismatch → `DELTA_UNAVAILABLE` error.

## What the server does NOT have

No progress/cancel notifications, no server-initiated requests, no
`prompts/sampling/roots/logging/elicitation`, no per-call MCP timeout
(except `omen_execute.timeout_ms`), no max-message cap on its side,
no `resources/subscribe`. Unknown methods → `-32601`.

## Process & state

Start: `omen mcp --workspace <path>` (argv, never shell). Workspace is
canonicalized; missing/not-a-dir → process exits non-zero before any
protocol. State root: `OMEN_STATE_HOME` →
`<it>/workspaces/ws_<16hex>/`, else platform default (`state.sqlite`,
`local-execution-status.json`). Fresh state roots have no history DB
until the first execution creates it — `history_query` before that
answers `PERSISTENCE_FAILURE` (omen-shell surfaces it; it is Omen's
truth about empty state, not a bug in the query).
