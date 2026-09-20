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
2. **First-Class Daemon-Owned PTY Engine**: Detachable and resumable interactive sessions with bounded 64 KiB ring buffers, real OS pseudo-terminal sizing and resize event propagation observed by the child process, and disconnect resilience.
3. **True Process-Tree Ownership**: Windows Job Objects (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) providing `ENFORCED` kernel-level descendant termination; Unix process groups (`setpgid(0, 0)`) providing `BEST_EFFORT` descendant termination (truthfully acknowledging that unconfined POSIX descendants calling `setsid()` / `setpgid()` escape ordinary process groups without cgroups v2).
4. **Physical Lifecycle Leases**: `RuntimeLeaseId` tracking service lifecycles with bounded cleanup deadlines (graceful signal -> timeout -> hard termination).
5. **Truthful Platform Assurance Matrix**: Canonical enforcement levels (`ENFORCED`, `MEDIATED`, `OBSERVED`, `BEST_EFFORT`, `UNSUPPORTED`) with fail-closed preflight refusing execution when required assurance cannot be met. Symlink escape is reported truthfully as `OBSERVED` on native host platforms and `MEDIATED` on WSL.
6. **Secret Handles & Redaction**: Strict separation of `secret.use` from `secret.expose`, with automatic byte-level canary redaction for environment variables, standard input streams, and temporary credential files in stdout, stderr, and CAS storage.
7. **Dual Raw / Sanitized Evidence Model**: Verbatim untampered raw bytes (with secrets redacted) are preserved in CAS (`artifact://`) for forensic integrity and deterministic audit, while `stdout_sanitized()` and `stderr_sanitized()` strip hostile CSI/OSC/BEL control sequences to safeguard terminal emulators, daemon logs, and AI agent prompts.
8. **Interactive Shell Control**: Added `:backend list`, `:backend status`, and `:backend use <id>` for live inspection and backend switching.

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
- Strongly typed IDs: `BackendId`, `PtySessionId`, `RuntimeLeaseId`.
- Canonical enforcement levels in `EnforcementLevel`: `Enforced`, `Mediated`, `Observed`, `BestEffort`, `Unsupported`.
- Session & Lease lifecycle states: `PtyState` (`Running`, `Detached`, `Exited`) and `RuntimeLeaseState` (`Active`, `Releasing`, `Released`, `Expired`).
- Secret injection contracts: `SecretInjectionContract` (`EnvironmentVariable`, `Stdin`, `TemporaryFile`) and `SecretHandle`.
- Updated wire schema parser in `omen-schema` supporting all 0.6 enforcement levels and execution structures.

### Slice B & C: Execution Backend SPI & Registry (`crates/omen-engine`)
- Pluggable SPI trait: `ExecutionBackend` with `spawn()` and `spawn_pty()`.
- Async execution handles: `ExecutionHandle` and `PtyExecutionHandle` providing bounded timeouts, process tree termination, and I/O streaming.
- `BackendRegistry`: Thread-safe backend management with discovery and active backend selection.
- `NativeExecutionBackend`: Native host OS backend using Windows Job Objects on Windows and POSIX process groups on Unix.
- `WslExecutionBackend`: Non-native execution backend bridging Windows host commands to WSL Linux instances, including automatic Windows-to-WSL path translation (`D:\...` -> `/mnt/d/...`).

### Slice D & E: Bounded PTY Engine & Daemon Service (`crates/omen-engine`, `crates/omen-daemon`, `crates/omen-ipc`)
- Bounded Ring Buffer: `RingBuffer` retaining at most 64 KiB of output, supporting offset-based slicing and eviction tracking.
- PTY Child Terminal Awareness: Child processes observe a real terminal (`isatty` / `IsTerminal`), detect initial window geometry (e.g. 80x24), and receive live resize signals (`SIGWINCH` / ConPTY resize) to adjust dimensions (e.g. 100x30).
- Dual Evidence Pipeline: Verbatim untampered byte stream stored in CAS for deterministic replay; sanitized output (`sanitize_terminal_escapes`) stripping CSI/OSC sequences and BEL codes for UI logs and LLM context.
- Daemon PTY Service: `PtySessionManager` managing background PTY sessions independent of client connection lifecycle.
- IPC Protocol: PTY messages (`CreatePtySession`, `AttachPtySession`, `DetachPtySession`, `WritePtyInput`, `ReadPtyOutput`, `ResizePty`, `TerminatePty`, `ListPtySessions`).

