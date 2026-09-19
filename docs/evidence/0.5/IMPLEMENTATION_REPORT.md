# Omen 0.5 Agent Interoperability — Implementation Report

**Release**: Omen 0.5.0  
**Target**: Agent Interoperability & Human-Agent Collaboration  
**Status**: VERIFIED & COMPLETE  
**Repository**: `https://github.com/matthewjameswatkins1978-cyber/Omen.git`  
**Branch**: `feature/omen-0.5-agent-interoperability`  

---

## 1. Executive Summary

Omen 0.2 established machine truth: typed resources, structured execution contracts, Fact Registry, dirty/current truth, Tool Atlas, CAS storage, and process containment.

Omen 0.3 gave that truth a human interface: interactive shell core with terminal capability detection, 3-lane grammar, fact-aware completion, subordinate physical history, and semantic blocks.

Omen 0.4 made that truth shared: two Omen shells, a CLI client, and an agent-facing client occupy the same workspace without each reconstructing their own private idea of machine reality.

**Omen 0.5 makes the shared runtime genuinely usable by software agents and introduces the first useful Agent-inside-Omen experience for humans.**

The headline proof is satisfied:
> **A human opens Omen, gets stuck on a real development task, asks Agent for help without copying terminal output anywhere, Agent sees the exact structured state of that session, explains what happened, proposes a useful next action, and performs permitted work through Omen without bypassing authority.**

External agents simultaneously gain a production-grade, standard Model Context Protocol (MCP) server over stdio and streams that exposes the shared runtime as tools and resources.

---

## 2. Core Architectural Doctrine

> **Omen is substrate, not sovereign.**

The division of responsibility remains inviolable:
- **Lantern** knows (durable memory, long-term context, and provenance).
- **Resolve** coordinates (live guard tokens and scope locks).
- **Tethers** controls (permission, trusted capability identity, policy, approval, durable intent, replay, and provider outcome truth).
- **Omen** makes the machine legible and enforceable (physical execution, containment, typed resources, machine-readable facts, and artifact CAS evidence).
- **ThreadMoth** deterministically mutates (bounded structural edits with cryptographic pre/post hashes and refusal).

Compact doctrine:
> **Lantern knows. Resolve coordinates. Tethers controls. Omen makes the machine legible and enforceable. ThreadMoth mutates deterministically.**

Omen 0.5 introduces zero competing policy, permission, lock, or approval systems. It exposes structured enforceability and machine truth to agents while remaining substrate.

---

## 3. Implemented Deliverables

### Slice A: Canonical Agent Context (`crates/omen-agent`)
- Structure: `AgentContext`
- Working directory, environment summary, platform metadata.
- Git repository awareness: active branch, commits ahead, modified files, staged files, untracked files.
- Recent physical execution and failure summaries: exit codes, execution IDs, durations, CAS artifact references (`artifact://sha256/...`), and bounded excerpts (<= 512 bytes).
- Structured active and dirty facts from Fact Registry and hot semantic index.
- Zero SQLite queries and zero subprocess executions on the interactive typing path; context is synthesized only when entering the AI reasoning lane (`?`).

### Slice B: Agent Provider Boundary (`crates/omen-agent`)
- Trait: `AgentProvider`
- Single async interface: `respond(AgentRequest) -> Result<AgentResponse, AgentError>`.
- `AgentResponse`: structured kind (`Explanation`, `Proposal`, `ActionRequest`, `Result`, `Question`, `Refusal`), markdown message, proposed actions, artifact references, and uncertainty explanation.
- `ProposedAction`: strongly typed actions:
  - `ChangeDirectory { path }`
  - `ExecuteTool { tool, operation, args, cwd }`
  - `ExecuteCommand { argv, cwd }`
  - `SemanticAction { action, args }`
- `TimeoutProvider`: universal 30-second ceiling wrapper. If a provider fails to respond within budget, execution returns `AgentError::Timeout`, leaving Omen state untouched and prompt responsive.
- `DiagnosticAgentProvider`: deterministic reasoning engine providing instantaneous, offline intelligence for common developer hurdles ("I'm lost", "why did that fail?", "what folder am I in?", "what is interesting in here?", directory navigation with ambiguity handling, and build checks).

### Slice C: Interactive Agent Lane Integration (`crates/omen-interactive`)
- Wired `agent_provider: Option<Arc<dyn AgentProvider>>` into `InteractiveSession`.
- User input starting with `?` dispatches to `AiLaneDispatcher::dispatch_with_session`.
- Formats responses in the canonical human-agent presentation:
  ```text
  Agent

  <agent markdown explanation>

  Proposed action:
    <action description or command>
  ```
- Identity: The assistant is strictly presented as `Agent`.
- Automatically executes safe proposed actions (e.g. `ChangeDirectory`, updating shell prompt and semantic completion index) and presents executable commands for user confirmation or daemon dispatch.
- **Session Scoping Preservation**: Querying Agent or running background agent tasks never mutates human session state or human `@last` reference.

