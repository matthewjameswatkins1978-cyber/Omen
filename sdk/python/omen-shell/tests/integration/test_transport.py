"""Transport integration tests against the fake MCP server (Layer 2).

No real Omen required. Each test runs exactly one event loop; every
wait is bounded by explicit transport timeouts.
"""

import asyncio
import sys
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import Any
from unittest import mock

import pytest

from omen_shell.errors import (
    OmenConnectionClosedError,
    OmenProtocolError,
    OmenRequestTimeout,
)
from omen_shell.transport import AsyncStdioTransport

FAKE = str(Path(__file__).resolve().parent.parent / "support" / "fake_mcp_server.py")


def _run_scenario(
    mode: str,
    scenario: Callable[[AsyncStdioTransport], Awaitable[None]],
    **kwargs: Any,
) -> None:
    async def _main() -> None:
        transport = AsyncStdioTransport(
            sys.executable,
            ".",
            request_timeout=float(kwargs.get("request_timeout", 10.0)),
            shutdown_grace=2.0,
            shutdown_hard=2.0,
            env={"FAKE_MCP_MODE": mode},
        )
        with mock.patch(
            "omen_shell.transport.start_command",
            return_value=[sys.executable, FAKE],
        ):
            await transport.start()
        try:
            await scenario(transport)
        finally:
            await transport.aclose()
        assert transport.pending_count() == 0

    asyncio.run(_main())


def test_normal_request_response() -> None:
    async def _scenario(t: AsyncStdioTransport) -> None:
        reply = await t.request("tools/list")
        assert [x["name"] for x in reply["result"]["tools"]] == ["fake_echo"]
        assert t.stats.requests_sent == 1

    _run_scenario("normal", _scenario)


def test_out_of_order_correlation() -> None:
    async def _scenario(t: AsyncStdioTransport) -> None:
        first = asyncio.ensure_future(t.request("tools/call", {"name": "a"}))
        await asyncio.sleep(0.5)
        second = await t.request("tools/call", {"name": "b"})
        first_result = await asyncio.wait_for(first, timeout=10.0)
        assert "result" in first_result
        assert "result" in second
        assert t.stats.responses_received >= 2

    _run_scenario("out-of-order", _scenario)


def test_malformed_line_skipped() -> None:
    async def _scenario(t: AsyncStdioTransport) -> None:
        assert (await t.request("ping"))["result"] == {}

    _run_scenario("malformed", _scenario)


def test_unknown_id_dropped() -> None:
    async def _scenario(t: AsyncStdioTransport) -> None:
        assert (await t.request("ping"))["result"] == {}

    _run_scenario("unknown-id", _scenario)


def test_exit_early_fails_closed() -> None:
    async def _scenario(t: AsyncStdioTransport) -> None:
        with pytest.raises(OmenConnectionClosedError):
            await t.request("ping")
        with pytest.raises(OmenConnectionClosedError):
            await t.request("ping")

    _run_scenario("exit-early", _scenario)


def test_stderr_noise_does_not_break_framing() -> None:
    async def _scenario(t: AsyncStdioTransport) -> None:
        assert (await t.request("ping"))["result"] == {}
        assert "fake setup noise" in t.stderr_excerpt()

    _run_scenario("stderr-noise", _scenario)


def test_request_timeout_is_explicit() -> None:
    async def _scenario(t: AsyncStdioTransport) -> None:
        with pytest.raises(OmenRequestTimeout):
            await t.request("tools/call", {"name": "x"})

    _run_scenario("slow", _scenario, request_timeout=1.0)


def test_oversize_message_fails() -> None:
    async def _scenario(t: AsyncStdioTransport) -> None:
        t._max_message_bytes = 1024
        with pytest.raises(OmenProtocolError):
            await t.request("tools/call", {"name": "x"})

    _run_scenario("large", _scenario)


def test_duplicate_response_second_dropped() -> None:
    async def _scenario(t: AsyncStdioTransport) -> None:
        reply = await t.request("tools/call", {"name": "x"})
        assert "result" in reply
        await asyncio.sleep(0.5)
        assert t.pending_count() == 0

    _run_scenario("duplicate", _scenario)


def test_large_payload_within_limit() -> None:
    async def _scenario(t: AsyncStdioTransport) -> None:
        reply = await t.request("tools/call", {"name": "x"})
        assert "result" in reply

    _run_scenario("large", _scenario)


def test_double_close_ok() -> None:
    async def _scenario(t: AsyncStdioTransport) -> None:
        await t.aclose()
        await t.aclose()
        assert t.closed

    _run_scenario("normal", _scenario)


def test_extra_env_is_preserved() -> None:
    t = AsyncStdioTransport(
        sys.executable,
        ".",
        env={"OMEN_STATE_HOME": "fake-state", "FAKE_MCP_MODE": "normal"},
    )
    assert t._extra_env["OMEN_STATE_HOME"] == "fake-state"
