# Omen 0.6 Physical Maturity — Implementation Report

**Release**: Omen 0.6.0  
**Target**: Physical Execution Maturity, Containment, PTY Engine, Leases & Truthful Assurance  
**Status**: VERIFIED & COMPLETE  
**Repository**: `https://github.com/matthewjameswatkins1978-cyber/Omen.git`  
**Branch**: `feature/omen-0.6-physical-maturity`  

---

## 1. Executive Summary

Omen 0.2 established machine truth: typed resources, structured execution contracts, Fact Registry, dirty/current truth, Tool Atlas, CAS storage, and process containment.  
Omen 0.3 provided an interactive human shell: 3-lane grammar, non-blocking fact completion, subordinate physical history, and semantic blocks.  
Omen 0.4 introduced the shared runtime daemon (`omend`), coordinating shared facts, session recovery, and managed services.  
Omen 0.5 established agent interoperability: standard MCP server, agent-inside-Omen interactive experience, and typed proposed actions without terminal scraping.  

**Omen 0.6 makes physical execution genuinely mature.**

Headline doctrine:
> **If Omen says it owns, contains, supervises, can resume, or has restricted a process, that statement must correspond to something the operating system or execution backend genuinely enforces.**

Omen 0.6 delivers:
1. **Pluggable Execution Backend SPI**: Strict separation of execution requests from execution backends (`ExecutionBackend`, `BackendRegistry`, `NativeExecutionBackend`, `WslExecutionBackend`).
2. **First-Class Daemon-Owned PTY Engine**: Detachable and resumable interactive sessions with bounded 64 KiB ring buffers, terminal escape sequence sanitization for logs/CAS, and disconnect resilience.
3. **True Process-Tree Ownership**: Windows Job Objects (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) and Unix process groups (`setpgid(0, 0)`) ensuring that terminating a process kills all children, grandchildren, and daemonized descendants.
4. **Physical Lifecycle Leases**: `RuntimeLeaseId` tracking service lifecycles with bounded cleanup deadlines (SIGTERM/graceful -> timeout -> SIGKILL/TerminateProcess).
5. **Truthful Platform Assurance Matrix**: Canonical enforcement levels (`ENFORCED`, `MEDIATED`, `OBSERVED`, `BEST_EFFORT`, `UNSUPPORTED`) with fail-closed preflight refusing execution when required assurance cannot be met.
6. **Secret Handles & Redaction**: Strict separation of `secret.use` from `secret.expose`, with automatic byte-level canary redaction in stdout, stderr, and CAS storage.
7. **Interactive Shell Control**: Added `:backend list`, `:backend status`, and `:backend use <id>` for inspection and backend switching.

---

## 2. Core Architectural Doctrine

> **Omen is substrate, not sovereign.**

The division of responsibility remains inviolable:
- **Lantern** knows (durable memory, long-term context, and provenance).
- **Resolve** coordinates (live guard tokens and scope locks).
- **Tethers** controls (permission, trusted capability identity, policy, approval, durable intent, replay, and provider outcome truth).
- **Omen** makes the machine legible and enforceable (physical execution, containment, typed resources, machine-readable facts, and artifact CAS evidence).
- **ThreadMoth** deterministically mutates (bounded structural edits with cryptographic pre/post hashes and refusal).

Compact system statement:
> **Lantern knows. Resolve coordinates. Tethers controls. Omen makes the machine legible and enforceable. ThreadMoth mutates deterministically.**

Tethers controls authority; Omen reports enforceability. Omen introduces zero competing permission systems, approval policies, or lock mechanisms.

---

## 3. Implemented Deliverables

### Slice A: Physical Vocabulary & Schema (`crates/omen-core`, `crates/omen-schema`)
- Added strongly typed IDs: `BackendId`, `PtySessionId`, `RuntimeLeaseId`.
- Added canonical enforcement levels in `EnforcementLevel`: `Enforced`, `Mediated`, `Observed`, `BestEffort`, `Unsupported`.
- Added `PtyState` (`Running`, `Detached`, `Exited`) and `RuntimeLeaseState` (`Active`, `Releasing`, `Released`, `Expired`).
- Added `SecretInjectionContract` (`EnvironmentVariable`, `StandardInput`, `TemporaryFile`) and `SecretHandle`.
- Updated wire parser in `omen-schema` for all 0.6 enforcement levels and execution structures.

