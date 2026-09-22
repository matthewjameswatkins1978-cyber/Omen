"""Cancel projection tests (E2.5).

Invariant: the SDK is a truthful projection, not a second truth source.
``cancel_execution`` passes Omen's cancel record through opaquely
(execution_id / outcome / detail) and validates only the local argument.
All waits bounded; the fake server stands in for ``omen mcp``.
"""

import asyncio
import sys
from pathlib import Path
from typing import Any
from unittest import mock

import pytest

from omen_shell.async_client import AsyncOmen
from omen_shell.client import Omen
from omen_shell.errors import OmenProtocolError

FAKE = str(Path(__file__).resolve().parent.parent / "support" / "fake_mcp_server.py")


def _run(coro: Any) -> Any:
    return asyncio.run(coro)


def test_cancel_returns_record_unchanged() -> None:
    async def _scenario() -> None:
        with mock.patch(
            "omen_shell.transport.start_command",
            return_value=[sys.executable, FAKE],
        ):
            client = await AsyncOmen.connect(
                ".",
                executable=sys.executable,
                env={"FAKE_MCP_MODE": "good-orient"},
            )
            try:
                record = await client.cancel_execution("exec_fake_1")
            finally:
                await client.close()
        assert record["execution_id"] == "exec_fake_1"
        assert record["outcome"] == {"outcome": "TerminationConfirmed"}
        assert record["detail"] == "fake stop confirmed"

    _run(_scenario())


def test_cancel_sync_mirror_returns_record() -> None:
    with mock.patch(
        "omen_shell.transport.start_command",
        return_value=[sys.executable, FAKE],
    ):
        with Omen(
            ".",
            executable=sys.executable,
            env={"FAKE_MCP_MODE": "good-orient"},
        ) as omen:
            record = omen.cancel_execution("exec_fake_2")
    assert record["execution_id"] == "exec_fake_2"
    assert record["outcome"] == {"outcome": "TerminationConfirmed"}


def test_cancel_rejects_empty_id_locally() -> None:
    async def _scenario() -> None:
        with mock.patch(
            "omen_shell.transport.start_command",
            return_value=[sys.executable, FAKE],
        ):
            client = await AsyncOmen.connect(
                ".",
                executable=sys.executable,
                env={"FAKE_MCP_MODE": "good-orient"},
            )
            try:
                with pytest.raises(OmenProtocolError):
                    await client.cancel_execution("")
            finally:
                await client.close()

    _run(_scenario())
