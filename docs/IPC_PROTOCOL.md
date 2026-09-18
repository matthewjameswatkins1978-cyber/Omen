# Omen Local IPC Wire Protocol Specification

> **Protocol Identity**: `omen.local-ipc/1`  
> **Classification**: *Local-runtime framing and message envelopes for same-user shared execution and state distribution.*  
> **Hard Frame Ceiling**: `1,048,576 bytes (1 MiB)`

---

## 1. Framing Specification

Omen Local IPC streams are framed using an explicit 4-byte big-endian length prefix followed by a JSON payload:

```text
┌───────────────────────────┬──────────────────────────────────────────┐
│ Length Prefix (4 bytes)   │ Payload (N bytes)                        │
│ u32 big-endian            │ UTF-8 JSON Encoded Message               │
└───────────────────────────┴──────────────────────────────────────────┘
```

### Invariant Rules:
1. **Unambiguous Message Boundaries**: Framed streams guarantee message integrity across chunked writes and socket buffers. Newline parsing is prohibited for framing.
2. **Strict Maximum Frame Budget**: Maximum payload length is strictly capped at **1 MiB (1,048,576 bytes)**. Frames declaring lengths above this limit are rejected immediately with `FRAME_TOO_LARGE` and the connection is terminated.
3. **CAS Offloading for Large Data**: Output streams and command artifacts exceeding 8 KiB must be stored in Content-Addressed Storage (CAS) and referenced via `artifact://` URIs rather than bloat IPC frames.

---

## 2. Handshake Phase

Before exchanging requests, client and daemon must successfully complete a version negotiation handshake:

```text
Client                                             Daemon
  │                                                  │
  │─── ClientHello (supported_versions, client_id) ──>│
  │                                                  │
  │<── DaemonHello (selected_version, daemon_id) ────│
```

### 2.1 `ClientHello`
```json
{
  "protocol_version_family": "omen.local-ipc",
  "supported_versions": [1],
  "client_instance_id": "cli_01J8F2A3B4C5D6E7",
  "product_version": "0.4.0",
  "platform": "windows",
  "requested_features": ["events", "services", "execution"]
}
```

### 2.2 `DaemonHello`
```json
{
  "selected_protocol_version": 1,
  "daemon_instance_id": "dmn_01J8F2A9Z9Y8X7W6",
  "product_version": "0.4.0",
  "supported_features": ["events", "services", "execution"],
  "max_frame_size": 1048576
}
```

### Handshake Failure:
If `supported_versions` contains no protocol version supported by `omend`, the daemon responds with `PROTOCOL_VERSION_UNSUPPORTED` and closes the stream. No heuristic fallback or silent downgrade is permitted.

---

## 3. Message Envelopes

### 3.1 Request Envelope (`IpcRequest`)
```json
{
  "request_id": "req_01J8F2C1D2E3F4G5",
  "session_id": "sess_01J8F2C5A1B2C3D4",
  "workspace_id": "ws_omen_shell_main",
  "payload": {
    "kind": "AttachWorkspace",
    "canonical_path": "D:/Omen Shell"
  }
}
```

### 3.2 Response Envelope (`IpcResponse`)
```json
{
  "request_id": "req_01J8F2C1D2E3F4G5",
  "outcome": "Ok",
  "payload": {
    "kind": "WorkspaceAttached",
    "workspace_id": "ws_omen_shell_main",
    "epoch": 4
  }
}
```

Or on error:
```json
{
  "request_id": "req_01J8F2C1D2E3F4G5",
  "outcome": "Err",
  "error": {
    "code": "WORKSPACE_NOT_ATTACHED",
    "message": "Workspace path could not be verified or is inaccessible",
    "details": null
  }
}
```

### 3.3 Event Envelope (`IpcEvent`)
```json
{
  "workspace_id": "ws_omen_shell_main",
  "epoch": 4,
  "sequence": 142,
  "kind": "FactInvalidated",
  "payload": {
    "fact_id": "fact://test:suite",
    "resource_uri": "fact://test/status",
    "previous_validity": "CURRENT",
    "new_validity": "DIRTY",
    "cause": "fs:workspace mutation"
  }
}
```

---

## 4. Structured Error Vocabulary

Protocol errors are strongly typed:

| Error Code | Meaning |
|---|---|
| `PROTOCOL_VERSION_UNSUPPORTED` | Client and daemon do not share a supported protocol version |
| `FRAME_TOO_LARGE` | Declared frame size exceeds 1 MiB control budget |
| `MALFORMED_REQUEST` | Message payload could not be deserialized as JSON or lacks required fields |
| `LOCAL_PEER_DENIED` | Peer OS user identity does not match daemon owner |
| `WORKSPACE_NOT_ATTACHED` | Operation attempted on an unattached workspace |
| `SESSION_NOT_FOUND` | Session ID is unknown or expired |
| `RESYNC_REQUIRED` | Event sequence gap detected or watcher overflow occurred |
| `DAEMON_DEGRADED` | Daemon is undergoing restart, resynchronization, or recovery |
| `SERVICE_NOT_FOUND` | Requested service name does not exist in workspace |
| `SERVICE_ALREADY_RUNNING` | Cannot start service; a service with this name is already active |
| `EXECUTION_STATUS_UNKNOWN` | Execution status cannot be determined; auto-retry refused |
| `REQUEST_DUPLICATE` | Consequential request ID has already been accepted |
| `INTERNAL_RUNTIME_ERROR` | Internal daemon exception or SQLite error |

---

## 5. Core Request / Response Payloads

### 5.1 Ping / Pong
- **Request**: `{ "kind": "Ping", "timestamp_ms": 1726700000000 }`
- **Response**: `{ "kind": "Pong", "timestamp_ms": 1726700000002 }`

### 5.2 AttachWorkspace
- **Request**: `{ "kind": "AttachWorkspace", "canonical_path": "..." }`
- **Response**: `{ "kind": "WorkspaceAttached", "workspace_id": "...", "epoch": 1 }`

### 5.3 GetSnapshot
- **Request**: `{ "kind": "GetSnapshot" }`
- **Response**: `{ "kind": "Snapshot", "epoch": 1, "sequence": 42, "facts": [...], "services": [...], "tools": [...] }`

### 5.4 Execute
- **Request**: `{ "kind": "Execute", "request_id": "...", "contract": { ... } }`
- **Response**: `{ "kind": "ExecutionResult", "result": { ... } }`

### 5.5 Service Lifecycle
- **Start**: `{ "kind": "StartService", "name": "dev", "command": "npm run dev", "argv": [...] }`
- **Stop**: `{ "kind": "StopService", "name": "dev" }`
- **List**: `{ "kind": "ListServices" }`
- **Logs**: `{ "kind": "ServiceLogs", "name": "dev", "tail_lines": 50 }`
