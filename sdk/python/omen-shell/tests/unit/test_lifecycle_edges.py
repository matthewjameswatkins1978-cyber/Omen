"""Lifecycle edge proofs: spawn failure, pre-start use, close paths (Layer 1/2).

Fast, bounded, no real Omen required.
"""

import asyncio
import threading

import pytest

from omen_shell.errors import (
    OmenConnectionClosedError,
    OmenNotFoundError,
)
from omen_shell.transport import AsyncStdioTransport


def _run(coro):  # type: ignore[no-untyped-def]
    return asyncio.run(coro)


def test_spawn_missing_executable_raises_not_found() -> None:
    async def _scenario() -> None:
        t = AsyncStdioTransport("/no/such/omen-binary-xyz", ".", connect_timeout=5.0)
        with pytest.raises(OmenNotFoundError):
            await t.start()
        await t.aclose()

    _run(_scenario())


def test_request_before_start_rejected() -> None:
    async def _scenario() -> None:
        t = AsyncStdioTransport("/no/such/omen-binary-xyz", ".")
        with pytest.raises(OmenConnectionClosedError):
            await t.request("ping")
        await t.aclose()

    _run(_scenario())


def test_notify_before_start_is_quiet() -> None:
    async def _scenario() -> None:
        t = AsyncStdioTransport("/no/such/omen-binary-xyz", ".")
        await t.notify("notifications/initialized", {})
        await t.aclose()

    _run(_scenario())


def test_aclose_before_start_ok() -> None:
    async def _scenario() -> None:
        t = AsyncStdioTransport("/no/such/omen-binary-xyz", ".")
        await t.aclose()
        assert t.closed

    _run(_scenario())


def test_sync_client_bad_executable_no_thread_leak() -> None:
    from omen_shell import Omen

    before = {t.name for t in threading.enumerate()}
    with pytest.raises(OmenNotFoundError):
        Omen(workspace=".", executable="/no/such/omen-binary-xyz")
    after = {t.name for t in threading.enumerate() if t.is_alive()}
    assert "omen-shell-loop" not in (after - before)


def test_sync_double_close_quiet() -> None:
    from omen_shell.testing import OmenHarness
    from tests.conftest import omen_exe

    if omen_exe is None:
        pytest.skip("no Omen executable")
    harness = OmenHarness()
    harness.open()
    harness.close()
    harness.close()
