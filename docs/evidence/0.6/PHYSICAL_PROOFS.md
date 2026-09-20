# Omen 0.6 Physical Maturity — Physical Proofs

**Status**: ALL 8 PROOFS PASSING ON REAL PATH  
**Test Suite**: `crates/omen-cli/tests/physical_maturity_proofs.rs`  

---

## Proof A: Daemon-Owned PTY Detach, Reattach & Child Terminal Resize Real Path

- **Test**: `test_proof_a_pty_detach_reattach_real_path`
- **Mechanism**:
  1. Spawns an interactive PTY session (`PtySessionId`) via `PtySessionManager` running `omen-gremlin --pty-echo`.
  2. Child process confirms attached terminal semantics (`isatty` / `std::io::IsTerminal`) emitting `IS_TERMINAL:true` and initial size `INITIAL_SIZE:80x24`.
  3. Client 1 attaches, writes input bytes `"HELLO_PTY_WORLD\n"`, and verifies echo.
  4. Client 1 detaches from the session; session transitions to `PtyState::Detached` while the process continues running under daemon ownership.
  5. Client 2 reattaches, verifies buffered output from offset 0, and asserts offset continuity.
  6. Client 2 resizes the PTY to 30 rows by 100 columns (`pty_manager.resize(..., 30, 100)`).
  7. Client 2 sends `get_size` command to the child process; child executes `crossterm::terminal::size()` and responds with `CURRENT_SIZE:100x30`, proving real OS pseudo-terminal resize event delivery.
  8. Client terminates the PTY session.
- **Result**: PASS

---

## Proof B: Hostile Process Tree Ownership & Kill Real Path

- **Test**: `test_proof_b_process_tree_kill_real_path`
- **Mechanism**:
  1. Spawns a hostile multi-level process tree via `omen-gremlin --spawn-tree 2 --sleep-ms 60000`.
  2. The parent process spawns a child, which spawns a grandchild, writing `TREE_SPAWNED:<pid>` to stdout.
  3. The execution request specifies a bounded timeout of 1000ms.
  4. Upon timeout expiration, `ProcessSupervisor` terminates the execution handle.
  5. On Windows, the process was assigned to a Windows Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
  6. The test asserts that both child and grandchild processes are forcefully terminated and no longer exist in the operating system process table.
- **Result**: PASS

---

## Proof C: Truthful Assurance Refusal Real Path

- **Test**: `test_proof_c_assurance_refusal_real_path`
- **Mechanism**:
  1. Requests execution on the Windows native backend with `required_assurance: RequiredAssurance::Enforced` for filesystem isolation.
  2. The Windows native backend truthfully reports filesystem isolation as `EnforcementLevel::Observed`.
  3. `ProcessSupervisor` runs preflight validation before process spawn.
  4. Preflight fails closed, immediately returning `CoreError::AssuranceNotSatisfied`.
  5. The test proves that the command never spawned and no unconfined process execution occurred.
- **Result**: PASS

---

## Proof D: Process Containment & Timeout Real Path

- **Test**: `test_proof_d_containment_real_path`
- **Mechanism**:
  1. Dispatches `omen-gremlin --sleep-ms 10000` with a strict execution timeout of 500ms.
  2. `ProcessSupervisor` bounds execution and terminates the process when the deadline passes.
  3. Asserts that reported `RuntimeStatus` is `TimedOut` and total elapsed time is <= 1500ms.
  4. Asserts that enforcement report correctly emits `EnforcementLevel::Observed` for filesystem isolation and symlink escape prevention.
- **Result**: PASS

---

## Proof E: Non-Native Backend (WSL) Real Path

- **Test**: `test_proof_e_non_native_wsl_backend_real_path`
- **Mechanism**:
  1. Validates dynamic availability of `WslExecutionBackend`.
  2. Verifies backend capabilities report `EnforcementLevel::Mediated` for descendants, filesystem, network, and symlink escape.
  3. Dispatches execution of `echo hello-from-wsl-kernel` via `wsl.exe`.
  4. Asserts exit code 0 and stdout matching `hello-from-wsl-kernel`.
- **Result**: PASS

---

## Proof F: Secret Injection & Redaction Real Path

- **Test**: `test_proof_f_secret_injection_and_redaction_real_path`
- **Mechanism**:
  1. Generates unique high-entropy canary secret tokens for environment variable (`canary_env`) and stdin injection (`canary_stdin`).
  2. Asserts `Debug` representation of `ExecutionSecret` masks raw secret values with `[REDACTED]` and leaks neither canary token.
  3. Executes `omen-gremlin --print-env AUTH_TOKEN --echo-stdin` with injected secrets.
  4. Child echoes both the environment variable and the standard input payload.
  5. `ProcessSupervisor` intercepts output streams and applies byte-level canary redaction.
  6. Asserts that neither raw canary token appears in stdout, and both `[REDACTED:AUTH_TOKEN]` and `[REDACTED:STDIN_TOKEN]` appear.
  7. Stores raw output in CAS (`ContentAddressedStore::store`) and verifies that the CAS-stored artifact also contains the redacted representation with zero plaintext secret leakage.
- **Result**: PASS

---

## Proof G: Service Lifecycle Lease & Disconnect Survival Real Path

- **Test**: `test_proof_g_service_ownership_real_path`
- **Mechanism**:
  1. Starts a managed background service (`test-worker-service`) via `DaemonServer` and workspace `start_managed_service`.
  2. The service is assigned a dedicated `RuntimeLeaseId` (`lease-...`).
  3. Simulates client session disconnection and queries the service registry from a newly connected client.
  4. Verifies that the service remains running, retains its PID, and retains its lease across disconnect.
  5. Stops the managed service via `stop_managed_service`.
  6. Asserts bounded cleanup: process terminates, PID is released, and service state transitions to `stopped`.
- **Result**: PASS

---

## Proof H: Hostile Terminal Escape Sanitization vs Verbatim Raw Evidence

- **Test**: `test_proof_hostile_terminal_sanitization_vs_raw_evidence`
- **Mechanism**:
  1. Dispatches `omen-gremlin --hostile-terminal-escapes`, which emits raw ANSI CSI clear-screen sequences (`\x1b[2J\x1b[H`), color formatting, hostile OSC window title injection (`\x1b]0;HostileWindowTitle\x07`), alert bells (`\x07`), and legitimate payload data.
  2. Confirms that raw execution output (`stdout_all`) retains untampered escape sequences, BEL control characters, and full hostile window title payloads for forensic integrity.
  3. Stores raw output into CAS and confirms byte-for-byte identity with untampered byte stream.
  4. Evaluates `output.stdout_sanitized()` and verifies:
     - Contains zero ESC (`0x1b`) bytes.
     - Contains zero BEL (`0x07`) bytes.
     - Completely removes the malicious window title payload (`HostileWindowTitle` and `HackedTitle`).
     - Preserves legitimate text data (`LEGITIMATE_DATA`).
- **Result**: PASS
