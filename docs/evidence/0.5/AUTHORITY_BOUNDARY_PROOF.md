# Omen 0.5 — Authority Boundary Proof

## 1. Architectural Doctrine

> **Omen is substrate, not sovereign.**

The surrounding division of responsibility is absolute:
- **Lantern** knows (durable memory, long-term context, and provenance).
- **Resolve** coordinates (live guard tokens and scope locks).
- **Tethers** controls (permission, trusted capability identity, policy, approval, durable intent, replay, and provider outcome truth).
- **Omen** makes the machine legible and enforceable (physical execution, containment, typed resources, machine-readable facts, and artifact CAS evidence).
- **ThreadMoth** deterministically mutates (bounded structural edits with cryptographic pre/post hashes and refusal).

Compact system statement:
> **Lantern knows. Resolve coordinates. Tethers controls. Omen makes the machine legible and enforceable. ThreadMoth mutates deterministically.**

---

## 2. Boundary Integrity Verification in 0.5

1. **No Competing Policy Engine**:
   `crates/omen-agent` and `crates/omen-mcp` do not decide user permissions or author durable security policies. When an action is proposed, it is either presented to the human for approval or executed subject to existing Omen engine constraints.
2. **No Invented Sovereign Memory**:
   `AgentContext` contains only current machine reality: working directory, git porcelain status, active facts, and recent execution summaries. It does not store conversational knowledge, long-term embeddings, or subjective memory (which belongs exclusively to Lantern).
3. **Deterministic Mutation Delegation**:
   File modifications are not performed by raw model writes; structural edits are routed through ThreadMoth with pre/post hashes.
4. **Strict Semantic Authority Gate (Default-Closed)**:
   Executable-name allowlisting is abolished. An action is never permitted merely because `tool == "git"`, `tool == "cargo"`, `tool == "exec"`, or `tool == "fs"`.
   - **Narrowly Permitted Safe Operations**: Read-only or reversible verification (`cargo check`, `cargo test`, `git status`, `git diff`, `git log`, read-only `fs stat`/`read`/`list`).
   - **Hostile / Destructive Proposals Refused**: Natural-language intent and Agent output never grant authority. Destructive operations (`git reset --hard`, `git clean -fd`, `git push --force`, `git branch -D`, arbitrary exec like `rm -rf`, filesystem deletions) are strictly refused by `InteractiveSession::is_permitted_agent_action`.
   - **Verified in Tests**:
     - `agent_lane_tests::test_authority_bypass_hostile_proposals_refused`: Proves hostile git/exec/fs proposals cannot cross the gate.
     - `agent_lane_tests::test_hostile_agent_proposal_does_not_execute_in_session`: Proves that when a hostile provider proposes `git reset --hard`, the session refuses execution and no execution record is written to the database.
5. **Substrate Legibility**:
   Omen reports truthful platform facts:
   - On Windows: Job Objects containment.
   - On Linux/macOS: Process group containment.
   - When sandboxing cannot be enforced, Omen reports `Observed` or `Unsupported`, never faking `Enforced`.
