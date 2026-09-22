"""Tiny fake JSON-RPC/MCP subprocess for transport-level tests.

Speaks newline-delimited JSON-RPC like ``omen mcp`` but with scripted
behavior selected by the ``FAKE_MCP_MODE`` environment variable:

* ``normal`` — answer initialize/tools/list/tools/call minimally.
* ``out-of-order`` — delay the first tools/call response until the
  second arrives, then answer both (order swapped).
* ``malformed`` — emit one non-JSON line, then behave as normal.
* ``unknown-id`` — emit a response for an id nobody asked about.
* ``exit-early`` — exit(0) after reading the first line.
* ``stderr-noise`` — write junk to stderr, behave as normal.
* ``large`` — answer tools/call with a ~2 MiB text payload.
* ``slow`` — sleep 30s before answering tools/call (tests use short
  deadlines to observe the timeout, then kill).
* ``duplicate`` — send the same response twice for one request.

Every mode answers ``initialize`` so handshake tests stay simple.
"""

from __future__ import annotations

import json
import os
import sys
import time

MODE = os.environ.get("FAKE_MCP_MODE", "normal")


def _send(obj: object) -> None:
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def _ok(call_id: object, result: object) -> None:
    _send({"jsonrpc": "2.0", "id": call_id, "result": result})


def _tool_text(payload: object, *, is_error: bool = False) -> dict[str, object]:
    result: dict[str, object] = {
        "content": [{"type": "text", "text": json.dumps(payload)}],
    }
    if is_error:
        result["isError"] = True
        result["error"] = {
            "schema_version": 1,
            "code": "FAKE_ERROR",
            "message": "scripted fake error",
            "category": "Validation",
            "state_changed": "NO",
            "retryability": "NEVER",
            "evidence": {"available": False, "artifacts": []},
            "details": {},
        }
    return result


def _handle(message: dict[str, object], pending: list[dict[str, object]]) -> None:
    method = message.get("method")
    call_id = message.get("id")
    params = message.get("params")
    args = params.get("arguments", {}) if isinstance(params, dict) else {}

    if method == "initialize":
        _ok(
            call_id,
            {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "serverInfo": {"name": "fake-mcp", "version": "0.0.0-fake"},
            },
        )
        return
    if method == "ping":
        _ok(call_id, {})
        return
    if method == "tools/list":
        _ok(
            call_id,
            {
                "tools": [
                    {
                        "name": "fake_echo",
                        "description": "echo",
                        "inputSchema": {"type": "object"},
                    }
                ]
            },
        )
        return
    if method == "tools/call":
        name = args.get("name") if isinstance(args, dict) else None
        if MODE == "out-of-order" and not pending:
            pending.append(message)
            return
        if MODE == "slow":
            time.sleep(30.0)
        payload: dict[str, object] = {"echo": args, "tool": name}
        if MODE == "large":
            payload["blob"] = "x" * (2 * 1024 * 1024)
        if MODE == "duplicate":
            _ok(call_id, _tool_text(payload))
            _ok(call_id, _tool_text(payload))
            return
        if MODE == "out-of-order" and pending:
            first = pending.pop(0)
            _ok(call_id, _tool_text(payload))
            _ok(first.get("id"), _tool_text({"echo": "first-was-delayed"}))
            return
        _ok(call_id, _tool_text(payload))
        return
    if call_id is not None:
        _send(
            {
                "jsonrpc": "2.0",
                "id": call_id,
                "error": {"code": -32601, "message": "Method not found"},
            }
        )


def main() -> int:
    if MODE == "stderr-noise":
        sys.stderr.write("fake setup noise line\n")
        sys.stderr.flush()
    if MODE == "malformed":
        sys.stdout.write("this is not json\n")
        sys.stdout.flush()
    if MODE == "unknown-id":
        _send({"jsonrpc": "2.0", "id": "nobody-asked", "result": {}})
    pending: list[dict[str, object]] = []
    first = True
    for line in sys.stdin:
        if not line.strip():
            continue
        try:
            message = json.loads(line)
        except ValueError:
            _send(
                {
                    "jsonrpc": "2.0",
                    "id": None,
                    "error": {"code": -32700, "message": "Parse error"},
                }
            )
            continue
        if MODE == "exit-early" and first:
            return 0
        first = False
        if isinstance(message, dict):
            _handle(message, pending)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
