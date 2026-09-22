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
from collections.abc import Coroutine, Mapping, Sequence
from types import TracebackType
from typing import Any, TypeVar, cast

from . import protocol
from .async_client import AsyncOmen
from .errors import OmenConnectionClosedError, OmenTransportError
from .models import (
    ArtifactData,
    ArtifactMetadata,
    ArtifactRef,
    ConnectionInfo,
    ExecutionResult,
    HistoryResult,
    Orientation,
    SemanticResult,
)
from .protocol import JsonObject, JsonValue

_T = TypeVar("_T")


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


class SyncRawAccessor:
    """Typed sync delegation to the async raw accessor. No semantics here."""

    def __init__(self, client: Omen) -> None:
        self._client = client

    def list_tools(self) -> list[JsonObject]:
        """Return the raw tool descriptors from ``tools/list``."""
        return self._client._submit(self._client._async.raw.list_tools())

    def call_tool(self, name: str, arguments: Mapping[str, JsonValue] | None = None) -> JsonValue:
        """Call any tool by name; return its parsed payload."""
        return self._client._submit(self._client._async.raw.call_tool(name, arguments))

    def list_resources(self) -> list[JsonObject]:
        """Return the raw resource descriptors from ``resources/list``."""
        return self._client._submit(self._client._async.raw.list_resources())

    def read_resource(self, uri: str) -> JsonObject:
        """Read a resource URI; return the raw ``resources/read`` result."""
        return self._client._submit(self._client._async.raw.read_resource(uri))

    def ping(self) -> None:
        """Protocol ping; raises on transport failure."""
        self._client._submit(self._client._async.raw.ping())


class SyncArtifacts:
    """Typed sync delegation to the async artifact namespace."""

    def __init__(self, client: Omen) -> None:
        self._client = client

    def read(
        self,
        uri: str | ArtifactRef,
        *,
        offset: int = 0,
        length: int | None = None,
    ) -> ArtifactData:
        """Read artifact bytes (as text) for a URI unchanged from Omen."""
        return self._client._submit(
            self._client._async.artifacts.read(uri, offset=offset, length=length)
        )

    def inspect(self, uri: str | ArtifactRef) -> ArtifactMetadata:
        """Return identity + served-payload metadata for an artifact URI."""
        return self._client._submit(self._client._async.artifacts.inspect(uri))


class SyncSemantic:
    """Typed sync delegation to the async semantic namespace."""

    def __init__(self, client: Omen) -> None:
        self._client = client

    def definition(
        self,
        symbol: str,
        *,
        file: str | None = None,
        line: int | None = None,
        col: int | None = None,
    ) -> SemanticResult[JsonValue]:
        """Resolve a symbol definition. ``NOT_FOUND`` is a result, not None."""
        return self._client._submit(
            self._client._async.semantic.definition(symbol, file=file, line=line, col=col)
        )

    def references(
        self,
        symbol: str,
        *,
        file: str | None = None,
        line: int | None = None,
        col: int | None = None,
        limit: int = 50,
    ) -> SemanticResult[JsonValue]:
        """Find references to a symbol."""
        return self._client._submit(
            self._client._async.semantic.references(
                symbol, file=file, line=line, col=col, limit=limit
            )
        )

    def search(self, query: str, *, limit: int = 50) -> SemanticResult[JsonValue]:
        """Search symbols by query string."""
        return self._client._submit(self._client._async.semantic.search(query, limit=limit))


class SyncActions:
    """Typed sync delegation to the async action namespace (planning only)."""

    def __init__(self, client: Omen) -> None:
        self._client = client

    def list(self) -> JsonObject:
        """List available named actions."""
        return self._client._submit(self._client._async.actions.list())

    def show(self, action_id: str) -> JsonObject:
        """Show one action definition."""
        return self._client._submit(self._client._async.actions.show(action_id))

    def plan(self, action_id: str) -> JsonObject:
        """Compute a read-only action plan (no execution)."""
        return self._client._submit(self._client._async.actions.plan(action_id))


class SyncHistory:
    """Typed sync delegation to the async history namespace."""

    def __init__(self, client: Omen) -> None:
        self._client = client

    def query(
        self,
        *,
        limit: int = 20,
        all_sessions: bool = True,
        session_id: str | None = None,
    ) -> HistoryResult:
        """Query durable history. Never reports "nothing happened" lightly."""
        return self._client._submit(
            self._client._async.history.query(
                limit=limit, all_sessions=all_sessions, session_id=session_id
            )
        )


class SyncFacts:
    """Typed sync delegation to the async fact namespace."""

    def __init__(self, client: Omen) -> None:
        self._client = client

    def query(self, *, filter: str = "all") -> list[JsonObject]:
        """Query facts; ``filter`` is one of all/current/dirty (else all)."""
        return self._client._submit(self._client._async.facts.query(filter=filter))


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
        # Retain the in-flight connect future: if our local deadline fires
        # (or the caller interrupts) while open is still running on the loop
        # thread, cancelling it drives _open's deterministic cleanup there.
        # The failed connect itself already closed its transport (Repair 1
        # invariant); here we only stop the thread afterwards.
        connect_future: concurrent.futures.Future[AsyncOmen] = asyncio.run_coroutine_threadsafe(
            AsyncOmen.connect(workspace, **kwargs), self._loop_thread.loop
        )
        try:
            self._async: AsyncOmen = connect_future.result(timeout=self._call_timeout)
        except BaseException:
            connect_future.cancel()
            try:
                connect_future.result(timeout=10.0)
            except BaseException:  # noqa: S110 — original failure propagates below
                pass
            self._loop_thread.stop()
            raise
        self.raw = SyncRawAccessor(self)
        self.artifacts = SyncArtifacts(self)
        self.semantic = SyncSemantic(self)
        self.actions = SyncActions(self)
        self.history = SyncHistory(self)
        self.facts = SyncFacts(self)

    @classmethod
    def connect(
        cls,
        workspace: str | os.PathLike[str] = ".",
        **kwargs: Any,
    ) -> Omen:
        """Connect explicitly; caller owns :meth:`close`."""
        return cls(workspace, **kwargs)

    def _submit(self, coro: Coroutine[Any, Any, _T], *, timeout: float | None = None) -> _T:
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
            return cast(
                _T,
                self._loop_thread.submit(
                    coro, timeout=self._call_timeout if timeout is None else timeout
                ),
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

    def cancel_execution(self, execution_id: str) -> JsonObject:
        """Request cancellation by canonical execution_id (intent != proof)."""
        return self._submit(self._async.cancel_execution(execution_id))

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
