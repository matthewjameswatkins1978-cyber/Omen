# Omen 0.4 Evidence — Human & Agent Coexistence Proof

**Document**: `docs/evidence/0.4/HUMAN_AGENT_PROOF.md`  
**Status**: VERIFIED  
**Crates**: `omen-interactive`, `omen-daemon`, `omen-knowledge`

---

## 1. Shared Reality Across Humans and Agents

In Omen 0.4, an interactive human user running `omen` and an autonomous agent submitting commands via IPC operate within the same workspace reality:
- **Shared Hot Semantic Index**: The daemon maintains the authoritative index of facts, tools, services, and dirty counts.
- **Out-of-Band Fact Updates**: When an agent mutates a file or invalidates a fact (`FactInvalidated`), the human shell's prompt updates its dirty count in real time (`[shared] ~/Projects/Omen 1* >`) without requiring a manual shell refresh or restart.
- **Non-Interactive Execution Routing**: Non-interactive command dispatches in the human shell route once through the shared daemon broker, ensuring the daemon and all connected clients immediately observe execution receipts and results.

---

## 2. Invariant: `@last` Remains Strictly Session-Scoped

While machine facts and physical history are shared across all processes, interactive shell actor context must never be corrupted by foreign processes:
- If Agent A executes `cargo check` in the background, a human typing `@last` in Shell 1 does **not** get `cargo check`.
- `@last` is resolved strictly with `InteractiveSessionId` against the subordinate physical history stored in SQLite.
- Two human shells open to the same directory maintain independent `@last` pointers while sharing the exact same underlying facts and services.

### Automated Test Evidence
- `crates/omen-interactive/tests/real_interactive_shell_shared_tests.rs`:
  - `test_real_interactive_shell_observes_remote_fact_dirty`: Proves real interactive shell receives remote invalidation and reflects dirty facts in its prompt adapter.
  - `test_real_interactive_shell_routes_execution_through_daemon_once`: Proves execution routes once through shared daemon broker, and writes exactly one execution history record into the canonical database.
  - `test_real_interactive_shell_last_remains_session_scoped`: Proves two interactive shells attached to the same daemon and workspace execute different commands, and `@last` returns strictly session-scoped values (`cargo --version` vs `cargo --help`).
- `crates/omen-daemon/tests/history_scoping_tests.rs`:
  - `test_shared_subordinate_history_and_session_scoped_last`: Proves broker queries for last execution are filtered by session ID.
- `crates/omen-interactive/tests/h10_shared_reality_proof_tests.rs`:
  - `test_human_agent_shared_reality_and_session_isolation`: Comprehensive human + agent coexistence test.
