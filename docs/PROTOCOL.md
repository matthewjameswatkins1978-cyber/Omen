# Omen Execution Protocol

## Overview
The Omen protocol defines the wire contracts exchanged between higher-level orchestrators (like Tethers) and the physical Omen runtime.

## Schema Versioning
- Execution Contract: `omen.execution/0.2`
- Execution Result: `omen.result/0.2`

Contracts and results are strictly typed and reject unexpected fields (`deny_unknown_fields`).

## Execution Contract Model
An execution contract specifies:
- `schema_version`: String (`"omen.execution/0.2"`)
- `execution_id`: Tethers execution URI (e.g. `tethers://exec/01K9F82A`)
- `actor`: Actor URI (e.g. `actor://agent/codex/session-42`)
- `intent`: Target tool and argument list
- `leases`: Requested resource leases and access rights
- `stdio`: Stdio configuration (default stdin is closed)
- `constraints`: Timeouts, network containment, etc.
- `required_assurance`: Required assurance levels per subsystem (e.g. filesystem: `ENFORCED`)

## Execution Result Model
An execution result reports:
- `schema_version`: String (`"omen.result/0.2"`)
- `execution_id`: The correlated execution ID
- `action_id`: Omen physical action ID (e.g. `omen://action/01K9F82B`)
- `runtime_status`: `COMPLETED`, `SPAWN_FAILED`, `TIMED_OUT`, `CANCELLED`, `CONTAINMENT_FAILED`, `IO_FAILED`
- `process_exit`: Exit code or signal
- `adapter_classification`: `SUCCESS`, `FAILURE`, `REFUSAL`, `UNKNOWN`
- `enforcement`: Actual assurance achieved for filesystem, network, descendants, and link escapes
- `observations`: Dynamic observations emitted during execution
- `fact_updates`: Facts established or modified
- `artifacts`: Artifact URIs in CAS
- `reduced_summary`: Bounded text summary
