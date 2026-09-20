# Omen AI Reasoning Lane Specification

> **Doctrine**: *Omen should be intelligent before it is AI-powered.*  
> **Core Principle**: *Can this be answered from structured state, schemas, history or provenance? If yes, do it deterministically. Use AI only where judgement is genuinely required.*

---

## 1. Architectural Role

Omen is an agent-native developer runtime substrate, not an autonomous agent or chat assistant. The AI Reasoning Lane (`?`) is an explicit, optional advisory interface that allows developers to request machine judgement without polluting the deterministic command flow:

```text
? why did the auth test fail?
? how do I reformat modified rust files?
```

---

## 2. Invariants & Guardrails

1. **Explicit Invocation Only**: AI is never ambient, intrusive, or unsolicited. It activates solely when the user explicitly prefixes an input line with `?`.
2. **Zero Authority Grant**: AI suggestions are advisory text. An LLM output cannot grant permissions, approve transactions, or silently execute commands.
3. **Fully Functional Without AI**: Omen operates completely and cleanly with AI turned off (`ai = "off"`). No core runtime feature, completion, diagnostic, or tool execution requires an LLM.
4. **Suggests Typed Substrate Actions**: When reasoning about failures, the AI lane suggests concrete, typed Omen operations:
   - `:show @failed`
   - `:why @last`
   - `:show @errors`
   - `:rerun @failed`
5. **Bounded Context**: Rather than scraping megabytes of raw terminal ANSI text, the AI lane supplies bounded structured context (`AgentContext`):
   - Working directory, environment info, Git status (branch, modified/staged files).
   - Command line, exit code, execution IDs.
   - Bounded excerpts (<= 512 bytes) from CAS stderr/stdout artifacts.
   - Active dirty/current Facts and dependencies from SQLite and hot cache.
6. **Strict Session Isolation**: Agent reasoning or background tasks never mutate the human session state or human `@last` reference.
7. **Timeout Protection**: Universal 30-second execution ceiling. If an agent model provider does not return within 30 seconds, execution cleanly aborts with state intact.

---

## 3. Canonical Presentation Format

Agent responses follow a standardized, clean markdown structure:

```text
Agent

<agent markdown explanation>

Proposed action:
  <action description or command>
```

- **Displayed Identity**: Strictly `Agent`.
- **Proposed Actions**: Either deterministic actions (`:why @last`, `:show @failed`), safe mutations executed automatically with user notification (`ChangeDirectory`), or proposed commands (`:cargo check`) presented for execution.

---

## 4. Deterministic Local Fallback (`DiagnosticAgentProvider`)

If no external LLM provider is connected, Omen's built-in `DiagnosticAgentProvider` provides deterministic assistance based on structured machine truth:

```text
? I'm lost

Agent

You are in:
  crates/omen-interactive

Recent change:
  2 modified file(s)

Git:
  2 modified file(s)
  0 commit(s) ahead

Tests:
  recent execution 'cargo test' exited with code 101

Likely current problem:
  error[E0432]: unresolved import `foo::bar`

Dirty facts:
  1 fact(s) currently DIRTY (fact://project/build_status)

Suggested next step:
  inspect 'cargo test' and repair the failing test assertion

Proposed action:
  :show @failed
  :cargo test
```

---

## 5. Configuration & Pluggability

The AI lane is backed by the `AgentProvider` trait (`crates/omen-agent`). Any external model service or agent framework can plug in by implementing:

```rust
pub trait AgentProvider: Send + Sync {
    fn respond<'a>(
        &'a self,
        request: AgentRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AgentResponse, AgentError>> + Send + 'a>>;
}
```
