"""Async Omen client: the semantic authority of omen-shell.

Everything here runs against one persistent ``omen mcp`` subprocess.
The synchronous :class:`Omen` façade delegates to this implementation,
so protocol interpretation exists exactly once.
"""

from __future__ import annotations

import json
import logging
import os
from collections.abc import Mapping, Sequence
from types import TracebackType
from typing import Any, cast

from . import protocol
from .compatibility import SUPPORTED_MACHINE_CONTRACTS, check_contract
from .errors import (
    OmenConnectionClosedError,
    OmenError,
    OmenProtocolError,
    OmenTransportError,
)
from .models import (
    ArtifactData,
    ArtifactMetadata,
    ArtifactRef,
    CapabilityInfo,
    ConnectionInfo,
    ExecutionResult,
    HistoryResult,
    LocalExecution,
    Orientation,
    SemanticResult,
)
from .protocol import JsonObject, JsonValue
from .transport import AsyncStdioTransport, platform_name, resolve_executable

_log = logging.getLogger("omen_shell")


def _parse_tool_text(result: JsonObject, *, tool: str) -> JsonValue:
    """Extract ``content[0].text`` as parsed JSON from a tools/call result."""
    content = result.get("content")
    if not isinstance(content, list) or not content:
        raise OmenProtocolError(f"tool {tool!r}: missing content array", raw=result)
    first = content[0]
    if not isinstance(first, dict) or first.get("type") != "text":
        raise OmenProtocolError(f"tool {tool!r}: content is not text", raw=result)
    text = first.get("text")
    if not isinstance(text, str):
        raise OmenProtocolError(f"tool {tool!r}: text is not a string", raw=result)
    try:
        return json.loads(text)
    except ValueError as exc:
        raise OmenProtocolError(f"tool {tool!r}: text is not JSON", raw=result) from exc


class AsyncRawAccessor:
    """Deliberately low-level MCP access: tools, resources, and ping.

    Forward access to Omen surfaces newer than this SDK release.
    Responses are parsed JSON ``text`` payloads plus the raw envelope.
    """

    def __init__(self, client: AsyncOmen) -> None:
        self._client = client

    async def list_tools(self) -> list[JsonObject]:
        """Return the raw tool descriptors from ``tools/list``."""
        result = await self._client._rpc("tools/list")
        tools = result.get("tools")
        if not isinstance(tools, list) or not all(isinstance(t, dict) for t in tools):
            raise OmenProtocolError("tools/list returned no tool array", raw=result)
        return [dict(t) for t in tools if isinstance(t, dict)]

    async def call_tool(
        self, name: str, arguments: Mapping[str, JsonValue] | None = None
    ) -> JsonValue:
        """Call any tool by name; return its parsed payload.

        Raises :class:`OmenError` on domain errors (``isError: true``),
        preserving the canonical envelope losslessly.
        """
        if not isinstance(name, str) or not name:
            raise OmenProtocolError("tool name must be a non-empty string")
        return await self._client._call_tool(name, dict(arguments) if arguments else {})

    async def list_resources(self) -> list[JsonObject]:
        """Return the raw resource descriptors from ``resources/list``."""
        result = await self._client._rpc("resources/list")
        resources = result.get("resources")
        if not isinstance(resources, list):
            raise OmenProtocolError("resources/list returned no array", raw=result)
        return [dict(r) for r in resources if isinstance(r, dict)]

    async def read_resource(self, uri: str) -> JsonObject:
        """Read a resource URI; return the raw ``resources/read`` result."""
        if not isinstance(uri, str) or not uri:
            raise OmenProtocolError("resource URI must be a non-empty string")
        return await self._client._rpc("resources/read", {"uri": uri})

    async def ping(self) -> None:
        """Protocol ping; raises on transport failure."""
        await self._client._rpc("ping")


