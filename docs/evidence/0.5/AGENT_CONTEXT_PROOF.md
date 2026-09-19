# Omen 0.5 — Agent Context Proof

## 1. Principle & Requirements

An AI agent assisting a human developer in Omen must never be starved of machine truth, nor should it be drowned in unbounded raw text dumps.

Omen 0.5 establishes `AgentContext` in `crates/omen-agent`:
1. **Zero Hot-Path Overhead**: As long as the user is typing, receiving completions, or inspecting the prompt, zero model calls, zero Git subprocess spawns, and zero SQLite database queries take place.
2. **On-Demand Synthesis**: `build_agent_context` executes strictly when the user dispatches an AI query via the `?` grammar lane or when an MCP tool request arrives.
3. **Bounded Context**: Transcripts and output blobs are never passed raw. Excerpts are bounded to 512 bytes (`AgentContext::bounded_excerpt`), with full unreduced evidence referenced by Content-Addressed Storage URI (`artifact://sha256/...`).
4. **Structured Facts & History**: Dirty and current facts from the Fact Registry and hot semantic index are directly exposed as typed records.

---

## 2. Structure Definition

```rust
pub struct AgentContext {
    pub cwd: PathBuf,
    pub session_id: String,
    pub git: Option<GitStatusInfo>,
    pub recent_execution: Option<ExecutionSummary>,
    pub recent_failed_execution: Option<ExecutionSummary>,
    pub dirty_facts: Vec<FactInfo>,
    pub current_facts: Vec<FactInfo>,
    pub services: Vec<ManagedServiceInfo>,
    pub env: EnvironmentInfo,
}
```

---

## 3. Verified Proofs

### Proof 1: Git Status & Branch Awareness
- `GitStatusInfo` inspects:
  - Active branch (`git rev-parse --abbrev-ref HEAD`)
  - Remote origin (`git config --get remote.origin.url`)
  - Working tree status (`git status --porcelain`) bounded to 30 lines.
- Verified in `agent_lane_tests::test_agent_lane_im_lost_proof_a`:
  Active branch and modified count are observed and reported cleanly in human diagnosis.

### Proof 2: Bounded Excerpts & CAS References
- When commands fail, `build_agent_context` locates the execution's stderr CAS artifact via the canonical workspace directory.
- Reads at most 512 bytes of stderr as an inline preview, leaving the full blob safely in CAS.
- Verified in `agent_lane_tests::test_agent_lane_why_did_that_fail_proof_b`:
  Failure diagnosis identifies the exact root cause (`error[E0308]: mismatched types`) without dumping megabytes of compiler output.

### Proof 3: Dirty Fact Visibility
- Facts invalidated by workspace mutations transition from `CURRENT` to `DIRTY`.
- `build_agent_context` queries both SQLite persistent registry and in-memory hot cache.
- Verified in `agent_lane_tests::test_agent_lane_im_lost_proof_a`:
  When a workspace file changes, dirty facts (e.g. `fact://project/build_status`) are explicitly reported in Agent's situation assessment.
