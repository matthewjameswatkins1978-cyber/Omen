"""Synchronous ``Omen`` client: a thin façade over :class:`AsyncOmen`.

The async implementation is the semantic authority; this module owns no
protocol interpretation. One private event-loop thread per client runs
the single :class:`AsyncOmen` and its persistent ``omen mcp`` process,
so the sync API keeps working inside notebooks and async-hosted
environments (no ``asyncio.run()`` per call, no new process per call).
"""

from __future__ import annotations

import asyncio
import concurrent.futures
import os
import threading
from collections.abc import Sequence
from types import TracebackType
from typing import Any

from . import protocol
from .async_client import AsyncOmen
from .errors import OmenConnectionClosedError, OmenTransportError
from .models import ConnectionInfo, ExecutionResult, Orientation
from .protocol import JsonObject


class _LoopThread:
    """A single event loop parked on a daemon thread."""

    def __init__(self) -> None:
        self._ready = threading.Event()
        self._loop: asyncio.AbstractEventLoop | None = None
        self._thread = threading.Thread(target=self._run, name="omen-shell-loop", daemon=True)
        self._thread.start()
        if not self._ready.wait(timeout=30.0):
            raise OmenTransportError("timed out starting omen-shell loop thread")

    def _run(self) -> None:
        loop = asyncio.new_event_loop()
        asyncio.set_event_loop(loop)
        self._loop = loop
        self._ready.set()
        try:
            loop.run_forever()
        finally:
            try:
                loop.run_until_complete(loop.shutdown_asyncgens())
            except (RuntimeError, asyncio.CancelledError):
                pass
            asyncio.set_event_loop(None)
            loop.close()

    @property
    def loop(self) -> asyncio.AbstractEventLoop:
        assert self._loop is not None
        return self._loop

    def submit(self, coro: Any, *, timeout: float | None = None) -> Any:
        future: concurrent.futures.Future[Any] = asyncio.run_coroutine_threadsafe(coro, self.loop)
        return future.result(timeout=timeout)

    def stop(self, timeout: float = 10.0) -> None:
        loop = self._loop
        if loop is not None:
            loop.call_soon_threadsafe(loop.stop)
        self._thread.join(timeout=timeout)


class _SyncNamespace:
    """Bind a sync client method set to one async namespace object."""

    def __init__(self, client: Omen, namespace: str) -> None:
        object.__setattr__(self, "_client", client)
        object.__setattr__(self, "_namespace", namespace)

    def __getattr__(self, name: str) -> Any:
        client = object.__getattribute__(self, "_client")
        namespace = object.__getattribute__(self, "_namespace")
        target = getattr(getattr(client._async, namespace), name)
        if not callable(target):
            return target

        def _call(*args: Any, **kwargs: Any) -> Any:
            return client._submit(target(*args, **kwargs))

        _call.__name__ = name
        _call.__doc__ = getattr(target, "__doc__", None)
        return _call


