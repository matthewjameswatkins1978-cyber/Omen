# Omen Interoperability Seam Architecture

> **Doctrine**: *Omen is substrate, not sovereign.*  
> **Boundary Rule**: *Tethers controls; Omen executes and makes legible; protocol adapters project at the edges.*

---

## 1. Architectural Purpose

As Omen progresses from 0.3 Human Interface toward 0.5 Agent Interoperability, external autonomous agents and tool ecosystems (such as the Model Context Protocol / MCP, Language Server Protocol / LSP, and Agent-to-Agent protocols) need to interface with Omen.

To prevent architecture decay, Omen enforces a strict **Interoperability Seam** inspired by Tethers:
1. Core Omen domain types (`ExecutionRequest`, `ExecutionResult`, `FactRecord`, `ResourceUri`, `ArtifactId`) are the **canonical truth**.
2. No MCP-specific or vendor-specific data structures or runtime assumptions are permitted inside the core substrate.
3. External protocols are projected at the edge via protocol adapters.

---

## 2. The Three Orthogonal Axes

Interoperability in Omen is factored along three strictly orthogonal axes:

```
[ Axis 1: Canonical Semantics ]  ──>  What the operation means in Omen
              │
              ▼
[ Axis 2: Protocol Binding ]     ──>  How the operation is shaped for a protocol (MCP, LSP, CLI)
              │
              ▼
[ Axis 3: Transport ]            ──>  How bytes travel across process boundaries (stdio, named pipe, socket)
```

1. **Canonical Semantics**:
   - The strongly typed Omen domain model.
   - Independent of wire protocols or serialization choices.
   - Governed by Omen core contracts (`crates/omen-core`, `crates/omen-schema`).

2. **Protocol Binding**:
   - Maps Omen tools and facts to external protocol primitives.
   - For MCP: maps `ToolProfile` to MCP Tool definitions, maps Fact Registry to MCP Resources.
   - For LSP: maps diagnostic artifacts to LSP Diagnostic items.
   - Protocol bindings do not invent new execution mechanisms; they delegate to Omen's canonical supervisor.

3. **Transport**:
   - The byte-level communication channel.
   - Decoupled from protocol semantics: an MCP binding can operate over `stdio`, Windows Named Pipes, Unix Domain Sockets, or HTTP/SSE without altering its semantic mapping.

---

## 3. Adapter Lifecycle & Distinction

To avoid conflating packages, configurations, and running processes, Omen defines three distinct lifecycle concepts:

| Entity | Description | Mutability | Location |
| :--- | :--- | :--- | :--- |
| **Adapter Package** | The portable distribution bundle (manifest, binary/script, wire schema, checksums, conformance fixtures). | Immutable | Registry / Tarball |
| **Installed Adapter** | An unpacked, verified adapter in the workspace or system catalog with exact version/digest. | Read-Only | Workspace Store (`.omen/adapters`) |
| **Exact Binding** | An active runtime instance bound to a specific session ID, transport channel, and capability scope. | Ephemeral | Runtime Memory / `omend` |

### Adapter Lifecycle States
```text
inspect ──> validate ──> conform ──> install ──> bind ──> active
```
Possible non-active states:
- `stale` (dependency or binary updated; requires re-conformance)
- `incompatible` (fails baseline protocol or schema expectations)
- `degraded` (partial protocol support; unsupported optional capabilities refused)
- `unavailable` (transport unreachable or process crashed)
- `disabled` (administratively turned off)
- `quarantined` (failed security/conformance check or erratic behavior detected)

Conformance proves protocol and translation behavior. It does not establish authority.

---

## 4. Compatibility Profiles and Interop Sets

External consumers declare their exact protocol expectations using **Compatibility Profiles** (also termed **Interop Sets**):

```toml
[interop]
profile = "agent-interop-2026"
requires_capabilities = [
    "mcp.tools.2026-07-28",
    "mcp.resources.2026-07-28",
    "a2a.tasks.1.0",
]
optional_capabilities = [
    "agentsmd.instructions.1.x",
]
conformance_test = "strict"
```

### Key Principles:
1. **Zero Authority Grant**: Declaring an Interop Set validates protocol compatibility; it **never** grants permission, authority, or elevated trust.
2. **Strict Authority Division**: Permission, approval, capability identity, durable intent, and provider outcome truth remain sovereign to **Tethers**.
3. **Pre-Activation Conformance**: Omen executes a dry-run capability handshake against the adapter before activation. If the adapter fails the interop set contract, activation is refused.
4. **Independent Version Axes**: Never collapse versioning into "latest". Track each axis independently:
   - Substrate Version: `omen 0.3.0`
   - Canonical Schema/SPI Version: `schema 0.2`
   - Adapter Package Format: `pkg 1.0`
   - Adapter Implementation Version: `mcp-edge 0.1.0`
   - External Protocol Version: `mcp 2026-07-28`
   - Protocol Extension Version: `ext.roots 1.0`
   - Transport-Binding Version: `stdio.v1`

---

## 5. The 10-Step Adapter Conformance Crucible

Before claiming support for an external standard, an adapter must pass the 10-step crucible:
1. **Inspect** package without executing it (static manifest and hash verification).
2. **Validate** schema and version claims against canonical wire definitions.
3. **Run** controlled conformance fixtures in an isolated test harness.
4. **Prove** deterministic translation into canonical Omen types.
5. **Prove** unsupported semantics refuse explicitly rather than silently faking support.
6. **Preserve** identity and provenance across translation boundaries.
7. **Prove** unknown extension data cannot grant authority or bypass constraints.
8. **Test** output projection into external protocol formats.
9. **Test** round-trip serialization where bidirectional mapping is promised.
10. **Record** exact package, protocol, and version evidence in the Tool Atlas.

---

## 6. Summary of Seam Boundaries

```
┌─────────────────────────────────────────────────────────────┐
│                           Tethers                           │
│     (Permission, Trust, Policy, Intent, Outcome Truth)     │
└──────────────────────────────┬──────────────────────────────┘
                               │ Governs Intent & Grants
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                    Omen Canonical Engine                    │
│   (ProcessSupervisor, FactRegistry, ContentAddressedStore)  │
└──────────────────────────────┬──────────────────────────────┘
                               │ Implements Substrate
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                 Omen Interoperability Seam                  │
│       Compatibility Profiles / Interop Sets (No Trust)      │
└──────────────┬───────────────────────────────┬──────────────┘
               │                               │
               ▼                               ▼
       [MCP Protocol Binding]          [LSP Protocol Binding]
               │                               │
               ▼                               ▼
      [Stdio / Pipe Transport]       [Stdio / Socket Transport]
               │                               │
               ▼                               ▼
          External Agent                 External IDE
```
