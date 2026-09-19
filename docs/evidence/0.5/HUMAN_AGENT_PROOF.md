# Omen 0.5 — Human-Agent Collaboration Proof

## 1. Mission

The primary user experience milestone of Omen 0.5 is:

> **A human opens Omen, gets stuck on a real development task, asks Agent for help without copying terminal output anywhere, Agent sees the exact structured state of that session, explains what happened, proposes a useful next action, and performs permitted work through Omen without bypassing authority.**

---

## 2. Proof Walkthroughs

### Proof A: "I'm lost" Recovery
- **Scenario**: A developer returns to a terminal session after interruption. Unstaged changes exist in Git, a recent test run failed, and a project fact is dirty.
- **Input**: `? I'm lost`
- **Agent Assessment**:
  1. Identifies working directory.
  2. Summarizes recent Git status (branch and modified files).
  3. Detects recent failed execution (`cargo test` exited with code 101).
  4. Identifies dirty facts (`fact://project/build_status`).
  5. Proposes next step: inspect failure (`:show @failed`) and suggests rerun.
- **Automated Test**: `agent_lane_tests::test_agent_lane_im_lost_proof_a` (PASSED).

### Proof B: "why did that fail?"
- **Scenario**: A command fails with compiler errors. The human asks for an explanation.
- **Input**: `? why did that fail?`
- **Agent Assessment**:
  1. Inspects last failed execution in the session history.
  2. Extracts the exact compile error from the CAS stderr artifact (`mismatched types: expected u32, found &str`).
  3. Cites the CAS artifact URI.
  4. Suggests diagnostic actions: `:why @last` and `:show @failed`.
- **Automated Test**: `agent_lane_tests::test_agent_lane_why_did_that_fail_proof_b` (PASSED).

### Proof C: Real Human-Agent Execution Proof
- **Scenario**: The human asks Agent to verify whether the project builds.
- **Input**: `? check whether this project still builds`
- **Execution Path**:
  1. Human inputs query in the interactive shell.
  2. Agent diagnoses workspace manifest (`Cargo.toml`) and produces proposed action `ExecuteTool { tool: "cargo", operation: "check", args: [] }`.
  3. `InteractiveSession::is_permitted_agent_action` evaluates the proposed action as permitted without secondary ceremony.
  4. `InteractiveSession::execute_via_broker` routes execution through Omen's real shared daemon broker (`client.submit_execution(...)`).
  5. Exactly one physical execution occurs via the shared daemon `ProcessSupervisor`.
  6. Exactly one `ExecutionId` is minted.
  7. Exactly one `ExecutionRecord` is written to canonical SQLite.
  8. Output is spooled to Content-Addressed Storage (`CAS`).
  9. Command exit status (0) is reported to the human.
  10. Session-scoped `@last` in that human session resolves directly to `cargo check`.
  11. Zero duplicate executions are performed.
- **Automated Tests**:
  - `agent_lane_tests::test_agent_lane_human_agent_build_proposal_proof_c` (PASSED).
  - `agent_lane_tests::real_human_agent_action_executes_through_daemon_once` (PASSED).
  - `agent_lane_tests::agent_execution_id_matches_history_and_receipt` (PASSED).

### Proof D: Preserved Workspace Root Across Directory Navigation
- **Rule**: Navigating the filesystem (`cd`) alters session `cwd` but never mutates `workspace_root`, `workspace_id`, or CAS store location.
- **Verification**:
  - Session starts at workspace root.
  - User executes `cd crates/test-sub`.
  - `session.cwd` points to `crates/test-sub`, while `session.workspace_root` remains strictly preserved at the top-level repository.
  - Subsequent agent queries construct `AgentContext` reflecting both locations cleanly.
  - CAS storage resolves to `<workspace_root>/.omen/workspaces/<ws_id>/cas`.
- **Automated Tests**:
  - `agent_lane_tests::agent_context_preserves_workspace_root_after_cd` (PASSED).
  - `agent_lane_tests::agent_context_workspace_id_stable_after_cd` (PASSED).
  - `agent_lane_tests::agent_context_cas_uses_workspace_root` (PASSED).

### Proof E1: Strict Subordinate Session Isolation
- **Rule**: Background agent work or agent queries must never mutate human session state or human `@last`.
- **Scenario**:
  1. Human executes `git status` under session `sess-human`.
  2. Background agent executes `npm run build` under session `sess-agent`.
  3. Human asks `? what folder am I in?`.
- **Verification**:
  `ReferenceResolver::resolve("@last", &human_session, &db)` continues to return `git status`. Background agent commands and agent reasoning calls leave human session context strictly isolated.
- **Automated Tests**:
  - `agent_lane_tests::test_agent_lane_session_isolation_at_last` (PASSED).
  - `agent_lane_tests::agent_background_work_preserves_human_last` (PASSED).

### Proof E2: Directory Disambiguation Without Guessing
- **Rule**: When an instruction has multiple legitimate interpretations, Agent must not guess silently. It must ask a clarifying question listing the concise alternatives.
- **Scenario**: Workspace contains both `src/components` and `tests/components`.
- **Input**: `? switch to components`
- **Verification**:
  Agent refuses to pick arbitrarily, returning `AgentResponseKind::Question` listing `1. src/components` and `2. tests/components`, and asking the user to choose.
- **Automated Test**: `agent_lane_tests::test_agent_lane_ambiguous_directory_navigation` (PASSED).
