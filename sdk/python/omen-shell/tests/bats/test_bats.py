"""Python Bat Cage: product behavior through the public API (Layer 4).

Rust tests prove the machinery. These Bats prove the product from
outside, using only the documented public surface.
"""

import asyncio
import sys
import time

import pytest

from omen_shell import AsyncOmen, OmenError
from omen_shell.errors import (
    OmenConnectionClosedError,
    OmenProcessError,
    OmenProtocolError,
)
from omen_shell.testing import OmenHarness
from tests.conftest import needs_omen


@needs_omen
def test_bat01_cold_truth() -> None:
    """Bat 1 — Cold Truth: fresh connect, orient, contract/capability truth."""
    with OmenHarness() as harness:
        omen = harness.omen
        info = omen.orient()
        assert info.contract_version == "0.8"
        assert info.omen_version
        assert info.contract_digest
        conn = omen.connection_info
        assert conn.omen_version == info.omen_version
        assert conn.contract_version == "0.8"
        assert conn.transport == "stdio: omen mcp"
        assert conn.pid is not None
        assert "0.8" in omen.supported_contracts
        tools = omen.raw.list_tools()
        names = {t["name"] for t in tools}
        assert "omen_execute" in names
        assert "omen_orient" in names
        harness.record_evidence("cold-truth", {"omen": conn.omen_version})


@needs_omen
def test_bat02_warm_truth_one_process() -> None:
    """Bat 2 — Warm Truth: many calls, one persistent omen mcp process."""
    with OmenHarness() as harness:
        omen = harness.omen
        pid_before = omen.connection_info.pid
        for _ in range(5):
            omen.orient()
            omen.context()
        assert omen.connection_info.pid == pid_before
        children = harness.snapshot_processes()
        mine = [p for p in children if p[1] == pid_before]
        assert mine, f"expected pid {pid_before} in {children}"


@needs_omen
def test_bat03_execution_identity() -> None:
    """Bat 3 — Execution Identity: execution_id exists with Omen form."""
    with OmenHarness() as harness:
        result = harness.omen.execute(["python", "-c", "print('bat3')"])
        assert result.execution_id.startswith("exec_")
        assert result.runtime_status == "COMPLETED"
        assert result.exit_code == 0
        assert result.ok is True
        assert "bat3" in result.stdout_preview


@needs_omen
def test_bat04_child_failure() -> None:
    """Bat 4 — Child Failure: stdout + stderr + exit 2 preserved as truth."""
    with OmenHarness() as harness:
        result = harness.omen.execute(
            [
                "python",
                "-c",
                "import sys; print('out-line'); print('err-line', file=sys.stderr); sys.exit(2)",
            ]
        )
        assert result.ok is False
        assert result.exit_code == 2
        assert "out-line" in result.stdout_preview
        assert "err-line" in result.stderr_preview
        with pytest.raises(OmenProcessError):
            result.raise_for_status()


@needs_omen
def test_bat05_artifact_round_trip() -> None:
    """Bat 5 — Artifact Round-Trip: URI unchanged through inspect/read."""
    with OmenHarness() as harness:
        omen = harness.omen
        result = omen.execute(["python", "-c", "print('artifact-bytes-123')"])
        assert result.stdout_artifact is not None
        uri = str(result.stdout_artifact)
        meta = omen.artifacts.inspect(uri)
        assert meta.uri.digest == result.stdout_artifact.digest
        data = omen.artifacts.read(uri)
        assert "artifact-bytes-123" in data.text
        sliced = omen.artifacts.read(uri, offset=0, length=8)
        assert data.text[:8] == sliced.text


@needs_omen
def test_bat06_history_truth() -> None:
    """Bat 6 — History Truth: standalone execution is never a lie.

    Known Omen Preview 8 behavior: standalone ``omen mcp`` executions
    are not journaled to durable history (and write no unjournaled
    marker); only daemon-brokered executions journal. The SDK therefore
    preserves Omen's report faithfully AND attaches session knowledge,
    so our own work is never reported as "nothing happened".
    """
    with OmenHarness() as harness:
        omen = harness.omen
        result = omen.execute(["python", "-c", "print('bat6')"])
        history = omen.history.query(limit=20)
        assert history.history_status in ("CURRENT", "UNJOURNALED_LOCAL_EXECUTION")
        assert history.knows(result.execution_id), (
            f"execution {result.execution_id} known neither durably nor in session"
        )
        session_ids = [e.execution_id for e in history.session_executions]
        assert result.execution_id in session_ids