class AsyncArtifacts:
    """CAS artifact access over the MCP resource surface."""

    def __init__(self, client: AsyncOmen) -> None:
        self._client = client

    @staticmethod
    def _ref(uri: str | ArtifactRef) -> ArtifactRef:
        return uri if isinstance(uri, ArtifactRef) else ArtifactRef.parse(uri)

    async def read(
        self,
        uri: str | ArtifactRef,
        *,
        offset: int = 0,
        length: int | None = None,
    ) -> ArtifactData:
        """Read artifact bytes (as text) for a URI unchanged from Omen.

        Omen truncates artifact reads server-side (64 KiB). ``offset``
        and ``length`` slice the payload Omen returned; they cannot
        recover bytes the server truncated. Negative values fail
        clearly instead of silently wrapping.
        """
        if offset < 0:
            raise OmenProtocolError("artifact offset must be >= 0")
        if length is not None and length < 0:
            raise OmenProtocolError("artifact length must be >= 0 or None")
        ref = self._ref(uri)
        result = await self._client.raw.read_resource(ref.uri)
        contents = result.get("contents")
        if not isinstance(contents, list) or not contents:
            raise OmenProtocolError("resources/read returned no contents", raw=result)
        first = contents[0]
        if not isinstance(first, dict):
            raise OmenProtocolError("resources/read content is not an object", raw=result)
        text = first.get("text", "")
        if not isinstance(text, str):
            raise OmenProtocolError("resources/read content has no text", raw=result)
        mime = first.get("mimeType", "text/plain")
        sliced = text[offset:] if length is None else text[offset : offset + length]
        return ArtifactData(
            uri=ref,
            text=sliced,
            mime_type=mime if isinstance(mime, str) else "text/plain",
            raw=result,
        )

    async def inspect(self, uri: str | ArtifactRef) -> ArtifactMetadata:
        """Return identity + served-payload metadata for an artifact URI."""
        ref = self._ref(uri)
        data = await self.read(ref)
        return ArtifactMetadata(uri=ref, mime_type=data.mime_type, byte_length=data.byte_length)


class AsyncSemantic:
    """Semantic truth with outcome/coverage preserved, never collapsed."""

    def __init__(self, client: AsyncOmen) -> None:
        self._client = client

    async def definition(
        self,
        symbol: str,
        *,
        file: str | None = None,
        line: int | None = None,
        col: int | None = None,
    ) -> SemanticResult[JsonValue]:
        """Resolve a symbol definition. ``NOT_FOUND`` is a result, not None."""
        if not isinstance(symbol, str) or not symbol:
            raise OmenProtocolError("semantic symbol must be a non-empty string")
        arguments: JsonObject = {"symbol": symbol}
        if file is not None:
            arguments["file"] = file
        if line is not None:
            arguments["line"] = line
        if col is not None:
            arguments["col"] = col
        payload = await self._client._call_tool("omen_symbol_definition", arguments)
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_symbol_definition returned no object", raw={})
        return SemanticResult.from_payload(payload)

    async def references(
        self,
        symbol: str,
        *,
        file: str | None = None,
        line: int | None = None,
        col: int | None = None,
        limit: int = 50,
    ) -> SemanticResult[JsonValue]:
        """Find references to a symbol."""
        if not isinstance(symbol, str) or not symbol:
            raise OmenProtocolError("semantic symbol must be a non-empty string")
        arguments: JsonObject = {"symbol": symbol, "limit": limit}
        if file is not None:
            arguments["file"] = file
        if line is not None:
            arguments["line"] = line
        if col is not None:
            arguments["col"] = col
        payload = await self._client._call_tool("omen_symbol_references", arguments)
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_symbol_references returned no object", raw={})
        return SemanticResult.from_payload(payload)

    async def search(self, query: str, *, limit: int = 50) -> SemanticResult[JsonValue]:
        """Search symbols by query string."""
        if not isinstance(query, str) or not query:
            raise OmenProtocolError("semantic query must be a non-empty string")
        payload = await self._client._call_tool(
            "omen_symbol_search", {"query": query, "limit": limit}
        )
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_symbol_search returned no object", raw={})
        return SemanticResult.from_payload(payload)


