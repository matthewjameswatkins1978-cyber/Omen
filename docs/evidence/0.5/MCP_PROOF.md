# Omen 0.5 — Model Context Protocol (MCP) Proof

## 1. Overview

External coding agents (Claude Desktop, Cursor, Gemini Code Assist, custom agent harnesses) communicate with Omen via the Model Context Protocol (MCP) standard (`2024-11-05`).

Omen provides:
- Binary: `omen-mcp` (or `omen mcp`)
- Transport: Standard input/output (stdio) and asynchronous streaming duplexes (`run_stream`).
- Format: JSON-RPC 2.0.

---

## 2. Supported Tools & Capabilities

| Tool Name | Description |
|---|---|
| `omen_workspace_status` | Returns structured snapshot: workspace path, facts count, dirty count, managed services count, available tools. |
| `omen_facts_query` | Queries published facts with optional validity filter (`all`, `current`, `dirty`). |
| `omen_execute` | Submits an execution request through the shared daemon broker (or local supervisor fallback if standalone). Generates CAS artifacts for all output. |
| `omen_execution_status` | Polls background execution request status and completion receipts. |
| `omen_history_query` | Queries subordinate physical execution history (session-scoped or all sessions). |
| `omen_services_list` | Lists managed background daemon services. |
| `omen_services_control` | Starts, stops, or restarts managed services. |
| `omen_capabilities_discover`| Explains physical runtime capabilities, platform constraints, boundaries, and doctrine. |

### Supported Resource Schemes
- `artifact://sha256/<digest>`: Content-Addressed Storage reading with bounded slicing (at most 64 KiB per read).
- `fact://<resource>`: Logical fact retrieval.
- `proc://<name>`: Managed service state inspection.

---

## 3. Proof D: End-to-End External Agent Workflow

1. **Attachment & Handshake**:
   - Client sends `initialize` with `protocolVersion: "2024-11-05"`.
   - Server returns capabilities (tools, resources, server info `omen-mcp 0.2.0`).
2. **Tools Discovery**:
   - Client calls `tools/list`.
   - Server returns 8 canonical Omen tools with typed JSON schemas.
3. **Capabilities Discovery**:
   - Client invokes `omen_capabilities_discover`.
   - Server returns doctrine `"substrate, not sovereign"` and architectural boundaries (Lantern, Resolve, Tethers, Omen, ThreadMoth).
4. **Execution Submission & CAS Storage**:
   - Client calls `omen_execute` with `argv: ["echo", "proof_d_token_hello"]`.
   - Execution executes via daemon broker with JobObject / ProcessGroup containment.
   - Stdout is offloaded to CAS, returning `artifact://sha256/<hash>`.
5. **Bounded CAS Artifact Resource Retrieval**:
   - Client calls `resources/read` with `uri: "artifact://sha256/<hash>"`.
   - Server resolves blob path in canonical workspace CAS directory and returns bounded slice containing `proof_d_token_hello`.

### Automated Verification
- `mcp_tests::test_mcp_execute_and_read_cas_artifact_proof_d` (PASSED).
- `mcp_tests::test_mcp_duplex_stream_transport` (PASSED).
- `mcp_tests::test_mcp_malformed_json_rpc_handling` (PASSED).
- `mcp_tests::test_mcp_initialize_and_protocol_version` (PASSED).