### Slice B & C: Execution Backend SPI & Registry (`crates/omen-engine`)
- Trait: `ExecutionBackend` with `spawn()` and `spawn_pty()`.
- Handles: `ExecutionHandle` and `PtyExecutionHandle` providing bounded async waits, process tree termination, and I/O streaming.
- `BackendRegistry`: Thread-safe backend management with discovery and active backend selection.
- `NativeExecutionBackend`: Native host OS backend using Windows Job Objects and Unix process groups.
- `WslExecutionBackend`: Non-native execution backend bridging Windows host commands to WSL Linux instances, including automatic Windows-to-WSL path translation (`C:\...` -> `/mnt/c/...`).

### Slice D & E: Bounded PTY Engine & Daemon Service (`crates/omen-engine`, `crates/omen-daemon`, `crates/omen-ipc`)
- Bounded Ring Buffer: `RingBuffer` retaining at most 64 KiB of output, supporting offset-based slicing and eviction tracking.
- Terminal Escape Sanitization: Strips ANSI escape sequences, CSI/OSC control codes, and bell characters for human logs, fact provenance, and CAS storage.
- Daemon PTY Service: `PtySessionManager` managing background PTY sessions independent of client connection lifecycle.
- IPC Protocol: Added PTY request/response messages (`CreatePtySession`, `AttachPtySession`, `DetachPtySession`, `WritePtyInput`, `ReadPtyOutput`, `ResizePty`, `TerminatePty`, `ListPtySessions`).

### Slice F & G: Process Tree Containment & Service Leases (`crates/omen-engine`, `crates/omen-daemon`)
- Process-Tree Ownership: Configures Windows Job Objects with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. When parent execution completes or times out, all descendant processes terminate immediately.
- Physical Lifecycle Leases: `RuntimeLeaseId` associated with managed background services. Services survive client disconnects.
- Bounded Cleanup: Service shutdown executes graceful signal followed by hard termination deadline (`kill_process`).

### Slice H & I: Truthful Assurance & Secret Redaction (`crates/omen-engine`)
- Preflight Validation: `ProcessSupervisor` validates requested assurance against backend capabilities. If unsupported, execution fails closed with `CoreError::PreflightFailed`.
- Secret Injection: Supports environment variables, standard input streams, and temporary credential files.
- Byte Redaction: Plaintext secret values are scrubbed and replaced with `[REDACTED:<id>]` from stdout and stderr prior to CAS storage and context pagination.

### Slice J: Interactive Shell Integration (`crates/omen-interactive`)
- Added `:backend list`, `:backend status`, and `:backend use <id>`.
- Shell dynamically reconfigures its execution supervisor to route physical commands through the selected backend.

---

## 4. Verification & Proof Suite

All 7 real-path proofs pass completely in `crates/omen-cli/tests/physical_maturity_proofs.rs`:
- **Proof A**: Daemon PTY detach and reattach real path (64 KiB bounded ring buffer, offset continuity).
- **Proof B**: Process tree kill real path (Windows Job Object termination of deep hostile process trees).
- **Proof C**: Assurance refusal real path (preflight rejection when requesting `ENFORCED` on unsupported platform).
- **Proof D**: Containment real path (timeout enforcement and clean termination).
- **Proof E**: Non-native backend real path (WSL execution and Windows-to-WSL path translation).
- **Proof F**: Secret injection and redaction real path (plaintext canary secret scrubbed from stdout and CAS).
- **Proof G**: Service ownership and disconnect survival real path (daemon-owned service surviving client detachment).

Additionally:
- Full workspace test suite: `cargo test --workspace` passes (0 failed).
- Lints and formatting: `cargo fmt --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` pass cleanly.
- Repository verification: `cargo run -p xtask -- verify` passes all supply chain, bans, licenses, and schema audits.