class AsyncActions:
    """Inert action surfaces. Planning only — there is no action run tool.

    The Python layer never weakens plan identity, expect-plan checking,
    authority, or freshness. Consequential execution stays in Omen's CLI
    (``action run --expect-plan``) under Omen's own policy.
    """

    def __init__(self, client: AsyncOmen) -> None:
        self._client = client

    async def list(self) -> JsonObject:
        """List available named actions."""
        payload = await self._client._call_tool("omen_action_list", {})
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_action_list returned no object", raw={})
        return payload

    async def show(self, action_id: str) -> JsonObject:
        """Show one action definition."""
        if not isinstance(action_id, str) or not action_id:
            raise OmenProtocolError("action_id must be a non-empty string")
        payload = await self._client._call_tool("omen_action_show", {"action_id": action_id})
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_action_show returned no object", raw={})
        return payload

    async def plan(self, action_id: str) -> JsonObject:
        """Compute a read-only action plan (no execution)."""
        if not isinstance(action_id, str) or not action_id:
            raise OmenProtocolError("action_id must be a non-empty string")
        payload = await self._client._call_tool("omen_action_plan", {"action_id": action_id})
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_action_plan returned no object", raw={})
        return payload


class AsyncHistory:
    """Durable history with the unjournaled distinction preserved."""

    def __init__(self, client: AsyncOmen) -> None:
        self._client = client

    async def query(
        self,
        *,
        limit: int = 20,
        all_sessions: bool = True,
        session_id: str | None = None,
    ) -> HistoryResult:
        """Query durable history. Never reports "nothing happened" lightly."""
        if not isinstance(limit, int) or isinstance(limit, bool) or not 1 <= limit <= 100:
            raise OmenProtocolError("history limit must be an integer in 1..100")
        arguments: JsonObject = {"limit": limit, "all_sessions": all_sessions}
        if session_id is not None:
            arguments["session_id"] = session_id
        payload = await self._client._call_tool("omen_history_query", arguments)
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_history_query returned no object", raw={})
        result = HistoryResult.from_payload(payload)
        return result.with_session(tuple(self._client._session_executions))


class AsyncFacts:
    """Fact registry reads."""

    def __init__(self, client: AsyncOmen) -> None:
        self._client = client

    async def query(self, *, filter: str = "all") -> list[JsonObject]:
        """Query facts; ``filter`` is one of all/current/dirty (else all)."""
        payload = await self._client._call_tool("omen_facts_query", {"filter": filter})
        if not isinstance(payload, list):
            raise OmenProtocolError("omen_facts_query returned no array", raw={})
        return [dict(e) for e in payload if isinstance(e, dict)]


