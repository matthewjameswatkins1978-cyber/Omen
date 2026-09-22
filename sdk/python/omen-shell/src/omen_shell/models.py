"""Public model objects for omen-shell.

Ordinary frozen dataclasses with slots — no runtime validation
dependency. Unknown/additive fields from Omen are preserved on
``extra`` so forward-compatible responses survive parsing; required
consequential fields are validated and missing ones raise
:class:`OmenProtocolError`. Unknown future enum values stay unknown:
they are carried as plain strings, never mapped to fake known values.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import ClassVar, Generic, NewType, TypeVar

from .errors import OmenProtocolError
from .protocol import JsonObject, JsonValue

ExecutionId = NewType("ExecutionId", str)

T = TypeVar("T")


def _require_str(payload: JsonObject, key: str, *, what: str) -> str:
    value = payload.get(key)
    if not isinstance(value, str):
        raise OmenProtocolError(f"{what} missing required string field {key!r}", raw=payload)
    return value


def _optional_str(payload: JsonObject, key: str) -> str | None:
    value = payload.get(key)
    if value is None:
        return None
    if not isinstance(value, str):
        raise OmenProtocolError(f"field {key!r} must be a string or null", raw=payload)
    return value


def _optional_int(payload: JsonObject, key: str) -> int | None:
    value = payload.get(key)
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, int):
        raise OmenProtocolError(f"field {key!r} must be an integer or null", raw=payload)
    return value


def _split_extra(payload: JsonObject, known: frozenset[str]) -> dict[str, JsonValue]:
    return {k: v for k, v in payload.items() if k not in known}


@dataclass(frozen=True, slots=True)
class ArtifactRef:
    """Canonical reference to a CAS artifact: ``artifact://sha256/<digest>``."""

    uri: str
    algorithm: str = "sha256"
    digest: str = ""

    @classmethod
    def parse(cls, uri: str) -> ArtifactRef:
        if not isinstance(uri, str) or not uri.startswith("artifact://"):
            raise OmenProtocolError(f"not an artifact URI: {uri!r}")
        rest = uri[len("artifact://") :]
        algorithm, sep, digest = rest.partition("/")
        if not sep or not algorithm or not digest:
            raise OmenProtocolError(f"malformed artifact URI: {uri!r}")
        return cls(uri=uri, algorithm=algorithm, digest=digest)

    def __str__(self) -> str:
        return self.uri