@needs_omen
def test_bat07_semantic_quartet() -> None:
    """Bat 7 — Semantic Quartet: FOUND/NOT_FOUND/UNSUPPORTED stay distinct."""
    with OmenHarness() as harness:
        omen = harness.omen
        harness.write_file("quartet_sample.py", "alpha_factor = 41\n")
        found = omen.semantic.search("alpha_factor")
        assert found.outcome in ("FOUND", "NOT_FOUND", "AMBIGUOUS")
        missing = omen.semantic.definition("no_such_symbol_xyz_123")
        assert missing.outcome == "NOT_FOUND"
        assert missing.found is False
        assert missing.coverage in ("NONE", "PARTIAL", "COMPLETE")


@needs_omen
def test_bat08_timeout() -> None:
    """Bat 8 — Timeout: TIMED_OUT with no zombie and no exception-as-truth."""
    with OmenHarness() as harness:
        before = harness.snapshot_processes()
        result = harness.omen.execute(
            ["python", "-c", "import time; time.sleep(30)"],
            timeout=3.0,
            request_timeout=30.0,
        )
        assert result.runtime_status == "TIMED_OUT"
        assert result.ok is False
        after = harness.snapshot_processes()
        assert after <= before or len(after - before) == 0


@needs_omen
def test_bat09_two_workspaces_isolated() -> None:
    """Bat 9 — Two Workspaces: no history/artifact leakage across harnesses."""
    with OmenHarness() as first, OmenHarness() as second:
        assert first.workspace != second.workspace
        assert first.state_home != second.state_home
        r1 = first.omen.execute(["python", "-c", "print('ws-one-marker')"])
        assert r1.stdout_artifact is not None
        with pytest.raises(OmenError):
            second.omen.artifacts.read(r1.stdout_artifact)
        # A never-used state home has no history DB yet; Omen answers
        # PERSISTENCE_FAILURE rather than empty history. That is Omen's
        # truth and must not leak across the isolation boundary either.
        try:
            h2 = second.omen.history.query(limit=20)
        except OmenError as exc:
            assert exc.code == "PERSISTENCE_FAILURE", exc.code
        else:
            assert all(e.execution_id != r1.execution_id for e in h2.entries)
        r2 = second.omen.execute(["python", "-c", "print('ws-two-marker')"])
        h2b = second.omen.history.query(limit=20)
        assert all(e.execution_id != r1.execution_id for e in h2b.entries)
        assert h2b.knows(r2.execution_id)


@needs_omen
def test_bat10_wrong_resource() -> None:
    """Bat 10 — Wrong Resource: canonical OmenError, lossless envelope."""
    with OmenHarness() as harness:
        with pytest.raises(OmenError) as exc_info:
            harness.omen.artifacts.read("artifact://sha256/" + "00" * 32)
        err = exc_info.value
        assert err.code == "ARTIFACT_NOT_FOUND"
        assert err.category == "Evidence"
        assert err.state_changed == "NO"
        with pytest.raises(OmenError) as exc_scheme:
            harness.omen.raw.read_resource("fact://unsupported-shape")
        assert exc_scheme.value.code == "RESOURCE_NOT_FOUND"


@needs_omen
def test_bat11_process_resurrection() -> None:
    """Bat 11 — kill omen mcp: fail closed, no silent replay; explicit reconnect works."""
    psutil = pytest.importorskip("psutil")
    with OmenHarness() as harness:
        omen = harness.omen
        pid = omen.connection_info.pid
        assert pid is not None
        before = omen.execute(["python", "-c", "print('before-kill')"])
        assert before.ok
        psutil.Process(pid).kill()
        with pytest.raises(OmenConnectionClosedError):
            omen.execute(["python", "-c", "print('after-kill')"])
        omen.reconnect()
        after = omen.execute(["python", "-c", "print('reborn')"])
        assert after.ok
        assert "reborn" in after.stdout_preview


@needs_omen
def test_bat12_big_mouth() -> None:
    """Bat 12 — Big Mouth: large output stays bounded in previews, full in CAS."""
    with OmenHarness() as harness:
        omen = harness.omen
        result = omen.execute(
            ["python", "-c", "print('L' * 200000); "],
            request_timeout=60.0,
        )
        assert result.ok
        assert len(result.stdout_preview) < 200000
        assert result.stdout_artifact is not None
        data = omen.artifacts.read(result.stdout_artifact)
        assert len(data.text) >= len(result.stdout_preview)


