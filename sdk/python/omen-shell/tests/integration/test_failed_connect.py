"""Failed-connect cleanup regressions (Repair 1, Layer 2).

Invariant: connect succeeds OR connect raises AND leaves zero
transport/process/task ownership behind — for AsyncOmen.connect,
async-with, the sync Omen constructor, and reconnect. The fake server
fails AFTER the process has spawned (bad contract / bad payload), so
these tests prove cleanup of a live child, not of a spawn failure.
All waits bounded; no sleep-and-hope.
"""

import asyncio
import sys
import threading
from pathlib import Path
from typing import Any
from unittest import mock

import pytest

from omen_shell.async_client import AsyncOmen
from omen_shell.client import Omen
from omen_shell.errors import (
    OmenCompatibilityError,
    OmenConnectionClosedError,
    OmenNotFoundError,
    OmenProtocolError,
)
from omen_shell.transport import AsyncStdioTransport

FAKE = str(Path(__file__).resolve().parent.parent / "support" / "fake_mcp_server.py")


def _run(coro: Any) -> Any:
    return asyncio.run(coro)


class _Recorder:
    """Records transports created inside the patched client module."""

    def __init__(self) -> None:
        self.created: list[AsyncStdioTransport] = []

    def __call__(self, *args: Any, **kwargs: Any) -> AsyncStdioTransport:
        transport = AsyncStdioTransport(*args, **kwargs)
        self.created.append(transport)
        return transport


def _patches(mode: str, recorder: _Recorder) -> Any:
    stack = mock.patch(
        "omen_shell.transport.start_command",
        return_value=[sys.executable, FAKE],
    )
    stack2 = mock.patch("omen_shell.async_client.AsyncStdioTransport", recorder)
    return stack, stack2


async def _assert_fully_closed(transport: AsyncStdioTransport) -> None:
    proc = transport._process
    assert proc is not None, "child must have spawned before the failure"
    # Bounded reap: wait_for fails the test (not hangs) if the child lives.
    await asyncio.wait_for(proc.wait(), timeout=10.0)
    assert proc.returncode is not None
    assert transport.pending_count() == 0
    for task in (transport._reader_task, transport._stderr_task):
        assert task is None or task.done()


def test_failed_connect_bad_contract_cleans_up() -> None:
    async def _scenario() -> None:
        recorder = _Recorder()
        stacks = _patches("bad-contract", recorder)
        for stack in stacks:
            stack.start()
        try:
            client = AsyncOmen(
                workspace=".",
                executable=sys.executable,
                env={"FAKE_MCP_MODE": "bad-contract"},
            )
            with pytest.raises(OmenCompatibilityError):
                await client._open()
        finally:
            for stack in stacks:
                stack.stop()
        assert len(recorder.created) == 1
        await _assert_fully_closed(recorder.created[0])

    _run(_scenario())


def test_failed_connect_bad_payload_cleans_up() -> None:
    async def _scenario() -> None:
        recorder = _Recorder()
        stacks = _patches("bad-payload", recorder)
        for stack in stacks:
            stack.start()
        try:
            with pytest.raises(OmenProtocolError):
                await AsyncOmen.connect(
                    ".",
                    executable=sys.executable,
                    env={"FAKE_MCP_MODE": "bad-payload"},
                )
        finally:
            for stack in stacks:
                stack.stop()
        assert len(recorder.created) == 1
        await _assert_fully_closed(recorder.created[0])

    _run(_scenario())


def test_failed_async_with_cleans_up() -> None:
    async def _scenario() -> None:
        recorder = _Recorder()
        stacks = _patches("bad-contract", recorder)
        for stack in stacks:
            stack.start()
        try:
            client = AsyncOmen(
                workspace=".",
                executable=sys.executable,
                env={"FAKE_MCP_MODE": "bad-contract"},
            )
            with pytest.raises(OmenCompatibilityError):
                async with client:
                    pass
        finally:
            for stack in stacks:
                stack.stop()
        assert len(recorder.created) == 1
        await _assert_fully_closed(recorder.created[0])

    _run(_scenario())


def test_failed_sync_connect_cleans_up_and_leaks_no_thread() -> None:
    recorder = _Recorder()
    before = {t.name for t in threading.enumerate() if t.is_alive()}
    stacks = _patches("bad-contract", recorder)
    for stack in stacks:
        stack.start()
    try:
        with pytest.raises(OmenCompatibilityError):
            Omen(
                workspace=".",
                executable=sys.executable,
                env={"FAKE_MCP_MODE": "bad-contract"},
            )
    finally:
        for stack in stacks:
            stack.stop()

    async def _check() -> None:
        assert len(recorder.created) == 1
        await _assert_fully_closed(recorder.created[0])

    _run(_check())
    after = {t.name for t in threading.enumerate() if t.is_alive()}
    assert "omen-shell-loop" not in (after - before)


def test_failed_reconnect_claims_no_session_and_cleans_up() -> None:
    async def _scenario() -> None:
        recorder = _Recorder()
        calls = {"n": 0}

        def _start_command(executable: str, workspace: str) -> list[str]:
            calls["n"] += 1
            if calls["n"] == 1:
                return [sys.executable, FAKE]
            return ["/no/such/omen-binary-xyz"]

        with (
            mock.patch("omen_shell.transport.start_command", _start_command),
            mock.patch("omen_shell.async_client.AsyncStdioTransport", recorder),
        ):
            client = await AsyncOmen.connect(
                ".",
                executable=sys.executable,
                env={"FAKE_MCP_MODE": "good-orient"},
            )
            assert len(recorder.created) == 1
            await client.close()
            # Second spawn fails: reconnect must raise, close the new
            # transport, and leave the client fail-closed (no new session).
            with pytest.raises(OmenNotFoundError):
                await client.reconnect()
            assert len(recorder.created) == 2
            for transport in recorder.created:
                proc = transport._process
                if proc is not None:
                    try:
                        await asyncio.wait_for(proc.wait(), timeout=10.0)
                    except TimeoutError as exc:
                        raise AssertionError("leaked child after failed reconnect") from exc
                assert transport.pending_count() == 0
            with pytest.raises(OmenConnectionClosedError):
                await client._rpc("ping")

    _run(_scenario())