@dataclass(frozen=True, slots=True)
class ExecutionResult:
    """Truth about one completed Omen execution.

    ``ok`` is deterministic: runtime completed AND child exit == 0.
    A non-zero child exit is truth carried here, never an exception.
    """

    execution_id: ExecutionId
    runtime_status: str
    exit_code: int | None
    duration_ms: int | None = None
    stdout_preview: str = ""
    stderr_preview: str = ""
    stdout_artifact: ArtifactRef | None = None
    stderr_artifact: ArtifactRef | None = None
    raw: JsonObject = field(default_factory=dict, compare=False)
    extra: dict[str, JsonValue] = field(default_factory=dict, compare=False)

    _KNOWN: ClassVar[frozenset[str]] = frozenset(
        {
            "execution_id",
            "runtime_status",
            "exit_code",
            "duration_ms",
            "stdout_preview",
            "stderr_preview",
            "stdout_artifact",
            "stderr_artifact",
        }
    )

    @property
    def ok(self) -> bool:
        """True only when the runtime completed AND the child exited 0."""
        return self.runtime_status == "COMPLETED" and self.exit_code == 0

    def raise_for_status(self) -> ExecutionResult:
        """Return self when :attr:`ok`, else raise :class:`OmenProcessError`.

        Raises:
            OmenProcessError: carrying this result on ``.result``.
        """
        if self.ok:
            return self
        from .errors import OmenProcessError

        raise OmenProcessError(
            f"child process failed: status={self.runtime_status} "
            f"exit={self.exit_code} execution={self.execution_id}",
            result=self,
        )

    @classmethod
    def from_payload(cls, payload: JsonObject) -> ExecutionResult:
        execution_id = ExecutionId(_require_str(payload, "execution_id", what="execute result"))
        runtime_status = _require_str(payload, "runtime_status", what="execute result")
        exit_code = _optional_int(payload, "exit_code")
        duration_ms = _optional_int(payload, "duration_ms")
        stdout_preview = payload.get("stdout_preview", "")
        stderr_preview = payload.get("stderr_preview", "")
        if not isinstance(stdout_preview, str) or not isinstance(stderr_preview, str):
            raise OmenProtocolError("execute previews must be strings", raw=payload)
        stdout_uri = _optional_str(payload, "stdout_artifact")
        stderr_uri = _optional_str(payload, "stderr_artifact")
        return cls(
            execution_id=execution_id,
            runtime_status=runtime_status,
            exit_code=exit_code,
            duration_ms=duration_ms,
            stdout_preview=stdout_preview,
            stderr_preview=stderr_preview,
            stdout_artifact=ArtifactRef.parse(stdout_uri) if stdout_uri else None,
            stderr_artifact=ArtifactRef.parse(stderr_uri) if stderr_uri else None,
            raw=dict(payload),
            extra=_split_extra(payload, cls._KNOWN),
        )

    def to_dict(self) -> JsonObject:
        """Faithful dict form (Omen's shape; ``extra`` merged back)."""
        base: JsonObject = {
            "execution_id": str(self.execution_id),
            "runtime_status": self.runtime_status,
            "exit_code": self.exit_code,
            "duration_ms": self.duration_ms,
            "stdout_preview": self.stdout_preview,
            "stderr_preview": self.stderr_preview,
            "stdout_artifact": str(self.stdout_artifact) if self.stdout_artifact else None,
            "stderr_artifact": str(self.stderr_artifact) if self.stderr_artifact else None,
        }
        base.update(self.extra)
        return base

    def __repr__(self) -> str:
        return (
            f"ExecutionResult(execution_id={self.execution_id!r}, "
            f"runtime_status={self.runtime_status!r}, exit_code={self.exit_code!r}, "
            f"ok={self.ok!r})"
        )


@dataclass(frozen=True, slots=True)
class HistoryEntry:
    """One durable history entry."""

    sequence: int | None
    execution_id: str
    session_id: str
    command: str
    status: str
    recorded_at: str
    duration_ms: int | None = None
    state_changed: str | None = None
    evidence: tuple[str, ...] = ()
    raw: JsonObject = field(default_factory=dict, compare=False)

    @classmethod
    def from_payload(cls, payload: JsonObject) -> HistoryEntry:
        sequence = payload.get("sequence")
        if sequence is not None and (isinstance(sequence, bool) or not isinstance(sequence, int)):
            raise OmenProtocolError("history entry sequence must be an integer", raw=payload)
        evidence = payload.get("evidence", [])
        if not isinstance(evidence, list):
            raise OmenProtocolError("history entry evidence must be a string list", raw=payload)
        evidence_items = tuple(e for e in evidence if isinstance(e, str))
        if len(evidence_items) != len(evidence):
            raise OmenProtocolError("history entry evidence must be a string list", raw=payload)
        return cls(
            sequence=sequence,
            execution_id=_require_str(payload, "execution_id", what="history entry"),
            session_id=str(payload.get("session_id", "")),
            command=str(payload.get("command", "")),
            status=str(payload.get("status", "UNKNOWN")),
            recorded_at=str(payload.get("recorded_at", "")),
            duration_ms=_optional_int(payload, "duration_ms"),
            state_changed=_optional_str(payload, "state_changed"),
            evidence=evidence_items,
            raw=dict(payload),
        )


