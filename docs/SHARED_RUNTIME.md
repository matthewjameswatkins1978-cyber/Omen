# Omen 0.4 — Shared Runtime Specification

> **Theme**: *One Omen reality across processes.*  
> **Core Doctrine**: *Omen is substrate, not sovereign.*  
> **Key Boundary**: *Personal interaction history remains local to the actor/session. Physical machine reality is shared.*

---

## 1. Architectural Vision

In Omen 0.2 and 0.3, execution, Facts, and history existed strictly per-process.
Omen 0.4 introduces a local daemon, `omend`, to create a **single, coherent machine reality shared across multiple processes** (interactive shells, background CLI commands, and autonomous agent clients) occupying the same workspace:

```text
Shell A ───┐
Shell B ───┤
CLI ───────┼── local IPC ──> omend ──> shared Omen reality
Agent ─────┘
```

They share:
- **Facts & Invalidation State**: Deterministic machine facts (`fact://test/status`, `fact://git/clean`) are unified.
- **Resource Generations**: File and tool generations increment once and invalidate across all observers.
- **Subordinate Physical History**: All physical machine actions are recorded in shared SQLite history.
- **Process & Service State**: Background services (`proc://`) are visible and inspectable by all workspace peers.
- **Hot Semantic Index**: High-speed, lock-free completion metadata is cached and updated incrementally.
- **CAS Evidence**: Content-Addressed Storage remains the shared immutable repository for command transcripts.

---

## 2. Inviolable System Boundaries

`omend` introduces physical runtime coordination. It does **not** introduce new authority.

```text
                  ┌──────────────┐
                  │   LANTERN    │ Durable memory, long-term context, provenance
                  │   "Knows"    │
                  └──────┬───────┘
                         │
                  ┌──────▼───────┐
                  │   RESOLVE    │ Live guard tokens, scope locks, fencing, conflict admission
                  │ "Coordinates"│
                  └──────┬───────┘
                         │
                  ┌──────▼───────┐
                  │   TETHERS    │ Policy, permission, capability identity, approval, intent, outcome truth
                  │  "Controls"  │
                  └──────┬───────┘
                         │
        ═════════════════╪═════════════════  Boundary Line
                         │
                  ┌──────▼───────┐
                  │     OMEN     │ Physical execution mechanics, containment, typed resources,
                  │   (omend)    │ shared Facts, shared history, process supervision
                  │ "Makes Legible│
                  │& Enforceable"│
                  └──────┬───────┘
                         │
                  ┌──────▼───────┐
                  │  THREADMOTH  │ Deterministic bounded structural mutation & refusal
                  │  "Mutates"   │
                  └──────────────┘
```

### Boundary Laws:
1. **Resolve coordinates live locks; omend coordinates physical runtime state**:
   - `omend` manages local database locks, IPC event fanout, and process bookkeeping.
   - It **never** issues guard tokens or decides semantic work conflicts that belong to Resolve.
2. **Tethers controls authority; Omen reports enforceability**:
   - **Same OS user does not equal Tethers authority.** Authenticating a local peer over IPC proves OS user identity; it does **not** grant permission for arbitrary consequential operations.
   - Service start, stop, restart, and tool execution remain subject to Tethers execution contracts.
3. **Session Scoping vs Shared Reality**:
   - `@last` means: *The most recent applicable operation initiated by the current interactive actor in the current interactive session.*
   - A background agent executing commands never alters Matthew's `@last`.
   - Shared reality: If that agent changes a file, Matthew's prompt immediately surfaces that the test Fact is `DIRTY`.

---

## 3. What 0.4 Is NOT

Omen 0.4 strictly avoids premature scope expansion:
- **Not an MCP Server**: MCP is an edge adapter for 0.5. Internal local IPC is Omen-native (`omen.local-ipc/1`).
- **Not an A2A Router**: Agent-to-Agent communication belongs to external systems.
- **Not a Remote Network Daemon**: `omend` listens strictly on local user-scoped IPC (Named Pipes / Unix Domain Sockets). No TCP, HTTP, or WebSocket endpoints.
- **Not an Autonomous Agent**: `omend` does not plan, reason, or decide actions.
- **Not a Cluster Scheduler / Distributed System**: Operates on a single host.
- **Not a Full Persistent PTY**: Interactive foreground programs (editors, pagers) remain client-local in 0.4. Full PTY abstraction arrives in 0.6.

---

## 4. Workspace Scope & Daemon Runtime Model

- **One Daemon Per OS User**: A single `omend` process serves all Omen workspaces for a given operating system user.
- **Multi-Workspace Isolation**: Workspaces are strictly partitioned:
  ```text
  omend
   ├── workspace A (/path/to/repoA)
   │    ├── SQLite DB & Facts
   │    ├── Hot Semantic Index
   │    ├── Sessions & Services
   │    └── Event Sequence (epoch A)
   └── workspace B (/path/to/repoB)
        ├── SQLite DB & Facts
        ├── Hot Semantic Index
        ├── Sessions & Services
        └── Event Sequence (epoch B)
  ```
  Zero state, facts, history, or services can leak between workspaces.

