"""Omen machine-result JSON types and the MCP protocol constants Omen speaks.

Omen's stdio transport is newline-delimited JSON-RPC 2.0. Only the
``params``/``result`` shapes documented in docs/python/MCP-SURFACE.md
are relied upon here.
"""

from __future__ import annotations

JsonScalar = None | bool | int | float | str
JsonValue = JsonScalar | list["JsonValue"] | dict[str, "JsonValue"]
JsonObject = dict[str, JsonValue]

# Protocol version offered at handshake. Must be one Omen accepts;
# Omen negotiates unknown well-formed versions to its latest, but we
# offer the latest known-good version explicitly.
MCP_PROTOCOL_VERSION = "2025-11-25"

# JSON-RPC error codes we translate (never invented by the SDK).
PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
INTERNAL_JSONRPC_ERROR = -32603

# Default transport bounds (seconds). Every external wait is bounded.
DEFAULT_CONNECT_TIMEOUT = 30.0
DEFAULT_REQUEST_TIMEOUT = 120.0
DEFAULT_SHUTDOWN_GRACE = 5.0
DEFAULT_SHUTDOWN_HARD = 5.0

# Largest single JSON-RPC line accepted before failing explicitly.
# Omen results are bounded by design; this is a backstop, not a target.
DEFAULT_MAX_MESSAGE_BYTES = 16 * 1024 * 1024

# `omen_execute` server-side default when the caller omits `timeout`.
OMEN_DEFAULT_EXEC_TIMEOUT_S = 60.0