class Omen:
    """Persistent sync connection to Omen.

    Usage::

        with Omen(workspace=".") as omen:
            result = omen.execute(["cargo", "test"])
    """

    def __init__(
        self,
        workspace: str | os.PathLike[str] = ".",
        **kwargs: Any,
    ) -> None:
        request_timeout = kwargs.get("request_timeout", protocol.DEFAULT_REQUEST_TIMEOUT)
        try:
            self._call_timeout = float(request_timeout) + 30.0
        except (TypeError, ValueError):
            self._call_timeout = protocol.DEFAULT_REQUEST_TIMEOUT + 30.0
        self._loop_thread = _LoopThread()
        self._closed_flag = False
        try:
            self._async: AsyncOmen = self._loop_thread.submit(
                AsyncOmen.connect(workspace, **kwargs),
                timeout=self._call_timeout,
            )
        except Exception:
            self._loop_thread.stop()
            raise
        self.raw = _SyncNamespace(self, "raw")
        self.artifacts = _SyncNamespace(self, "artifacts")
        self.semantic = _SyncNamespace(self, "semantic")
        self.actions = _SyncNamespace(self, "actions")
        self.history = _SyncNamespace(self, "history")
        self.facts = _SyncNamespace(self, "facts")

    @classmethod
    def connect(
        cls,
        workspace: str | os.PathLike[str] = ".",
        **kwargs: Any,
    ) -> Omen:
        """Connect explicitly; caller owns :meth:`close`."""
        return cls(workspace, **kwargs)

    def _submit(self, coro: Any, *, timeout: float | None = None) -> Any:
        if getattr(self, "_closed_flag", False):
            # The coroutine was already constructed by the call expression;
            # close it so nothing is left un-awaited, then reject the call.
            close = getattr(coro, "close", None)
            if callable(close):
                try:
                    close()
                except RuntimeError:
                    pass
            raise OmenConnectionClosedError("client is closed")
        try:
            return self._loop_thread.submit(
                coro, timeout=self._call_timeout if timeout is None else timeout
            )
        except concurrent.futures.TimeoutError as exc:
            raise OmenTransportError("sync call exceeded local deadline") from exc

    def orient(self) -> Orientation:
        """Return orientation truth (contract, version, digest, workspace)."""
        return self._submit(self._async.orient())

    def capabilities(self, *, group: str | None = None) -> list[Any]:
        """List capabilities, optionally filtered by group."""
        return self._submit(self._async.capabilities(group=group))

    def describe(self, capability_id: str) -> JsonObject:
        """Describe one capability's operational schema and constraints."""
        return self._submit(self._async.describe(capability_id))

    def how(self, recipe_id: str) -> JsonObject:
        """Fetch an advisory recipe by id."""
        return self._submit(self._async.how(recipe_id))

    def context(self, *, since: int | None = None) -> JsonObject:
        """Refresh dynamic workspace/session state."""
        return self._submit(self._async.context(since=since))

    def workspace_status(self) -> JsonObject:
        """Return workspace status truth."""
        return self._submit(self._async.workspace_status())

    def execution_status(self, request_id: str) -> JsonObject:
        """Poll a caller-owned consequential request id."""
        return self._submit(self._async.execution_status(request_id))

    def execute(
        self,
        argv: Sequence[str],
        *,
        timeout: float | None = None,
        request_timeout: float | None = None,
        cwd: str | None = None,
        tool: str = "exec",
        operation: str = "",
    ) -> ExecutionResult:
        """Execute an argv sequence. Child non-zero returns truth, not error."""
        call_timeout = (
            self._call_timeout if request_timeout is None else float(request_timeout) + 30.0
        )
        return self._submit(
            self._async.execute(
                argv,
                timeout=timeout,
                request_timeout=request_timeout,
                cwd=cwd,
                tool=tool,
                operation=operation,
            ),
            timeout=call_timeout,
        )

    @property
    def connection_info(self) -> ConnectionInfo:
        """Proven facts about this connection (measured at handshake)."""
        return self._async.connection_info

    @property
    def supported_contracts(self) -> frozenset[str]:
        """Machine Contracts this SDK release accepts."""
        return self._async.supported_contracts

    def reconnect(self) -> None:
        """Explicitly start a new session. Never automatic."""
        self._submit(self._async.reconnect())

    def close(self) -> None:
        """Close cleanly. Idempotent; the loop thread is joined."""
        if getattr(self, "_closed_flag", False):
            return
        self._closed_flag = True
        try:
            self._loop_thread.submit(self._async.close(), timeout=30.0)
        except Exception:  # noqa: S110 — close must not raise; errors already logged
            pass
        finally:
            self._loop_thread.stop()

    def __enter__(self) -> Omen:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        self.close()

    def __repr__(self) -> str:
        return f"Omen(workspace={self._async._workspace!r})"
