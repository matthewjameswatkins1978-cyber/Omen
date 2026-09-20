# Omen 0.6 Physical Maturity — Physical Proofs

**Status**: ALL 7 PROOFS PASSING ON REAL PATH  
**Test Suite**: `crates/omen-cli/tests/physical_maturity_proofs.rs`  

---

## Proof A: Daemon-Owned PTY Detach & Reattach Real Path

- **Test**: `test_proof_a_pty_detach_reattach_real_path`
- **Mechanism**:
  1. Spawns an interactive PTY session (`PtySessionId`) via `PtySessionManager` running `omen-gremlin --pty-echo`.
  2. Client writes input bytes `"HELLO_PTY_WORLD\n"`.
  3. Client reads output slice from offset 0, confirming receipt.
  4. Client detaches from the session. The PTY session transitions to `PtyState::Detached`.
  5. The underlying process continues executing in the background under daemon ownership.
  6. A second client reattaches to the existing session and reads output from the recorded offset, confirming byte continuity without buffer loss.
  7. Client terminates the PTY session.
- **Result**: PASS (0.21s)

---

## Proof B: Hostile Process Tree Ownership & Kill Real Path

- **Test**: `test_proof_b_process_tree_kill_real_path`
- **Mechanism**:
  1. Spawns a hostile multi-level process tree via `omen-gremlin --spawn-tree 2 --sleep-ms 60000`.
  2. The parent process spawns a child, which spawns a grandchild, writing `TREE_SPAWNED:<pid>` to stdout.
  3. The execution request specifies a bounded timeout of 1000ms.
  4. Upon timeout expiration, `ProcessSupervisor` terminates the process tree handle.
  5. On Windows, the process handle was assigned to a Windows Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
  6. The test asserts that the child and grandchild processes are completely terminated within 2000ms and no longer exist in the operating system process table.
- **Result**: PASS (1.08s)

---

## Proof C: Truthful Assurance Refusal Real Path

- **Test**: `test_proof_c_assurance_refusal_real_path`
- **Mechanism**:
  1. Requests execution on the Windows native backend with `required_assurance: RequiredAssurance::Enforced` for filesystem isolation.
  2. The Windows native backend truthfully reports filesystem isolation as `EnforcementLevel::Observed` (since hardware driver filesystem virtualization is not active).
  3. `ProcessSupervisor` runs preflight validation before process spawn.
  4. Preflight fails-closed, immediately returning `CoreError::PreflightFailed`.
  5. The test proves that the command never spawned and no unconfined process execution occurred.
- **Result**: PASS (0.00s)

---

## Proof D: Process Containment & Timeout Real Path

- **Test**: `test_proof_d_containment_real_path`
- **Mechanism**:
  1. Dispatches `omen-gremlin --sleep-ms 10000` with a strict execution timeout of 500ms.
  2. `ProcessSupervisor` bounds execution and terminates the process when the deadline passes.
  3. Asserts that the reported `RuntimeStatus` is `TimedOut` and total elapsed time is <= 1500ms.
- **Result**: PASS (0.52s)

---

## Proof E: Non-Native Backend (WSL) Real Path

- **Test**: `test_proof_e_non_native_wsl_backend_real_path`
- **Mechanism**:
  1. Validates dynamic availability of `WslExecutionBackend`.
  2. Translates current Windows workspace directory (`D:\Omen Shell`) into WSL path (`/mnt/d/Omen Shell`).
  3. Dispatches `pwd` through `wsl.exe`.
  4. Asserts exit code 0 and stdout matching `/mnt/d/Omen Shell` (case-insensitive drive translation).
  5. Verifies that backend descriptor reports `BackendKind::SubsystemWsl` and `EnforcementLevel::Mediated`.
- **Result**: PASS (0.42s)

---

## Proof F: Secret Injection & Redaction Real Path

- **Test**: `test_proof_f_secret_injection_and_redaction_real_path`
- **Mechanism**:
  1. Generates unique high-entropy canary secret tokens for environment variable and stdin injection.
  2. Executes `omen-gremlin --print-env AUTH_TOKEN --read-stdin` with injected `ExecutionSecret::env` and `ExecutionSecret::stdin`.
  3. The child process receives and echoes the secret values.
  4. `ProcessSupervisor` intercepts stdout and stderr streams and applies byte-level canary redaction.
  5. Asserts that the plaintext canary tokens are absent from stdout and replaced with `[REDACTED:AUTH_TOKEN]`.
  6. Stores output artifact in CAS via `ContentAddressedStore::store`.
  7. Reads the stored CAS slice and verifies that the CAS artifact itself contains the redacted content and does not leak the plaintext secret.
- **Result**: PASS (0.05s)

---

## Proof G: Service Lifecycle Lease & Disconnect Survival Real Path

- **Test**: `test_proof_g_service_ownership_real_path`
- **Mechanism**:
  1. Starts a managed background service (`proc://long_runner`) via `DaemonServer` and workspace `start_managed_service`.
  2. The service is assigned a dedicated `RuntimeLeaseId`.
  3. Simulates client session disconnection.
  4. Verifies that the service remains running and active under daemon ownership.
  5. Stops the managed service via `stop_managed_service`.
  6. Asserts bounded cleanup: process terminates, PID is released, and service state transitions to stopped.
- **Result**: PASS (0.12s)