@dataclass(frozen=True, slots=True)
class LocalExecution:
    """Client-side session knowledge of one execution this client performed.

    This is NOT durable history — Omen remains the sole authority for
    durable truth. It exists so the SDK never reports "nothing happened"
    for work it demonstrably performed when Omen's durable history has
    no record (standalone executions are not journaled by Omen; see
    docs). Session-scoped: gone when the client closes.
    """

    execution_id: ExecutionId
    command: str
    runtime_status: str
    exit_code: int | None = None
    stdout_artifact: ArtifactRef | None = None
    stderr_artifact: ArtifactRef | None = None

    def __repr__(self) -> str:
        return (
            f"LocalExecution(execution_id={self.execution_id!r}, "
            f"runtime_status={self.runtime_status!r}, exit_code={self.exit_code!r})"
        )


@dataclass(frozen=True, slots=True)
class HistoryResult:
    """Durable history plus the standalone-vs-durable distinction.

    When a local CLI execution exists that the daemon never journaled,
    ``history_status`` is ``UNJOURNALED_LOCAL_EXECUTION`` and
    ``local_execution`` carries the marker — the client must not report
    "nothing happened".
    """

    entries: tuple[HistoryEntry, ...] = ()
    history_status: str = "CURRENT"
    local_execution: JsonObject | None = None
    session_executions: tuple[LocalExecution, ...] = ()
    raw: JsonObject = field(default_factory=dict, compare=False)

    @classmethod
    def from_payload(cls, payload: JsonObject) -> HistoryResult:
        if "history" in payload and isinstance(payload["history"], dict):
            inner = payload["history"]
            status = payload.get("history_status", "UNJOURNALED_LOCAL_EXECUTION")
            local = payload.get("local_execution")
            history_status = status if isinstance(status, str) else "UNJOURNALED_LOCAL_EXECUTION"
            local_execution = dict(local) if isinstance(local, dict) else None
        else:
            inner = payload
            history_status = "CURRENT"
            local_execution = None
        entries_raw = inner.get("entries", [])
        if not isinstance(entries_raw, list):
            raise OmenProtocolError("history entries must be a list", raw=payload)
        entries = tuple(HistoryEntry.from_payload(e) for e in entries_raw if isinstance(e, dict))
        return cls(
            entries=entries,
            history_status=history_status,
            local_execution=local_execution,
            raw=dict(payload),
        )

    def with_session(self, executions: tuple[LocalExecution, ...]) -> HistoryResult:
        """Attach client-side session knowledge (Omen truth untouched)."""
        from dataclasses import replace

        return replace(self, session_executions=executions)

    def knows(self, execution_id: str) -> bool:
        """True when Omen's durable entries or this session knows the execution.

        An UNJOURNALED marker (``local_execution is not None``) is
        reported as-is on the result; callers must not read it as
        "nothing happened".
        """
        if any(e.execution_id == execution_id for e in self.entries):
            return True
        return any(e.execution_id == execution_id for e in self.session_executions)

    def __repr__(self) -> str:
        return f"HistoryResult(entries={len(self.entries)}, history_status={self.history_status!r})"


@dataclass(frozen=True, slots=True)
class SemanticResult(Generic[T]):
    """Omen semantic truth: outcome and coverage are never collapsed.

    ``outcome`` is one of ``FOUND`` / ``NOT_FOUND`` / ``AMBIGUOUS``
    (or an unknown future value, preserved verbatim). ``coverage`` is
    one of ``COMPLETE`` / ``PARTIAL`` / ``NONE``. A ``NOT_FOUND`` is a
    result, not ``None`` and not an exception.
    """

    operation: str
    outcome: str
    coverage: str
    data: T
    generation: JsonObject = field(default_factory=dict, compare=False)
    raw: JsonObject = field(default_factory=dict, compare=False)
    extra: dict[str, JsonValue] = field(default_factory=dict, compare=False)

    _KNOWN: ClassVar[frozenset[str]] = frozenset(
        {"schema_version", "operation", "outcome", "coverage", "generation", "data"}
    )

    @property
    def found(self) -> bool:
        """True only when ``outcome == "FOUND"``."""
        return self.outcome == "FOUND"

    @classmethod
    def from_payload(cls, payload: JsonObject) -> SemanticResult[JsonValue]:
        data: JsonValue = payload.get("data")
        generation_raw = payload.get("generation")
        generation: JsonObject = dict(generation_raw) if isinstance(generation_raw, dict) else {}
        return SemanticResult[JsonValue](
            operation=str(payload.get("operation", "")),
            outcome=str(payload.get("outcome", "UNKNOWN")),
            coverage=str(payload.get("coverage", "UNKNOWN")),
            data=data,
            generation=generation,
            raw=dict(payload),
            extra=_split_extra(payload, cls._KNOWN),
        )

    def __repr__(self) -> str:
        return (
            f"SemanticResult(operation={self.operation!r}, outcome={self.outcome!r}, "
            f"coverage={self.coverage!r})"
        )


