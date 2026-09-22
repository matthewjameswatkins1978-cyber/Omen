"""Async stdio JSON-RPC transport for ``omen mcp``.

One persistent subprocess per connection. Line-delimited JSON on
stdout, request registry keyed by ``id``, bounded timeouts everywhere,
bounded stderr diagnostics, deterministic shutdown (close stdin →
graceful wait → terminate → kill). No retries: a failed request is
reported, never replayed.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import shutil
import sys
import time
from dataclasses import dataclass, field

from . import protocol
from .errors import (
    OmenConnectionClosedError,
    OmenNotFoundError,
    OmenProtocolError,
    OmenRequestTimeout,
    OmenTransportError,
)
from .protocol import JsonObject

_log = logging.getLogger("omen_shell")
_TRACE_PAYLOADS = os.environ.get("OMEN_SHELL_TRACE_PAYLOADS") == "1"


def resolve_executable(explicit: str | None = None) -> str:
    """Resolve the Omen executable: explicit > OMEN_EXE > PATH."""
    if explicit:
        return explicit
    override = os.environ.get("OMEN_EXE")
    if override:
        return override
    found = shutil.which("omen")
    if found:
        return found
    raise OmenNotFoundError(
        "no Omen executable found: pass executable=, set OMEN_EXE, "
        'or install Omen so that "omen" is on PATH'
    )


def start_command(executable: str, workspace: str) -> list[str]:
    """Build the transport argv. Never a shell string; never shell=True."""
    return [executable, "mcp", "--workspace", workspace]


@dataclass
class TransportStats:
    requests_sent: int = 0
    responses_received: int = 0
    transport_errors: int = 0
    started_at: float = field(default_factory=time.monotonic)


class AsyncStdioTransport:
    """Persistent ``omen mcp`` subprocess speaking JSON-RPC 2.0 lines."""

    def __init__(
        self,
        executable: str,
        workspace: str,
        *,
        connect_timeout: float = protocol.DEFAULT_CONNECT_TIMEOUT,
        request_timeout: float = protocol.DEFAULT_REQUEST_TIMEOUT,
        shutdown_grace: float = protocol.DEFAULT_SHUTDOWN_GRACE,
        shutdown_hard: float = protocol.DEFAULT_SHUTDOWN_HARD,
        max_message_bytes: int = protocol.DEFAULT_MAX_MESSAGE_BYTES,
        env: dict[str, str] | None = None,
    ) -> None:
        self._executable = executable
        self._workspace = workspace
        self._connect_timeout = connect_timeout
        self._request_timeout = request_timeout
        self._shutdown_grace = shutdown_grace
        self._shutdown_hard = shutdown_hard
        self._max_message_bytes = max_message_bytes
        self._extra_env = dict(env) if env else {}
        self._process: asyncio.subprocess.Process | None = None
        self._reader_task: asyncio.Task[None] | None = None
        self._stderr_task: asyncio.Task[None] | None = None
        self._write_lock = asyncio.Lock()
        self._pending: dict[str, asyncio.Future[JsonObject]] = {}
        self._next_id = 0
        self._closed = False
        self._broken: OmenConnectionClosedError | None = None
        self._stderr_tail = bytearray()
        self.stats = TransportStats()

    @property
    def pid(self) -> int | None:
        proc = self._process
        return proc.pid if proc is not None else None

    @property
    def is_alive(self) -> bool:
        proc = self._process
        return proc is not None and proc.returncode is None

    @property
    def is_usable(self) -> bool:
        """True only when the process is alive AND no fatal error was recorded.

        ``returncode`` can lag reality (unkilled/reaped states); a recorded
        broken error means the transport already failed closed.
        """
        return self.is_alive and self._broken is None

    @property
    def closed(self) -> bool:
        return self._closed

    def _new_id(self) -> str:
        self._next_id += 1
        return str(self._next_id)

    async def start(self) -> None:
        """Spawn ``omen mcp`` and begin reading. Bounded by connect timeout."""
        if self._process is not None:
            raise OmenTransportError("transport already started")
        argv = start_command(self._executable, self._workspace)
        _log.debug("omen_shell: spawning %s (workspace=%s)", argv[0], self._workspace)
        environment = dict(os.environ)
        environment.update(self._extra_env)
        try:
            async with asyncio.timeout(self._connect_timeout):
                # StreamReader limit must cover max_message_bytes plus line
                # framing slack, or asyncio raises LimitOverrunError before
                # our explicit size check can fire.
                reader_limit = min(self._max_message_bytes + (1 << 20), 1 << 29)
                self._process = await asyncio.create_subprocess_exec(
                    *argv,
                    stdin=asyncio.subprocess.PIPE,
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE,
                    env=environment,
                    limit=reader_limit,
                )
        except FileNotFoundError as exc:
            raise OmenNotFoundError(f"Omen executable not runnable: {argv[0]}") from exc
        except TimeoutError as exc:
            raise OmenTransportError(
                f"timed out spawning omen mcp after {self._connect_timeout}s"
            ) from exc
        except OSError as exc:
            raise OmenTransportError(f"failed to spawn omen mcp: {exc}") from exc
        assert self._process.stdout is not None
        assert self._process.stderr is not None
        self._reader_task = asyncio.ensure_future(self._reader_loop())
        self._stderr_task = asyncio.ensure_future(self._stderr_loop())
        _log.debug("omen_shell: transport started pid=%s", self.pid)

    async def _stderr_loop(self) -> None:
        assert self._process is not None and self._process.stderr is not None
        try:
            async for chunk in self._process.stderr:
                self._stderr_tail += chunk
                if len(self._stderr_tail) > 8192:
                    del self._stderr_tail[: len(self._stderr_tail) - 8192]
        except (asyncio.CancelledError, ValueError):
            pass
        except Exception:  # noqa: BLE001 — stderr drain must never kill the client
            _log.debug("omen_shell: stderr drain ended", exc_info=True)

    def stderr_excerpt(self) -> str:
        return bytes(self._stderr_tail[-2048:]).decode("utf-8", "replace")

    async def _reader_loop(self) -> None:
        assert self._process is not None and self._process.stdout is not None
        try:
            while True:
                try:
                    line = await self._process.stdout.readline()
                except asyncio.LimitOverrunError as exc:
                    self._fail_all(
                        OmenProtocolError(
                            f"message exceeds {self._max_message_bytes} bytes; "
                            "increase max_message_bytes explicitly",
                        )
                    )
                    _log.debug("omen_shell: reader overrun: %s", exc)
                    break
                except (asyncio.CancelledError, ValueError):
                    break
                if not line:
                    break
                if len(line) > self._max_message_bytes:
                    self._fail_all(
                        OmenProtocolError(
                            f"message exceeds {self._max_message_bytes} bytes; "
                            "increase max_message_bytes explicitly",
                        )
                    )
                    break
                if not line.strip():
                    continue
                self._dispatch_line(line)
        except asyncio.CancelledError:
            pass
        except Exception as exc:  # noqa: BLE001 — reader death must fail closed
            self._fail_all(OmenConnectionClosedError(f"transport reader failed: {exc}"))
        finally:
            if not self._closed:
                self._fail_all(
                    OmenConnectionClosedError(
                        "omen mcp exited unexpectedly"
                        + (f"; stderr: {self.stderr_excerpt()}" if self._stderr_tail else "")
                    )
                )

    def _dispatch_line(self, line: bytes) -> None:
        try:
            message = json.loads(line.decode("utf-8"))
        except (UnicodeDecodeError, ValueError) as exc:
            _log.debug("omen_shell: discarding undecodable line: %s", exc)
            return
        if not isinstance(message, dict):
            return
        raw_id = message.get("id")
        if raw_id is None:
            # Server-initiated notification: Omen sends none; ignore safely.
            return
        key = str(raw_id)
        future = self._pending.pop(key, None)
        self.stats.responses_received += 1
        if future is None:
            _log.debug("omen_shell: response for unknown id %r; dropping", key)
            return
        if not future.done():
            future.set_result(message)

    def _fail_all(self, error: Exception) -> None:
        if isinstance(error, OmenConnectionClosedError) and self._broken is None:
            self._broken = error
        pending, self._pending = self._pending, {}
        for future in pending.values():
            if not future.done():
                future.set_exception(error)
        self.stats.transport_errors += 1

    def _check_usable(self) -> None:
        if self._closed:
            raise OmenConnectionClosedError("client is closed")
        if self._broken is not None:
            raise self._broken
        proc = self._process
        if proc is None or proc.returncode is not None:
            error = OmenConnectionClosedError(
                "omen mcp process is gone"
                + (f"; stderr: {self.stderr_excerpt()}" if self._stderr_tail else "")
            )
            self._broken = error
            raise error

    async def request(
        self,
        method: str,
        params: JsonObject | None = None,
        *,
        request_timeout: float | None = None,
    ) -> JsonObject:
        """Send one JSON-RPC request; await its response. Never retried."""
        self._check_usable()
        assert self._process is not None and self._process.stdin is not None
        call_id = self._new_id()
        envelope: JsonObject = {"jsonrpc": "2.0", "id": call_id, "method": method}
        if params is not None:
            envelope["params"] = params

        encoded = (json.dumps(envelope) + "\n").encode("utf-8")
        loop = asyncio.get_running_loop()
        future: asyncio.Future[JsonObject] = loop.create_future()
        self._pending[call_id] = future
        self.stats.requests_sent += 1
        if _TRACE_PAYLOADS:
            _log.warning("omen_shell UNSAFE trace: --> %s", envelope)
        else:
            _log.debug("omen_shell: --> id=%s method=%s", call_id, method)
        started = time.monotonic()
        try:
            async with self._write_lock:
                self._process.stdin.write(encoded)
                await self._process.stdin.drain()
        except (OSError, ValueError, ConnectionError) as exc:
            self._pending.pop(call_id, None)
            error = OmenConnectionClosedError(f"failed writing to omen mcp: {exc}")
            self._fail_all(error)
            raise error from exc
        timeout = self._request_timeout if request_timeout is None else request_timeout
        try:
            async with asyncio.timeout(timeout):
                message = await asyncio.shield(future)
        except TimeoutError as exc:
            self._pending.pop(call_id, None)
            raise OmenRequestTimeout(
                f"request {method!r} (id={call_id}) exceeded {timeout}s transport deadline"
            ) from exc
        except asyncio.CancelledError:
            # Python-side cancellation only: drop our waiter. A late Omen
            # response is discarded by id; NO claim about Omen-side state.
            self._pending.pop(call_id, None)
            if not future.done():
                future.cancel()
            raise
        elapsed_ms = (time.monotonic() - started) * 1000.0
        if _TRACE_PAYLOADS:
            _log.warning("omen_shell UNSAFE trace: <-- %s", message)
        else:
            _log.debug(
                "omen_shell: <-- id=%s method=%s %.1fms keys=%s",
                call_id,
                method,
                elapsed_ms,
                sorted(message.keys()),
            )
        return message

    async def notify(self, method: str, params: JsonObject | None = None) -> None:
        """Send a notification (no id, no response expected)."""
        proc = self._process
        if proc is None or self._closed or proc.returncode is not None:
            return
        assert proc.stdin is not None

        envelope: JsonObject = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            envelope["params"] = params
        try:
            async with self._write_lock:
                proc.stdin.write((json.dumps(envelope) + "\n").encode("utf-8"))
                await proc.stdin.drain()
        except (OSError, ValueError, ConnectionError):
            pass

    async def aclose(self) -> None:
        """Deterministic shutdown: stdin EOF → grace → terminate → kill."""
        if self._closed:
            return
        self._closed = True
        self._fail_all(OmenConnectionClosedError("client is closed"))
        proc = self._process
        if proc is None:
            await self._stop_tasks()
            return
        try:
            if proc.stdin is not None and proc.returncode is None:
                try:
                    proc.stdin.close()
                except (OSError, ValueError):
                    pass
            try:
                async with asyncio.timeout(self._shutdown_grace):
                    await proc.wait()
            except TimeoutError:
                if proc.returncode is None:
                    proc.terminate()
                    try:
                        async with asyncio.timeout(self._shutdown_hard):
                            await proc.wait()
                    except TimeoutError:
                        if proc.returncode is None:
                            proc.kill()
                            try:
                                await asyncio.wait_for(proc.wait(), timeout=5.0)
                            except (TimeoutError, OSError):
                                pass
        finally:
            await self._stop_tasks()
        _log.debug("omen_shell: transport closed pid=%s", self.pid)

    async def _stop_tasks(self) -> None:
        for task in (self._reader_task, self._stderr_task):
            if task is not None and not task.done():
                task.cancel()
        for task in (self._reader_task, self._stderr_task):
            if task is not None:
                try:
                    await task
                except (asyncio.CancelledError, Exception):  # noqa: BLE001, S110
                    pass
        self._reader_task = None
        self._stderr_task = None

    def pending_count(self) -> int:
        """Number of requests currently awaiting a response."""
        return len(self._pending)


def platform_name() -> str:
    if sys.platform.startswith("win"):
        return "windows"
    if sys.platform == "darwin":
        return "macos"
    return "linux"