class AsyncOmen:
    """Persistent async connection to Omen over ``omen mcp`` stdio.

    Usage::

        async with AsyncOmen(workspace=".") as omen:
            result = await omen.execute(["cargo", "test"])
    """

    def __init__(
        self,
        workspace: str | os.PathLike[str] = ".",
        *,
        executable: str | None = None,
        connect_timeout: float = protocol.DEFAULT_CONNECT_TIMEOUT,
        request_timeout: float = protocol.DEFAULT_REQUEST_TIMEOUT,
        shutdown_grace: float = protocol.DEFAULT_SHUTDOWN_GRACE,
        shutdown_hard: float = protocol.DEFAULT_SHUTDOWN_HARD,
        max_message_bytes: int = protocol.DEFAULT_MAX_MESSAGE_BYTES,
        env: dict[str, str] | None = None,
    ) -> None:
        self._workspace = os.fspath(workspace)
        if not isinstance(self._workspace, str) or not self._workspace:
            raise OmenProtocolError("workspace must be a non-empty path")
        self._executable = resolve_executable(executable)
        self._transport_kwargs: dict[str, Any] = {
            "connect_timeout": connect_timeout,
            "request_timeout": request_timeout,
            "shutdown_grace": shutdown_grace,
            "shutdown_hard": shutdown_hard,
            "max_message_bytes": max_message_bytes,
            "env": env,
        }
        self._transport = AsyncStdioTransport(
            self._executable,
            self._workspace,
            **self._transport_kwargs,
        )
        self._request_timeout = request_timeout
        self._orientation: Orientation | None = None
        self._server_version: str | None = None
        self._closed = False
        # Client-side session knowledge (NOT durable history): every
        # execution this client demonstrably performed, so history can
        # never claim "nothing happened" for our own work.
        self._session_executions: list[LocalExecution] = []
        self.raw = AsyncRawAccessor(self)
        self.artifacts = AsyncArtifacts(self)
        self.semantic = AsyncSemantic(self)
        self.actions = AsyncActions(self)
        self.history = AsyncHistory(self)
        self.facts = AsyncFacts(self)

    @classmethod
    async def connect(
        cls,
        workspace: str | os.PathLike[str] = ".",
        **kwargs: Any,
    ) -> AsyncOmen:
        """Connect explicitly; caller owns :meth:`close`."""
        self = cls(workspace, **kwargs)
        await self._open()
        return self

    async def _open(self) -> None:
        await self._transport.start()
        # MCP handshake: explicit protocol version, then optional
        # initialized notification (no reply is expected or required).
        init = await self._rpc(
            "initialize",
            {
                "protocolVersion": protocol.MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "omen-shell", "version": self.sdk_version()},
            },
        )
        server_info = init.get("serverInfo")
        if isinstance(server_info, dict):
            version = server_info.get("version")
            self._server_version = version if isinstance(version, str) else None
        await self._transport.notify("notifications/initialized", {})
        orientation = await self.orient()
        check_contract(orientation.contract_version, runtime_version=self._server_version)
        _log.debug(
            "omen_shell: connected omen=%s contract=%s",
            self._server_version,
            orientation.contract_version,
        )

    @staticmethod
    def sdk_version() -> str:
        """This package's version (importlib.metadata, source-tree aware)."""
        from importlib.metadata import PackageNotFoundError, version

        try:
            return version("omen-shell")
        except PackageNotFoundError:
            try:
                import tomllib
                from pathlib import Path

                pyproject = Path(__file__).resolve().parents[2] / "pyproject.toml"
                data = tomllib.loads(pyproject.read_text(encoding="utf-8"))
                return str(data["project"]["version"])
            except (OSError, ValueError, KeyError):
                return "0.0.0+unknown"

    async def _rpc(
        self,
        method: str,
        params: JsonObject | None = None,
        *,
        request_timeout: float | None = None,
    ) -> JsonObject:
        if self._closed:
            raise OmenConnectionClosedError("client is closed")
        message = await self._transport.request(
            method,
            params,
            request_timeout=request_timeout
            if request_timeout is not None
            else self._request_timeout,
        )
        if "error" in message and isinstance(message["error"], dict):
            error = message["error"]
            code = error.get("code")
            text = error.get("message", "")
            text_str = text if isinstance(text, str) else ""
            if code == -32004:
                # Omen serves resource failures (notably artifact reads) as
                # bare JSON-RPC errors: resources/read cannot carry the
                # isError envelope. Map losslessly to OmenError using
                # Omen's own message prefix; nothing is invented beyond the
                # required envelope fields, and the raw reply is preserved.
                if text_str.startswith("Artifact not found"):
                    domain_code = "ARTIFACT_NOT_FOUND"
                elif text_str.startswith("Resource not found"):
                    domain_code = "RESOURCE_NOT_FOUND"
                else:
                    domain_code = "RESOURCE_ERROR"
                raise OmenError(
                    code=domain_code,
                    message=text_str,
                    category="Evidence",
                    state_changed="NO",
                    retryability="UNKNOWN",
                    details={"jsonrpc_code": code, "method": method},
                    raw=message,
                )
            raise OmenProtocolError(
                f"JSON-RPC error {code}: {text_str} (method {method!r})", raw=message
            )
        result = message.get("result")
        if not isinstance(result, dict):
            raise OmenProtocolError(f"method {method!r} returned no result object", raw=message)
        return result

    async def _call_tool(
        self,
        name: str,
        arguments: JsonObject,
        *,
        request_timeout: float | None = None,
    ) -> JsonValue:
        message = await self._transport.request(
            "tools/call",
            {"name": name, "arguments": arguments},
            request_timeout=request_timeout
            if request_timeout is not None
            else self._request_timeout,
        )
        if self._closed:
            raise OmenConnectionClosedError("client is closed")
        if "error" in message and isinstance(message["error"], dict):
            error = message["error"]
            raise OmenProtocolError(
                f"JSON-RPC error {error.get('code')}: {error.get('message')} (tool {name!r})",
                raw=message,
            )
        result = message.get("result")
        if not isinstance(result, dict):
            raise OmenProtocolError(f"tool {name!r} returned no result", raw=message)
        if result.get("isError") is True:
            envelope = result.get("error")
            if not isinstance(envelope, dict):
                raise OmenProtocolError(
                    f"tool {name!r} failed without an error envelope", raw=result
                )
            raise OmenError.from_envelope(envelope)
        return _parse_tool_text(result, tool=name)

    # -- discovery ----------------------------------------------------

    async def orient(self) -> Orientation:
        """Return orientation truth (contract, version, digest, workspace)."""
        payload = await self._call_tool("omen_orient", {})
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_orient returned no object", raw={})
        orientation = Orientation.from_payload(payload)
        self._orientation = orientation
        return orientation

    async def capabilities(self, *, group: str | None = None) -> list[CapabilityInfo]:
        """List capabilities, optionally filtered by group."""
        arguments: JsonObject = {}
        if group is not None:
            arguments["group"] = group
        payload = await self._call_tool("omen_capabilities", arguments)
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_capabilities returned no object", raw={})
        entries = payload.get("capabilities", [])
        if not isinstance(entries, list):
            raise OmenProtocolError("omen_capabilities has no array", raw=payload)
        return [CapabilityInfo.from_payload(e) for e in entries if isinstance(e, dict)]

    async def describe(self, capability_id: str) -> JsonObject:
        """Describe one capability's operational schema and constraints."""
        if not isinstance(capability_id, str) or not capability_id:
            raise OmenProtocolError("capability_id must be a non-empty string")
        payload = await self._call_tool("omen_describe", {"capability_id": capability_id})
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_describe returned no object", raw={})
        return payload

    async def how(self, recipe_id: str) -> JsonObject:
        """Fetch an advisory recipe by id."""
        if not isinstance(recipe_id, str) or not recipe_id:
            raise OmenProtocolError("recipe_id must be a non-empty string")
        payload = await self._call_tool("omen_recipe", {"recipe_id": recipe_id})
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_recipe returned no object", raw={})
        return payload

    async def context(self, *, since: int | None = None) -> JsonObject:
        """Refresh dynamic workspace/session state, optionally as a delta."""
        arguments: JsonObject = {}
        if since is not None:
            if not isinstance(since, int) or isinstance(since, bool) or since < 0:
                raise OmenProtocolError("context since must be an integer >= 0")
            arguments["since"] = since
        payload = await self._call_tool("omen_context", arguments)
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_context returned no object", raw={})
        return payload

    async def workspace_status(self) -> JsonObject:
        """Return workspace status truth."""
        payload = await self._call_tool("omen_workspace_status", {})
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_workspace_status returned no object", raw={})
        return payload

    async def execution_status(self, request_id: str) -> JsonObject:
        """Poll a caller-owned consequential request id (not execution_id)."""
        if not isinstance(request_id, str) or not request_id:
            raise OmenProtocolError("request_id must be a non-empty string")
        payload = await self._call_tool("omen_execution_status", {"request_id": request_id})
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_execution_status returned no object", raw={})
        return payload

    # -- execution ----------------------------------------------------

    async def execute(
        self,
        argv: Sequence[str],
        *,
        # `timeout` is Omen's child-execution deadline, not asyncio's.
        timeout: float | None = None,  # noqa: ASYNC109
        request_timeout: float | None = None,
        cwd: str | None = None,
        tool: str = "exec",
        operation: str = "",
    ) -> ExecutionResult:
        """Execute an argv sequence under Omen authority.

        Only argv sequences are accepted: a single command string fails
        clearly (no shlex/shell splitting on the caller's behalf). A
        non-zero child exit returns an :class:`ExecutionResult` — it is
        truth, not an exception.

        Args:
            argv: Program + arguments, e.g. ``["cargo", "test"]``.
            timeout: Omen child-execution deadline in seconds.
            request_timeout: Python transport deadline in seconds.
            cwd: Working directory override (server defaults to workspace).
            tool: Execution tool name (server default ``"exec"``).
            operation: Operation label for evidence (server default ``""``).
        """
        if isinstance(argv, str) or not isinstance(argv, Sequence):
            raise OmenProtocolError(
                "execute() requires an argv sequence (e.g. ['cargo', 'test']); "
                "a single command string is refused — Omen is argv-only"
            )
        items = list(argv)
        if not items or not all(isinstance(a, str) and a for a in items):
            raise OmenProtocolError("argv must be a non-empty sequence of non-empty strings")
        arguments: JsonObject = {
            "argv": cast(JsonValue, items),
            "tool": tool,
            "operation": operation,
        }
        if timeout is not None:
            if timeout <= 0:
                raise OmenProtocolError("execute timeout must be positive")
            arguments["timeout_ms"] = int(timeout * 1000)
        if cwd is not None:
            arguments["cwd"] = cwd
        if request_timeout is not None and request_timeout <= 0:
            raise OmenProtocolError("request_timeout must be positive")
        payload = await self._call_tool("omen_execute", arguments, request_timeout=request_timeout)
        if not isinstance(payload, dict):
            raise OmenProtocolError("omen_execute returned no object", raw={})
        result = ExecutionResult.from_payload(payload)
        self._session_executions.append(
            LocalExecution(
                execution_id=result.execution_id,
                command=" ".join(items),
                runtime_status=result.runtime_status,
                exit_code=result.exit_code,
                stdout_artifact=result.stdout_artifact,
                stderr_artifact=result.stderr_artifact,
            )
        )
        return result

    # -- lifecycle ----------------------------------------------------

    @property
    def connection_info(self) -> ConnectionInfo:
        """Proven facts about this connection (never claimed unmeasured)."""
        orientation = self._orientation
        return ConnectionInfo(
            sdk_version=self.sdk_version(),
            omen_version=self._server_version or "unknown",
            contract_version=orientation.contract_version if orientation else "unknown",
            contract_digest=orientation.contract_digest if orientation else "",
            platform=platform_name(),
            workspace=self._workspace,
            transport="stdio: omen mcp",
            pid=self._transport.pid,
        )

    @property
    def supported_contracts(self) -> frozenset[str]:
        """Machine Contracts this SDK release accepts."""
        return SUPPORTED_MACHINE_CONTRACTS

    async def reconnect(self) -> None:
        """Explicitly start a new session after close or transport death.

        Never automatic: the caller decides. Pending state from the old
        session is gone; the new handshake re-establishes truth.
        """
        if self._transport.is_usable and not self._closed:
            return
        await self._transport.aclose()
        self._transport = AsyncStdioTransport(
            self._executable, self._workspace, **self._transport_kwargs
        )
        self._orientation = None
        self._server_version = None
        self._closed = False
        await self._open()

    async def close(self) -> None:
        """Close cleanly. Idempotent; no threads, tasks, or processes remain."""
        if self._closed:
            return
        self._closed = True
        try:
            await self._transport.aclose()
        except OmenTransportError as exc:
            raise OmenTransportError(f"error during close: {exc}") from exc

    async def __aenter__(self) -> AsyncOmen:
        await self._open()
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        await self.close()

    def __repr__(self) -> str:
        state = "closed" if self._closed else ("alive" if self._transport.is_alive else "new")
        return f"AsyncOmen(workspace={self._workspace!r}, state={state})"