@dataclass(frozen=True, slots=True)
class CapabilityInfo:
    """One catalogue entry from capability discovery."""

    id: str
    group: str
    summary: str
    availability: str
    raw: JsonObject = field(default_factory=dict, compare=False)

    @classmethod
    def from_payload(cls, payload: JsonObject) -> CapabilityInfo:
        return cls(
            id=_require_str(payload, "id", what="capability"),
            group=str(payload.get("group", "")),
            summary=str(payload.get("summary", "")),
            availability=str(payload.get("availability", "")),
            raw=dict(payload),
        )


@dataclass(frozen=True, slots=True)
class Orientation:
    """Connection orientation truth from ``omen_orient``."""

    contract_version: str
    omen_version: str
    contract_digest: str
    workspace: JsonObject = field(default_factory=dict, compare=False)
    raw: JsonObject = field(default_factory=dict, compare=False)
    extra: dict[str, JsonValue] = field(default_factory=dict, compare=False)

    _KNOWN: ClassVar[frozenset[str]] = frozenset(
        {"contract_version", "omen_version", "contract_digest", "workspace"}
    )

    @classmethod
    def from_payload(cls, payload: JsonObject) -> Orientation:
        workspace = payload.get("workspace", {})
        return cls(
            contract_version=_require_str(payload, "contract_version", what="orient"),
            omen_version=_require_str(payload, "omen_version", what="orient"),
            contract_digest=str(payload.get("contract_digest", "")),
            workspace=dict(workspace) if isinstance(workspace, dict) else {},
            raw=dict(payload),
            extra=_split_extra(payload, cls._KNOWN),
        )


@dataclass(frozen=True, slots=True)
class ArtifactData:
    """Bytes (as text) retrieved for an artifact URI.

    Omen truncates artifact reads server-side (64 KiB); the SDK reports
    what Omen returned and never claims completeness it cannot prove.
    Use :attr:`byte_length` for the returned payload size.
    """

    uri: ArtifactRef
    text: str
    mime_type: str = "text/plain"
    raw: JsonObject = field(default_factory=dict, compare=False)

    @property
    def byte_length(self) -> int:
        """Length of the returned payload in bytes (UTF-8)."""
        return len(self.text.encode("utf-8"))

    def __repr__(self) -> str:
        return f"ArtifactData(uri={self.uri.uri!r}, bytes={self.byte_length})"


@dataclass(frozen=True, slots=True)
class ArtifactMetadata:
    """Identity + served-payload metadata for an artifact URI."""

    uri: ArtifactRef
    mime_type: str = "text/plain"
    byte_length: int = 0

    def __repr__(self) -> str:
        return f"ArtifactMetadata(uri={self.uri.uri!r}, bytes={self.byte_length})"


@dataclass(frozen=True, slots=True)
class ConnectionInfo:
    """Proven facts about this connection (never claimed, always measured)."""

    sdk_version: str
    omen_version: str
    contract_version: str
    contract_digest: str
    platform: str
    workspace: str
    transport: str
    pid: int | None = None
