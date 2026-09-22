# omen-shell public API (0.9.0a1) — freeze candidate

Small top level; everything else lives in namespaces reached through
the client. `omen_shell._internal`, `omen_shell.transport`,
`omen_shell.protocol`, and `omen_shell.compatibility` carry no
compatibility promise.

## Top-level imports (`from omen_shell import ...`)

`Omen`, `AsyncOmen`, `OmenError`, `OmenShellError`, `ExecutionResult`,
`SemanticResult`, `ArtifactRef`, `ExecutionId`, `HistoryResult`,
`LocalExecution`, `__version__`.

Advanced error/model types stay in `omen_shell.errors` /
`omen_shell.models` (`OmenNotFoundError`, `OmenTransportError`,
`OmenProtocolError`, `OmenCompatibilityError`,
`OmenConnectionClosedError`, `OmenRequestTimeout`, `OmenProcessError`,
`Orientation`, `HistoryEntry`, `ArtifactData`, `ArtifactMetadata`,
`CapabilityInfo`, `ConnectionInfo`).

## Clients

- `Omen(workspace=".", *, executable=None, connect_timeout=30.0, request_timeout=120.0, shutdown_grace=5.0, shutdown_hard=5.0, max_message_bytes=16777216, env=None)` — sync façade (private loop thread; safe inside notebooks).
- `Omen.connect(...)` / `with Omen(...) as omen:` / `omen.close()` (idempotent).
- `AsyncOmen(...)` / `await AsyncOmen.connect(...)` /
  `async with AsyncOmen(...) as omen:` / `await omen.close()`.
- Shared methods: `orient()`, `capabilities(*, group=None)`,
  `describe(capability_id)`, `how(recipe_id)`,
  `context(*, since=None)`, `workspace_status()`,
  `execution_status(request_id)`, `execute(argv, *, timeout=None,
  request_timeout=None, cwd=None, tool="exec", operation="")`,
  `reconnect()`, `connection_info`, `supported_contracts`.
  Failed `connect`/`_open`/`reconnect` leaves zero transport ownership
  (process reaped, tasks stopped, pending failed) and re-raises the
  original failure. A successful `reconnect()` resets the Python
  session overlay (`session_executions`); durable Omen history is untouched.
- `execute()` accepts argv sequences only; a command string raises
  `OmenProtocolError`. Non-zero child exit returns `ExecutionResult`.

## Namespaces (sync and async mirrors)

Sync namespaces are concrete typed classes (`SyncRawAccessor`,
`SyncArtifacts`, `SyncSemantic`, `SyncActions`, `SyncHistory`,
`SyncFacts`) delegating to the async implementation — one semantic
layer only, with consumer-visible signatures (proven by
`typing_consumer/consumer_check.py` under `mypy --strict`).

- `omen.raw.list_tools()`, `call_tool(name, arguments=None)`,
  `list_resources()`, `read_resource(uri)`, `ping()`.
  Resource failures stay `OmenProtocolError` (JSON-RPC truth, raw kept).
- `omen.artifacts.read(uri, *, offset=0, length=None)` → `ArtifactData`;
  `inspect(uri)` → `ArtifactMetadata`. Server truncates at 64 KiB;
  slicing applies to the returned payload; completeness beyond the
  served bound is never claimed.
- `omen.semantic.definition(symbol, *, file=None, line=None, col=None)`,
  `references(symbol, *, file=None, line=None, col=None, limit=50)`,
  `search(query, *, limit=50)` → `SemanticResult` (never None).
- `omen.actions.list()`, `show(action_id)`, `plan(action_id)` (no run —
  Omen exposes no action-run tool; kept CLI-side by Omen's design).
- `omen.history.query(*, limit=20, all_sessions=True, session_id=None)`
  → `HistoryResult` (durable entries + `history_status` +
  `local_execution` marker + `session_executions`).
- `omen.facts.query(*, filter="all")` → list of fact dicts.

## Models (selected fields)

- `ExecutionResult`: `execution_id`, `runtime_status`, `exit_code`,
  `duration_ms`, `stdout_preview`, `stderr_preview`,
  `stdout_artifact`, `stderr_artifact`, `raw`, `extra`; `ok`
  (COMPLETED and exit 0); `raise_for_status()`; `to_dict()`.
- `HistoryResult`: `entries`, `history_status`, `local_execution`,
  `session_executions`; `knows(execution_id)`.
- `SemanticResult[T]`: `operation`, `outcome`, `coverage`, `data`,
  `generation`, `raw`, `extra`; `found`.
- `ArtifactRef`: `uri`, `algorithm`, `digest`; `parse()`.
- `OmenError`: `code`, `message`, `category`, `state_changed`,
  `retryability`, `evidence`, `details`, `schema_version`, `raw`;
  `from_envelope()`; `to_dict()`.
- Every high-level model keeps `.raw` (Omen's response) for debugging.

## Testing

`from omen_shell.testing import OmenHarness` —
`OmenHarness(workspace=None, *, executable=None, state_home=None,
isolate_state=True, evidence_name=None, request_timeout=120.0)` with
`.open()/.close()`, context-manager support, `.omen`, `.workspace`,
`.state_home`, `.write_file(relpath, content)`,
`.run_native(argv, ...)`, `.snapshot_processes()`,
`.wait_for(predicate, *, timeout, poll, what)`,
`.record_evidence(label, payload)`.

Env: `OMEN_EXE` (executable override), `OMEN_STATE_HOME` (isolated by
default per harness), `OMEN_TEST_EVIDENCE_DIR` (retain evidence),
`OMEN_SHELL_TRACE_PAYLOADS=1` (unsafe full-payload debug logging).
