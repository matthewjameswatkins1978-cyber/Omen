# End-to-End Proofs (Omen 0.2)

This document provides recorded execution evidence for the end-to-end integration and assurance proofs in Omen 0.2.

---

## End-to-End Proof 1: Authoritative Lifecycle & Refusal Verification

**Test Target**: `crates/omen-cli/tests/e2e_proofs.rs:proof_1_authoritative_lifecycle_and_refusal`  
**Fixture**: `tests/fixtures/auth_project`

### Lifecycle Progression:
1. **Initial Verification**:
   - `CargoAdapter::test` executes against clean fixture.
   - Result: `passing`, exit code `0`.
   - Fact published: `fact://test/status` = `passing`, validity `CURRENT`, assurance `VERIFIED`.
   - Fact is retrievable with `require_current = true`.

2. **Deterministic Mutation (ThreadMoth)**:
   - `ThreadMothAdapter::replace_exact` mutates `token == "omen-valid-token"` to `token == "different-token"`.
   - Pre/post cryptographic SHA-256 hashes captured in ThreadMoth certificate: outcome `APPLIED`.
   - Workspace filesystem dependency generation `fs:workspace` is incremented.

3. **Lazy Pessimism Refusal**:
   - Querying `FactRegistry::get_fact("fact://test/status", require_current = true)` **refuses** with `CoreError::FactDirty`.
   - Omen **refuses to silently rerun the command** or forge validity.
   - Querying with `require_current = false` returns the fact with validity `DIRTY`.

4. **Explicit Re-Execution**:
   - Explicit invocation of `CargoAdapter::test` detects test failure (exit code `101`).
   - New fact published: `fact://test/status` = `failing`, validity `CURRENT`, assurance `VERIFIED`.
   - Previous `passing` fact is marked `SUPERSEDED` by the new fact.

5. **Provenance & CAS Evidence Inspection**:
   - `FactRegistry::why_fact` traces the failing fact:
     - Exact dependencies recorded: `fs:workspace` at current generation.
     - History contains the preceding `passing` fact.
     - Associated CAS artifact references the raw test failure transcript (`artifact://sha256/...`).
   - Inspecting and reading bounded slices from the CAS store returns the full compiler/test failure output (`test tests::test_auth_success ... FAILED`).

6. **Deterministic Reversion**:
   - `ThreadMothAdapter::replace_exact` reverts the code using the expected `post_hash` from mutation 1.
   - Mutation succeeds (`APPLIED`).
   - Generation `fs:workspace` is incremented.
   - Querying `require_current = true` **again refuses** with `CoreError::FactDirty`: restoring code does **NOT** magically make the test fact `CURRENT` without verification.

7. **Re-Verification**:
   - Explicit `CargoAdapter::test` run produces `passing`, validity `CURRENT`, assurance `VERIFIED`.

---

## End-to-End Proof 2: Hostile Gremlin Torture Suite

**Test Target**: `crates/omen-engine/tests/engine_tests.rs`  
**Executable**: `omen-gremlin`

### Hostile Behaviors & Engine Guarantees:
1. **Unbounded Output Flood**:
   - Gremlin generates megabytes of spam.
   - Engine enforces inline budget strictly (`stdout_bounded.len() <= budget`), while spooling complete output to CAS.
2. **Infinite Interactive Loop**:
   - Gremlin attempts to hang reading stdin.
   - Engine supplies closed stdin by default (`StdioMode::Closed`), causing gremlin to immediately read EOF (0 bytes) and terminate cleanly.
3. **Execution Timeout & Process-Tree Containment**:
   - Gremlin ignores SIGINT, creates background threads, and attempts to run forever.
   - Engine terminates process upon timeout expiry (`RuntimeStatus::TimedOut`).
   - On Windows, Windows Job Objects with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` ensure all descendant processes are terminated.
4. **Exit Code & Runtime Completion Separation**:
   - Hostile exit code (`42`) is preserved as `ProcessExit::code = Some(42)`, while runtime status is truthfully reported as `Completed`.
5. **Preflight Assurance Truthfulness**:
   - Demanding unsupported enforcement (e.g. filesystem `Enforced` when only `Observed` is available) is refused pre-flight (`CoreError::AssuranceNotSatisfied`).
