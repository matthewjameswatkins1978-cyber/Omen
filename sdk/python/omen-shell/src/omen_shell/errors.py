"""Error hierarchy for omen-shell.

Two disjoint families:

* SDK-layer errors (:class:`OmenShellError` and subclasses) — the Python
  transport, handshake, compatibility gate, or lifecycle failed. Omen
  itself may never have been reached.
* Omen-domain errors (:class:`OmenError`) — Omen answered with its
  canonical machine error envelope. Every field is preserved losslessly.

Critical rule: ``retryability`` is information, never an instruction.
The SDK never retries a consequential operation automatically.
"""

from __future__ import annotations

from typing import Any

from .protocol import JsonObject, JsonValue


class OmenShellError(Exception):
    """Base for every error raised by the omen-shell package itself."""


class OmenNotFoundError(OmenShellError):
    """No Omen executable could be resolved.

    Resolution order: explicit ``executable=`` argument, then the
    ``OMEN_EXE`` environment variable, then ``shutil.which("omen")``.
    Remediation: install Omen and ensure ``omen`` is on ``PATH``, or
    pass ``executable=`` / set ``OMEN_EXE``.
    """


class OmenTransportError(OmenShellError):
    """The stdio transport to ``omen mcp`` failed.

    Covers spawn failure, broken pipes, and unreadable framing.
    """


class OmenProtocolError(OmenShellError):
    """Omen spoke JSON-RPC the SDK could not interpret.

    Covers JSON-RPC ``error`` replies, malformed frames, unknown
    message shapes, and oversized messages. The raw reply (or frame
    excerpt) is preserved on :attr:`raw` for diagnosis.
    """

    def __init__(self, message: str, *, raw: JsonValue | None = None) -> None:
        super().__init__(message)
        self.raw = raw


class OmenCompatibilityError(OmenShellError):
    """Omen answered with a Machine Contract the SDK does not support."""

    def __init__(
        self,
        message: str,
        *,
        found_contract: str | None,
        supported_contracts: frozenset[str],
        runtime_version: str | None,
    ) -> None:
        super().__init__(message)
        self.found_contract = found_contract
        self.supported_contracts = supported_contracts
        self.runtime_version = runtime_version


class OmenConnectionClosedError(OmenShellError):
    """The ``omen mcp`` process is gone; the client is closed or broken.

    The client fails closed: pending requests are failed, new calls are
    rejected, and no replacement process is silently spawned. Use an
    explicit reconnect to start a new session.
    """


class OmenRequestTimeout(OmenShellError):
    """A request exceeded the SDK transport deadline.

    This is a Python-side deadline, distinct from Omen's own execution
    ``timeout``. A timed-out execution may still complete inside Omen;
    the SDK makes no claim about the Omen-side outcome.
    """


class OmenProcessError(OmenShellError):
    """Ergonomic wrapper for a child process that exited non-zero.

    Only ever raised by ``ExecutionResult.raise_for_status()`` — never
    by ``execute()`` itself, which returns the result as truth. Carries
    the full :class:`ExecutionResult` on :attr:`result`.
    """

    def __init__(self, message: str, *, result: Any) -> None:
        super().__init__(message)
        self.result = result


class OmenError(OmenShellError):
    """Python representation of Omen's canonical machine error envelope.

    Field-for-field mapping of Omen's ``OmenError`` (schema_version 1):
    ``code``, ``message``, ``category``, ``state_changed``,
    ``retryability``, ``evidence``, ``details``. Unknown future enum
    values are preserved verbatim — never normalized into known ones.

    A child process exiting non-zero is NOT an ``OmenError``: Omen
    successfully performed that execution, and the caller receives an
    :class:`ExecutionResult` instead.
    """

    def __init__(
        self,
        *,
        code: str,
        message: str,
        category: str = "Unknown",
        state_changed: str = "UNKNOWN",
        retryability: str = "UNKNOWN",
        evidence: JsonObject | None = None,
        details: JsonObject | None = None,
        schema_version: int = 1,
        raw: JsonObject | None = None,
    ) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message
        self.category = category
        self.state_changed = state_changed
        self.retryability = retryability
        self.evidence: JsonObject = evidence if evidence is not None else {}
        self.details: JsonObject = details if details is not None else {}
        self.schema_version = schema_version
        self.raw = raw if raw is not None else {}

    @classmethod
    def from_envelope(cls, envelope: JsonObject) -> OmenError:
        """Build losslessly from an ``error`` object in a tools/call result."""

        def _str(key: str, default: str) -> str:
            value = envelope.get(key, default)
            return value if isinstance(value, str) else default

        def _obj(key: str) -> JsonObject:
            value = envelope.get(key, {})
            return dict(value) if isinstance(value, dict) else {}

        schema_version = envelope.get("schema_version", 1)
        return cls(
            code=_str("code", "UNKNOWN"),
            message=_str("message", ""),
            category=_str("category", "Unknown"),
            state_changed=_str("state_changed", "UNKNOWN"),
            retryability=_str("retryability", "UNKNOWN"),
            evidence=_obj("evidence"),
            details=_obj("details"),
            schema_version=schema_version if isinstance(schema_version, int) else 1,
            raw=dict(envelope),
        )

    def to_dict(self) -> JsonObject:
        """Return the envelope in Omen's own shape (no second schema)."""
        return {
            "schema_version": self.schema_version,
            "code": self.code,
            "message": self.message,
            "category": self.category,
            "state_changed": self.state_changed,
            "retryability": self.retryability,
            "evidence": dict(self.evidence),
            "details": dict(self.details),
        }