### Slice D: Standard Model Context Protocol (MCP) Server (`crates/omen-mcp`)
- Binary: `omen-mcp` (also accessible via CLI subcommand `omen mcp`).
- Protocol: JSON-RPC 2.0 with MCP specification (`2024-11-05`).
- Tools exposed:
  1. `omen_workspace_status`: structured workspace snapshot, facts count, dirty count, managed services.
  2. `omen_facts_query`: filterable query of active/dirty facts.
  3. `omen_execute`: execution via shared daemon broker with CAS offload.
  4. `omen_execution_status`: query deduplication and completion status of background executions.
  5. `omen_history_query`: query subordinate physical execution history.
  6. `omen_services_list`: list managed background services.
  7. `omen_services_control`: start, stop, restart services.
  8. `omen_capabilities_discover`: inspect Omen physical containment, boundaries, and doctrine.
- Resources exposed:
  1. `artifact://sha256/<hash>`: bounded slice retrieval from CAS.
  2. `fact://<resource>`: read fact state.
  3. `proc://<name>`: read service status.
- Transport: stdio and async duplex streams (`run_stream`).

---

## 4. Verification & Proof Summary

| Proof ID | Focus | Test Target | Result |
|---|---|---|---|
| **Proof A** | Human "I'm lost" recovery | `agent_lane_tests::test_agent_lane_im_lost_proof_a` | **PASSED** |
| **Proof B** | Human "why did that fail?" diagnosis | `agent_lane_tests::test_agent_lane_why_did_that_fail_proof_b` | **PASSED** |
| **Proof C** | Real Human-Agent execution through shared broker | `agent_lane_tests::test_agent_lane_human_agent_build_proposal_proof_c` | **PASSED** |
| **Proof C (Exec)** | Real action executes through daemon once | `agent_lane_tests::real_human_agent_action_executes_through_daemon_once` | **PASSED** |
| **Proof C (Match)** | Execution ID matches history and receipt | `agent_lane_tests::agent_execution_id_matches_history_and_receipt` | **PASSED** |
| **Proof D** | MCP external agent tool & CAS artifact read | `mcp_tests::test_mcp_execute_and_read_cas_artifact_proof_d` | **PASSED** |
| **Isolation** | Subordinate session `@last` preservation | `agent_lane_tests::test_agent_lane_session_isolation_at_last` | **PASSED** |
| **Background**| Background agent work preserves human `@last` | `agent_lane_tests::agent_background_work_preserves_human_last` | **PASSED** |
| **Ambiguity** | Multi-candidate directory disambiguation | `agent_lane_tests::test_agent_lane_ambiguous_directory_navigation` | **PASSED** |
| **Navigation** | Unambiguous directory navigation | `agent_lane_tests::test_agent_lane_unambiguous_directory_navigation_executes` | **PASSED** |
| **Root/CWD** | Preserve workspace root separately from CWD | `agent_lane_tests::agent_context_preserves_workspace_root_after_cd` | **PASSED** |
| **Workspace ID**| Workspace ID remains stable across cd | `agent_lane_tests::agent_context_workspace_id_stable_after_cd` | **PASSED** |
| **CAS Root** | CAS directory resolves from workspace root | `agent_lane_tests::agent_context_cas_uses_workspace_root` | **PASSED** |
| **Bounded Git**| Stalling Git probe terminates at deadline | `agent_regression_tests::agent_git_probe_times_out_cleanly` | **PASSED** |
| **Timeout** | Agent context timeout does not hang shell | `agent_regression_tests::agent_context_timeout_does_not_hang_shell` | **PASSED** |
| **Zero Model** | Deterministic inquiries consume zero model calls | `agent_regression_tests::deterministic_question_uses_zero_model_calls` | **PASSED** |
| **Registry** | Provider registry selection & status | `agent_regression_tests::provider_registry_selects_configured_provider` | **PASSED** |
| **Real Switch**| Real interactive provider switching proof | `agent_lane_tests::test_provider_switching_real_interactive_proof` | **PASSED** |
| **Auth Gate** | Strict semantic authority gate hostile proposal refusal | `agent_lane_tests::test_authority_bypass_hostile_proposals_refused` | **PASSED** |
| **Hostile Exec**| Hostile proposal refuses execution in session | `agent_lane_tests::test_hostile_agent_proposal_does_not_execute_in_session` | **PASSED** |
| **0 Model Calls**| Trivial question uses zero provider calls | `agent_lane_tests::trivial_question_uses_zero_provider_calls` | **PASSED** |
| **0 Git Probes** | Trivial question uses zero git probes | `agent_lane_tests::trivial_question_uses_zero_git_probes` | **PASSED** |
| **0 DB Queries** | Trivial question uses zero DB context queries | `agent_lane_tests::trivial_question_uses_zero_db_context_queries` | **PASSED** |
| **Errors** | Provider failures translate to stable Omen errors | `agent_regression_tests::provider_failure_is_translated_to_omen_error` | **PASSED** |
| **Diagnostics**| Raw provider traces never leak into normal UX | `agent_regression_tests::provider_diagnostics_do_not_leak_into_normal_agent_output` | **PASSED** |
| **Protocol** | Incompatible MCP protocol version rejection | `mcp_tests::test_mcp_initialize_and_protocol_version` | **PASSED** |
| **Robustness**| Malformed JSON-RPC handling | `mcp_tests::test_mcp_malformed_json_rpc_handling` | **PASSED** |
| **Workspace** | Entire workspace verification & tests | `cargo test --workspace` & `cargo run -p xtask -- verify` | **PASSED** |