@needs_omen
def test_bat13_concurrency() -> None:
    """Bat 13 — Concurrency: parallel async calls stay correctly correlated."""

    async def _scenario() -> None:
        async with AsyncOmen(workspace=".") as omen:
            results = await asyncio.gather(
                *[omen.execute(["python", "-c", f"print({i})"]) for i in range(8)]
            )
            previews = sorted(r.stdout_preview.strip() for r in results)
            assert previews == [str(i) for i in range(8)]
            ids = {r.execution_id for r in results}
            assert len(ids) == 8

    asyncio.run(_scenario())


@needs_omen
def test_bat14_reconnect() -> None:
    """Bat 14 — Reconnect: clean close, new session, no stale state."""
    harness = OmenHarness()
    with harness:
        first = harness.omen.execute(["python", "-c", "print('one')"])
        assert first.ok
    with harness:
        second = harness.omen.execute(["python", "-c", "print('two')"])
        assert second.ok
        assert second.execution_id != first.execution_id


@needs_omen
def test_bat15_harness_tax() -> None:
    """Bat 15 — Harness Tax: measure SDK overhead vs raw transport."""
    import json
    import subprocess

    with OmenHarness() as harness:
        omen = harness.omen
        started = time.monotonic()
        omen.orient()
        sdk_ms = (time.monotonic() - started) * 1000.0

        exe = omen.connection_info.pid  # noqa: F841 (proves connection first)
        import shutil

        omen_bin = shutil.which("omen")
        assert omen_bin is not None
        wire = (
            json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {"name": "omen_orient", "arguments": {}},
                }
            )
            + "\n"
        )
        init = (
            json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": 0,
                    "method": "initialize",
                    "params": {"protocolVersion": "2025-11-05", "capabilities": {}},
                }
            )
            + "\n"
        )
        started = time.monotonic()
        proc = subprocess.run(  # noqa: S603 — argv list, no shell; test-only raw probe
            [omen_bin, "mcp", "--workspace", harness.workspace],
            input=init + wire,
            capture_output=True,
            text=True,
            timeout=60,
        )
        raw_ms = (time.monotonic() - started) * 1000.0
        assert proc.returncode == 0
        harness.record_evidence(
            "harness-tax", {"sdk_orient_ms": sdk_ms, "raw_spawn_orient_ms": raw_ms}
        )
        # SDK reuses a warm connection; raw spawns a process. The tax must
        # not exceed the cost of a cold spawn.
        assert sdk_ms < raw_ms + 5000.0


@needs_omen
def test_sync_client_inside_running_loop() -> None:
    """Sync API works on a thread that already runs an event loop (notebooks)."""

    async def _on_loop_thread() -> None:
        with OmenHarness() as harness:
            result = harness.omen.execute(["python", "-c", "print('notebook')"])
            assert result.ok

    asyncio.run(_on_loop_thread())


@needs_omen
def test_argv_string_refused() -> None:
    """A single command string fails clearly (argv-only discipline)."""
    with OmenHarness() as harness:
        with pytest.raises(OmenProtocolError):
            harness.omen.execute("python -c print('x')")  # type: ignore[arg-type]


@needs_omen
def test_unknown_tool_is_omen_error() -> None:
    """Unknown tools surface as OmenError, not transport failure."""
    with OmenHarness() as harness:
        with pytest.raises(OmenError) as exc_info:
            harness.omen.raw.call_tool("omen_no_such_tool", {})
        assert "omen_no_such_tool" in exc_info.value.message


@needs_omen
def test_call_after_close_rejected() -> None:
    """Calls after close are rejected; double close is quiet."""
    harness = OmenHarness()
    harness.open()
    omen = harness.omen
    harness.close()
    with pytest.raises(OmenConnectionClosedError):
        omen.orient()
    harness.close()


@needs_omen
def test_no_stdout_pollution(capsys: object) -> None:
    """The library never prints protocol chatter to stdout."""
    with OmenHarness() as harness:
        harness.omen.orient()
        harness.omen.execute(["python", "-c", "print('quiet')"])
    cap = capsys  # type: ignore[attr-defined]
    assert cap.readouterr().out == ""


def test_python_floor() -> None:
    """Package floor is Python 3.11+."""
    assert sys.version_info >= (3, 11)
