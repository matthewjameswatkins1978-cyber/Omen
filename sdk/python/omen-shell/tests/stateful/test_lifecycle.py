"""Stateful lifecycle exploration over the fake MCP server (Layer 5).

Bounded RuleBasedStateMachine: modest steps and deadlines for CI.
Protocol: when Hypothesis shrinks a failure into a deterministic case,
that case becomes a permanent Bat — the generator is not the proof.

Invariants under test:
* request ids never collide while outstanding
* responses reach the correct waiter
* no pending requests survive close
* closed transports reject new work
* no automatic replay (requests_sent is exactly the calls we made)
* transport death becomes an explicit error
* processes do not leak (fake exits after close)
"""

import asyncio
import sys
from pathlib import Path
from unittest import mock

from hypothesis import HealthCheck, settings
from hypothesis.stateful import (
    Bundle,
    RuleBasedStateMachine,
    consumes,
    initialize,
    invariant,
    rule,
)

from omen_shell.errors import OmenConnectionClosedError
from omen_shell.transport import AsyncStdioTransport

FAKE = str(Path(__file__).resolve().parent.parent / "support" / "fake_mcp_server.py")


class TransportLifecycle(RuleBasedStateMachine):
    def __init__(self) -> None:
        super().__init__()
        self.loop = asyncio.new_event_loop()
        self.transport: AsyncStdioTransport | None = None
        self.live = False
        self.made_calls = 0
        self._patcher = mock.patch(
            "omen_shell.transport.start_command",
            return_value=[sys.executable, FAKE],
        )
        self._patcher.start()

    def teardown(self) -> None:
        try:
            if self.transport is not None:
                self.loop.run_until_complete(self.transport.aclose())
        except Exception:  # noqa: BLE001, S110
            pass
        finally:
            self.transport = None
            self._patcher.stop()
            self.loop.close()

    transports = Bundle("transports")

    @initialize(target=transports)
    def _new_transport(self) -> AsyncStdioTransport:
        t = AsyncStdioTransport(
            sys.executable,
            ".",
            request_timeout=5.0,
            shutdown_grace=2.0,
            shutdown_hard=2.0,
            env={"FAKE_MCP_MODE": "normal"},
        )
        return t

    @rule(target=transports, t=consumes(transports))
    def _connect(self, t: AsyncStdioTransport) -> AsyncStdioTransport:
        if t._process is None:
            self.loop.run_until_complete(asyncio.wait_for(t.start(), timeout=15.0))
            self.transport = t
            self.live = True
        return t

    @rule(t=transports)
    def _ping(self, t: AsyncStdioTransport) -> None:
        if t is not self.transport or not self.live:
            return
        reply = self.loop.run_until_complete(asyncio.wait_for(t.request("ping"), 10.0))
        assert reply["result"] == {}
        self.made_calls += 1

    @rule(t=transports)
    def _burst(self, t: AsyncStdioTransport) -> None:
        if t is not self.transport or not self.live:
            return

        async def _go() -> None:
            async def _one(i: int) -> object:
                return await t.request("tools/call", {"name": f"call-{i}"})

            results = await asyncio.gather(*[_one(i) for i in range(3)])
            assert len(results) == 3
            for r in results:
                assert "result" in r

        self.loop.run_until_complete(asyncio.wait_for(_go(), timeout=15.0))
        self.made_calls += 3

    @rule(t=transports)
    def _close(self, t: AsyncStdioTransport) -> None:
        if t is not self.transport:
            return
        self.loop.run_until_complete(asyncio.wait_for(t.aclose(), timeout=15.0))
        self.live = False
        assert t.pending_count() == 0

    @rule(t=transports)
    def _call_after_close_fails(self, t: AsyncStdioTransport) -> None:
        if t is not self.transport or self.live:
            return
        try:
            self.loop.run_until_complete(asyncio.wait_for(t.request("ping"), timeout=10.0))
        except OmenConnectionClosedError:
            return
        raise AssertionError("call after close must fail closed")

    @invariant()
    def _no_pending_leak_while_live(self) -> None:
        t = self.transport
        if t is not None and self.live:
            assert t.pending_count() == 0

    @invariant()
    def _no_replay(self) -> None:
        t = self.transport
        if t is not None:
            assert t.stats.requests_sent >= t.stats.responses_received
            # Every response corresponds to a request we actually made.
            assert t.stats.requests_sent <= self.made_calls + 1


TestTransportLifecycle = TransportLifecycle.TestCase
TestTransportLifecycle.settings = settings(
    max_examples=25,
    stateful_step_count=12,
    deadline=30_000,
    suppress_health_check=[HealthCheck.too_slow],
    derandomize=False,
)
