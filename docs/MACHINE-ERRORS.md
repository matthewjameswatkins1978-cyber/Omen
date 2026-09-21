# Omen machine errors

Omen domain failures use the versioned `omen-core::OmenError` envelope. Its
independent `schema_version` is currently `1`; it is not the MCP protocol
version, the execution-result schema version, or the workspace configuration
version.

The envelope contains:

- `code`: a stable uppercase `ErrorCode` registry value;
- `message`: human-readable context, not a machine discriminator;
- `category`: bounded authority, validation, semantic, execution, resource,
  evidence, persistence, protocol, or internal classification;
- `state_changed`: `NO`, `YES`, or `POSSIBLE`;
- `retryability`: `NEVER`, `AFTER_CHANGE`, `TRANSIENT`, `CONDITIONAL`, or
  `UNKNOWN`; this does not promise idempotent replay;
- `evidence`: availability plus bounded artifact references; and
- `details`: bounded structured JSON for operation-specific context.

Composition reports retain their existing step `code` and `message` fields for
compatibility and additionally expose the canonical envelope as `error`. CLI
machine failures expose `{ "ok": false, "error": <OmenError> }`. MCP tool
failures expose the same envelope in the tool result. JSON-RPC parse, method,
and parameter failures remain JSON-RPC protocol errors and are not converted
into domain errors.

`UNSUPPORTED` is not `NOT_FOUND`, and provider/readiness failures are not
`NOT_FOUND`. A caller may use `message` for display, but must branch on
`code`, `category`, state-change, retryability, and evidence fields.
