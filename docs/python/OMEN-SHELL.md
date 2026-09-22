# omen-shell — Python SDK for Omen

`omen-shell` is the official Python interface to Omen
(distribution `omen-shell`, import `omen_shell`, current `0.9.0a1`).

Python provides orchestration. Omen provides machine truth.
The SDK provides ergonomics. Omen remains the sole authority for
execution, evidence, semantics, identity, history, resources, and
machine truth.

## Installation

Python 3.11+ required (3.11, 3.12, 3.13, 3.14 supported).
Runtime dependencies: none (standard library only).

```bash
pip install omen-shell
```

You also need an `omen` binary (Preview 9, Machine Contract 0.8).
Resolution order: `executable=` argument → `OMEN_EXE` env →
`omen` on `PATH`. Otherwise `OmenNotFoundError` with remediation.

## Quickstart

```python
from omen_shell import Omen

with Omen(workspace=".") as omen:
    info = omen.orient()
    result = omen.execute(["cargo", "test"])
    print(result.execution_id, result.exit_code)
    history = omen.history.query(limit=20)
```

Async:

```python
from omen_shell import AsyncOmen

async with AsyncOmen(workspace=".") as omen:
    result = await omen.execute(["cargo", "test"])
```

Or explicit connect/close:

```python
omen = Omen.connect(workspace=".")
try:
    ...
finally:
    omen.close()
```

## Sync API / Async API

The async client is the semantic authority; the sync `Omen` is a
façade over it on a private loop thread. One client holds one
persistent `omen mcp` process — never spawn-per-call. The sync API
works inside notebooks (no `asyncio.run` per call).

Discovery: `orient()`, `capabilities(group=...)`,
`describe(...)`, `how(...)`, `context(...)`. Forward access:
`omen.raw.list_tools()` / `call_tool(...)` /
`list_resources()` / `read_resource(...)` / `ping()`.

## Execution

Argv sequences only — `"cargo test"` as one string fails clearly.
Options map 1:1 to the real Omen surface: `argv`, `timeout` (Omen
child deadline), `request_timeout` (Python transport deadline),
`cwd`, `tool`, `operation`. No stdin/budget/env options exist because
Omen's MCP surface has none.

## Artifacts

```python
meta = omen.artifacts.inspect(result.stdout_artifact)
data = omen.artifacts.read(result.stdout_artifact, offset=0, length=4096)
```

URIs pass through unchanged (`ArtifactRef` preserves
`artifact://sha256/<digest>`). Omen truncates reads server-side at
64 KiB; slicing applies to the returned payload. Missing blobs and
unsupported schemes surface as JSON-RPC protocol truth
(`OmenProtocolError` with code `-32004`, message, and raw reply) —
the SDK invents no canonical `OmenError` fields from message text.

## Cancellation

```python
record = omen.cancel_execution(result.execution_id)
record["outcome"]  # {"outcome": "TerminationConfirmed"} | {"outcome": "DispatchPrevented"} | {"outcome": "AlreadyFinished", ...} | ...
```

Intent and proof stay distinct end to end: Omen records
`CancellationRequested`, performs the physical tree-stop, and only
observed death becomes `TerminationConfirmed`. A stop that arrives
before dispatch reports `DispatchPrevented` (no spawn, no tree-stop,
no death claimed). Unconfirmed stops stay
`OutcomeUnknown`; finished executions report `AlreadyFinished` without
rewriting history. The SDK passes the record through opaquely — it never
reinterprets the outcome. Cancelling needs the daemon broker; standalone
mode refuses explicitly instead of faking a stop.

## History

```python
history = omen.history.query(limit=20)
history.history_status        # CURRENT | UNJOURNALED_LOCAL_EXECUTION
history.local_execution       # Omen's marker when present (never hidden)
history.session_executions    # work THIS client performed (not durable)
history.knows(execution_id)   # durable OR session knowledge
```

Omen is the authority for durable history. Standalone `omen mcp`
executions are recorded directly into durable history under their
canonical ID (same as interactive standalone). Direct `omen exec`
remains marker-based by design (`UNJOURNALED_LOCAL_EXECUTION`), and MCP
history never hides that marker when durable entries also exist.
The SDK attaches session knowledge so it never claims "nothing happened"
for its own work — clearly separated from durable truth.

## Semantics

`definition()` / `references()` / `search()` return
`SemanticResult` with `outcome` (FOUND/NOT_FOUND/AMBIGUOUS) and
`coverage` (COMPLETE/PARTIAL/NONE) preserved. `NOT_FOUND` is a result.

## Errors

`OmenShellError` tree: `OmenNotFoundError`, `OmenTransportError`,
`OmenProtocolError`, `OmenCompatibilityError`,
`OmenConnectionClosedError`, `OmenRequestTimeout`. `OmenError` is
Omen's canonical envelope, mapped losslessly (unknown future values
stay unknown). `OmenProcessError` only comes from
`result.raise_for_status()` — never from `execute()`.

Distinctions that matter: child non-zero ≠ SDK failure; `retryability`
≠ permission to replay (the SDK never auto-retries consequential
work); Python cancellation ≠ Omen cancellation (late responses are
dropped by id, Omen-side state unclaimed); `close()` ≠ rollback; a
dead transport fails closed — reconnect explicitly.

## Compatibility

Supported contracts: `{"0.8"}` (`SUPPORTED_MACHINE_CONTRACTS`).
Anything else raises `OmenCompatibilityError` at connect. A changed
contract *digest* within 0.8 is diagnostic, not incompatible.

## Testing harness

`OmenHarness` gives an isolated workspace, isolated Omen state
(`OMEN_STATE_HOME` per harness), fixture files, native comparison
(`run_native`), process snapshots, bounded waiting, and evidence
capture (`OMEN_TEST_EVIDENCE_DIR`). It uses the public client.

## Lifecycle

Bounded timeouts on connect/request/shutdown; deterministic shutdown
(stdin EOF → grace → terminate → kill); no leaked processes, tasks, or
pending futures. A failed connect raises AND leaves zero transport
ownership behind (no reliance on GC). A successful `reconnect()`
resets the Python session overlay; durable Omen history is untouched.
No `shell=True` anywhere, no network, no telemetry,
no model calls, no secret logging (default logs carry ids/durations
only). No streaming in E1 — Omen has no execution event stream, and
the SDK will not fake one by polling.

## Security / Troubleshooting

- `OmenNotFoundError` → install Omen or set `OMEN_EXE`.
- `OmenCompatibilityError` → runtime contract outside `{"0.8"}`.
- `OmenConnectionClosedError` → process died; inspect stderr excerpt
  in the message; `reconnect()` explicitly.
- `OmenRequestTimeout` → transport deadline (distinct from Omen's
  execution `timeout`); the Omen-side outcome is unknown by design.
- `PERSISTENCE_FAILURE` on first history query → fresh state root has
  no history DB until the first execution creates it (Omen's truth
  about empty state).
- Hangs → every wait is bounded; check `request_timeout` and shutdown
  grace settings. Debug payloads: `OMEN_SHELL_TRACE_PAYLOADS=1`
  (unsafe for sensitive data).
