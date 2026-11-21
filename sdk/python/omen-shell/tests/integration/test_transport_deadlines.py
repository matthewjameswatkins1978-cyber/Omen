"""Bound the complete stdio transaction, not just the response wait.

Uses the existing fake MCP subprocess plus controlled contention. Every
scenario has an outer watchdog independent of the transport's deadline.
"""

import asyncio
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

from omen_shell.errors import OmenProtocolError, OmenRequestTimeout
from omen_shell.transport import AsyncStdioTransport

FAKE = str(Path(__file__).resolve().parent.parent / "support" / "fake_mcp_server.py")


class TransportDeadlineTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        self.transport = AsyncStdioTransport(
            sys.executable,
            ".",
            request_timeout=0.15,
            shutdown_grace=0.5,
            shutdown_hard=0.5,
            env={"FAKE_MCP_MODE": "normal"},
        )
        self.addAsyncCleanup(self.transport.aclose)
        with patch("omen_shell.transport.start_command", return_value=[sys.executable, FAKE]):
            await asyncio.wait_for(self.transport.start(), 3)

    async def assert_recovered(self) -> None:
        self.assertEqual(self.transport.pending_count(), 0)
        reply = await asyncio.wait_for(self.transport.request("ping", request_timeout=2), 3)
        self.assertEqual(reply["result"], {})
        self.assertEqual(self.transport.pending_count(), 0)

    async def test_request_deadline_includes_write_lock(self) -> None:
        async with self.transport._write_lock:
            with self.assertRaisesRegex(OmenRequestTimeout, "write-lock"):
                await asyncio.wait_for(self.transport.request("ping"), 2)
        self.assertEqual(self.transport.stats.requests_sent, 0)
        await self.assert_recovered()

    async def test_cancel_while_waiting_for_write_lock_removes_pending(self) -> None:
        async with self.transport._write_lock:
            task = asyncio.create_task(self.transport.request("ping", request_timeout=2))
            await asyncio.sleep(0)  # Let the request reach the already-owned lock.
            self.assertEqual(self.transport.pending_count(), 1)
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await asyncio.wait_for(task, 2)
            self.assertEqual(self.transport.pending_count(), 0)
        await self.assert_recovered()

    async def test_request_deadline_includes_backpressured_drain(self) -> None:
        proc = self.transport._process
        assert proc is not None and proc.stdin is not None
        never = asyncio.Event()
        with patch.object(proc.stdin, "drain", side_effect=never.wait):
            with self.assertRaisesRegex(OmenRequestTimeout, "write-drain"):
                await asyncio.wait_for(self.transport.request("ping"), 2)
        await self.assert_recovered()

    async def test_cancel_during_drain_releases_pending_and_lock(self) -> None:
        proc = self.transport._process
        assert proc is not None and proc.stdin is not None
        entered = asyncio.Event()
        never = asyncio.Event()

        async def drain() -> None:
            entered.set()
            await never.wait()

        with patch.object(proc.stdin, "drain", side_effect=drain):
            task = asyncio.create_task(self.transport.request("ping", request_timeout=2))
            await asyncio.wait_for(entered.wait(), 2)
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await asyncio.wait_for(task, 2)
        await self.assert_recovered()

    async def test_notification_deadline_includes_write_lock(self) -> None:
        async with self.transport._write_lock:
            with self.assertRaisesRegex(OmenRequestTimeout, "write-lock"):
                await asyncio.wait_for(self.transport.notify("notifications/initialized"), 2)
        await self.assert_recovered()

    async def test_real_pipe_backpressure_has_a_deadline(self) -> None:
        # Real child process deliberately does not consume stdin. The frame
        # is larger than the OS pipe and asyncio's write-buffer high water mark.
        blocked = AsyncStdioTransport(
            sys.executable,
            ".",
            request_timeout=0.15,
            shutdown_grace=0.2,
            shutdown_hard=0.5,
        )
        self.addAsyncCleanup(blocked.aclose)
        with patch(
            "omen_shell.transport.start_command",
            return_value=[sys.executable, "-c", "import time; time.sleep(30)"],
        ):
            await asyncio.wait_for(blocked.start(), 3)
        with self.assertRaisesRegex(OmenRequestTimeout, "write-drain"):
            await asyncio.wait_for(blocked.request("ping", {"payload": "x" * (2 << 20)}), 3)
        self.assertEqual(blocked.pending_count(), 0)
        await asyncio.wait_for(blocked.aclose(), 3)
        self.assertFalse(blocked.is_alive)

    async def start_small_transport(self, mode: str) -> AsyncStdioTransport:
        transport = AsyncStdioTransport(
            sys.executable,
            ".",
            request_timeout=2,
            shutdown_grace=0.2,
            shutdown_hard=0.5,
            max_message_bytes=1024,
            env={"FAKE_MCP_MODE": mode},
        )
        self.addAsyncCleanup(transport.aclose)
        with patch("omen_shell.transport.start_command", return_value=[sys.executable, FAKE]):
            await asyncio.wait_for(transport.start(), 3)
        return transport

    async def test_stderr_flood_without_newline_does_not_block_rpc(self) -> None:
        transport = await self.start_small_transport("stderr-flood")
        reply = await asyncio.wait_for(transport.request("ping"), 3)
        self.assertEqual(reply["result"], {})
        # stdout and stderr have independent scheduling. Wait for real EOF
        # before checking the final diagnostic tail, not for a guessed sleep.
        proc = transport._process
        assert proc is not None and proc.stdin is not None
        proc.stdin.close()
        await asyncio.wait_for(proc.wait(), 2)
        assert transport._stderr_task is not None
        await asyncio.wait_for(transport._stderr_task, 2)
        self.assertTrue(transport.stderr_excerpt().endswith("END-OF-STDERR"))
        self.assertLessEqual(len(transport._stderr_tail), 8192)

    async def test_configured_stdout_limit_reports_protocol_error(self) -> None:
        transport = await self.start_small_transport("large")
        with self.assertRaisesRegex(OmenProtocolError, "message exceeds 1024 bytes"):
            await asyncio.wait_for(transport.request("tools/call", {"name": "x"}), 3)
        self.assertEqual(transport.pending_count(), 0)


if __name__ == "__main__":
    unittest.main()