---

## 5. Local IPC Transport & Platform Endpoints

### 5.1 Unix & macOS
- **Transport**: Unix Domain Sockets (`tokio::net::UnixListener`).
- **Endpoint Location**:
  - Primary: `$XDG_RUNTIME_DIR/omen/1/omend.sock`
  - Fallback: `~/.omen/run/1/omend.sock`
- **Security & Permissions**:
  - Directory permissions must be `0700` (user-only).
  - Socket file ownership verified before connection or creation.
  - Parent directories checked to reject world-writable paths.

### 5.2 Windows
- **Transport**: Windows Named Pipes (`tokio::net::windows::named_pipe`).
- **Endpoint Location**: `\\.\pipe\omen-<user_hash>-1`
- **Security**:
  - Named Pipe Security Descriptor configured to allow connection only to current user token SID and system principals.
  - Does not treat local machine presence as sufficient authorization.

---

## 6. Shared Hot Semantic Index & Invalidation

### 6.1 Architecture
The daemon maintains an in-memory `SharedHotIndex` per workspace, caching:
- Active Facts (Current & Dirty);
- Workspace filesystem entries;
- Atlas tools and subcommands;
- Registered services.

### 6.2 Client Typing Isolation
**Keystroke completion never waits for IPC.**
- The interactive shell maintains a local `HotSemanticIndex`.
- When connecting or recovering, the client fetches a bounded `SharedIndexSnapshot`.
- The daemon pushes incremental `FactInvalidated` or `ServiceChanged` events out-of-band.
- The client applies events asynchronously to its local index without blocking typing.

### 6.3 Event Sequencing & Gap Detection
Every workspace event carries `(epoch: u64, sequence: u64)`:
- `epoch`: Incremented whenever `omend` starts or a workspace is rebound.
- `sequence`: Monotonically increasing counter per event.
- **Resync Invariant**: If a client receives a sequence gap (e.g. 101 followed by 104), the client declares `RESYNC_REQUIRED`, marks the local cache stale, and requests a fresh snapshot.

---

## 7. Filesystem Watcher & Pessimistic Invalidation

- **Integration**: Cross-platform file monitoring via `notify`.
- **Known vs Observed Mutation**:
  - *Known Omen Mutation* (e.g. ThreadMoth edit): Omen knows exact files modified and updates specific resource generations.
  - *Observed Watcher Event* (e.g. external editor save): Daemon marks dependencies as `possibly changed` and transitions affected Facts from `CURRENT` to `DIRTY`.
- **Watcher Overflow**: If watcher event queues overflow, daemon reports `RESYNC_REQUIRED` and pessimistically marks the broad workspace root dirty (`fs:workspace`).
- **No Silent Revalidation**: Neither the watcher nor `omend` will silently run `cargo test` to refresh facts. Invalidation is pessimistic; revalidation is explicit.

---

## 8. Execution Broker & Deduplication

### 8.1 Non-Interactive Supervised Execution
- Clients submit execution requests to `omend` via `ExecuteRequest`.
- `omend` dispatches through `omen_engine::ProcessSupervisor` with Job Objects (Windows) or process groups (Unix).
- Standard output/error exceeding 8 KiB is spooled directly to CAS artifacts (`artifact://`). Bounded summaries are returned over IPC.

### 8.2 Consequential Request Deduplication
To prevent duplicate execution on network/IPC disconnect:
1. Every consequential request carries a unique UUID `request_id`.
2. `omend` records a persistent `RequestReceipt` in SQLite (`ACCEPTED`, `RUNNING`, `COMPLETED`, `FAILED`).
3. If a client disconnects during execution and reconnects, it queries `status(request_id)` instead of resubmitting.
4. If status is `UNKNOWN`, automatic re-execution is strictly refused.

---

## 9. Shared Services Registry

- Managed processes (`proc://`) are registered in `omend` and stored in SQLite.
- Scoped to workspace; starting a duplicate name returns `SERVICE_ALREADY_RUNNING`.
- Lifecycle States: `STARTING`, `RUNNING`, `STOPPED`, `FAILED`, `UNKNOWN`, `ORPHANED`.
- **Daemon Restart Reconciliation**: PIDs alone are never trusted due to OS PID recycling. Daemon persists start tokens and executable paths. On restart, unverified processes transition to `UNKNOWN` or `ORPHANED`.
- Bounded rolling log tails are served over IPC; full output is spooled to CAS.

---

## 10. Truthful Degraded Mode

Omen remains functional if `omend` fails or is stopped:
- State transitions to `SHARED_RUNTIME_OFFLINE`.
- Prompt displays a compact, calm indicator: `! shared runtime offline · local mode`.
- Shell falls back to standalone local execution, in-memory completion, and local database access.
- On daemon reconnect, client emits an `OfflineGap` marker; daemon pessimistically dirties workspace facts to reconcile untracked local mutations.
