# omen-shell

`omen-shell` is the official Python interface to Omen.

Python provides orchestration.
Omen provides machine truth.

```python
from omen_shell import Omen

with Omen(workspace=".") as omen:
    print(omen.orient())

    result = omen.execute(["python", "-c", "print('hello from Omen')"])

    print(result.execution_id)
    print(result.exit_code)
```

## Install

Requires Python 3.11+ and an installed `omen` binary (0.9.0-preview.8,
Machine Contract 0.8). Zero runtime dependencies — standard library only.

```bash
pip install omen-shell
```

> Do not confuse with the unrelated `omen` distribution on PyPI.
> The distribution name is `omen-shell`; the import is `omen_shell`.

## Truth rules (read these once)

- Child non-zero exit is `ExecutionResult` truth, not an exception.
  Use `result.raise_for_status()` only when you want it to raise.
- `retryability` is information, never an instruction: the SDK never
  retries consequential operations automatically.
- `NOT_FOUND` is a semantic result, not `None` and not a provider failure.
  `PARTIAL` is not `COMPLETE`.
- Cancelling your `await` does not cancel Omen's execution. `close()`
  does not roll anything back.
- One `Omen` holds one persistent `omen mcp` process. If it dies, the
  client fails closed — reconnect explicitly.

## Layout

- `src/omen_shell/` — the package (`client.py`, `async_client.py`,
  `transport.py`, `protocol.py`, `models.py`, `errors.py`,
  `compatibility.py`, `artifacts`/`semantic`/`actions`/`history`
  namespaces, `testing/` harness).
- `tests/unit` — no Omen required. `tests/integration` — fake MCP
  subprocess. `tests/bats` — real Omen product proofs.
  `tests/stateful` — Hypothesis lifecycle exploration.
- `examples/` — runnable `basic.py`, `async_basic.py`,
  `semantic.py`, `harness.py`.

Full documentation lives in the Omen repo under `docs/python/`.
Set `OMEN_SHELL_TRACE_PAYLOADS=1` only for debugging: it logs full
payloads and is unsafe for sensitive data.