### Slice F & G: Process Tree Containment & Service Leases (`crates/omen-engine`, `crates/omen-daemon`)
- Process-Tree Ownership: Configures Windows Job Objects with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. When parent execution completes or times out, all descendant processes terminate immediately (`ENFORCED`). On Unix, process groups (`setpgid(0, 0)`) provide `BEST_EFFORT` termination.
- Physical Lifecycle Leases: `RuntimeLeaseId` associated with managed background services. Services survive client disconnects under daemon supervision.
- Bounded Cleanup: Service shutdown executes graceful signal followed by hard termination deadline (`kill_process`).

### Slice H & I: Truthful Assurance & Secret Redaction (`crates/omen-engine`)
- Preflight Validation: `ProcessSupervisor` validates requested assurance against backend capabilities. If unsupported, execution fails closed with `CoreError::AssuranceNotSatisfied`.
- Secret Injection: Supports environment variables, standard input streams, and temporary credential files.
- Byte Redaction: Plaintext secret values are scrubbed and replaced with `[REDACTED:<id>]` from stdout, stderr, and CAS storage. `Debug` implementations mask secret values with `[REDACTED]`.

### Slice J: Interactive Shell Integration (`crates/omen-interactive`)
- Added `:backend list`, `:backend status`, and `:backend use <id>`.
- Shell dynamically reconfigures its execution supervisor to route physical commands through the selected backend.

---

## 4. Verification & Proof Suite

All 8 real-path proofs pass completely in `crates/omen-cli/tests/physical_maturity_proofs.rs`:

1. **Proof A: Daemon PTY Detach, Reattach & Child Terminal Resize Real Path** (`test_proof_a_pty_detach_reattach_real_path`):
   - Proves child detects a real terminal (`IS_TERMINAL:true`) and initial size (`80x24`).
   - Proves interactive input and output across client detachment.
   - Proves dynamic resize to `100x30` is received and observed by the child process (`CURRENT_SIZE:100x30`).
   - Proves offset continuity and ring buffer bounded retention.

2. **Proof B: Process Tree Kill Real Path** (`test_proof_b_process_tree_kill_real_path`):
   - Proves deep hostile process tree (child, grandchild) spawned by `omen-gremlin --spawn-tree 2`.
   - Proves Windows Job Object terminates all descendants upon timeout; process table confirms zero lingering processes.

3. **Proof C: Assurance Refusal Real Path** (`test_proof_c_assurance_refusal_real_path`):
   - Proves fail-closed preflight refusal when requesting `ENFORCED` filesystem isolation on native host without sandbox.
   - Proves process is never spawned.

4. **Proof D: Containment Real Path** (`test_proof_d_containment_real_path`):
   - Proves execution timeout terminates uncooperative long-running processes within bounded latency.
   - Proves truthful reporting of `EnforcementLevel::Observed` for native filesystem and symlink escape.

5. **Proof E: Non-Native WSL Backend Real Path** (`test_proof_e_non_native_wsl_backend_real_path`):
   - Proves execution through WSL2 Linux kernel with Windows-to-WSL path translation.
   - Proves backend capabilities report `EnforcementLevel::Mediated`.

6. **Proof F: Secret Injection & Redaction Real Path** (`test_proof_f_secret_injection_and_redaction_real_path`):
   - Proves both environment variable and stdin secret injection (`AUTH_TOKEN` and `STDIN_TOKEN`).
   - Proves child echoes secrets, but plaintext canary tokens are scrubbed from stdout and CAS storage.
   - Proves `[REDACTED:AUTH_TOKEN]` and `[REDACTED:STDIN_TOKEN]` replace the canaries.
   - Proves `Debug` representation never leaks cleartext secrets.

7. **Proof G: Service Ownership & Disconnect Survival Real Path** (`test_proof_g_service_ownership_real_path`):
   - Proves managed background service with `RuntimeLeaseId` survives shell disconnect and reconnect.
   - Proves bounded service shutdown terminates physical process.

8. **Proof H: Hostile Terminal Escape Sanitization vs Verbatim Raw Evidence** (`test_proof_hostile_terminal_sanitization_vs_raw_evidence`):
   - Proves child emitting hostile CSI, OSC window titles, and BEL characters.
   - Proves CAS stores verbatim untampered byte stream for forensics and cryptographic replay.
   - Proves `stdout_sanitized()` strips all escape sequences, BEL bytes, and malicious title payloads, preserving safe legitimate data.
