# Omen Semantic History Specification

> **Doctrine**: *Personal interaction history and shared machine reality are different concepts.*  
> **Boundary Rule**: *This is subordinate physical execution history, not a competing authoritative operation journal. Tethers still owns durable execution intent and authoritative replay/outcome semantics.*

---

## 1. Subordinate Physical Execution History

Traditional shells store execution history as a flat array of human-typed strings (e.g. `~/.bash_history`). Omen indexes structured physical runtime evidence in an embedded SQLite database (`execution_history` table):

- **Execution ID**: Machine action UUID (e.g. `omen://action/...`).
- **Actor URI**: Explicit executing identity (e.g. `actor://human/matthew/session-1` or `actor://agent/codex`).
- **Interactive Session ID**: Unique session UUID (`InteractiveSessionId`).
- **Entered Text**: Raw string submitted by the user.
- **Resolved Operation**: Parsed semantic command or tool invocation.
- **Argv Array**: Exact JSON-serialized arguments passed to process spawn.
- **Workspace Directory**: Absolute path of working directory at execution time.
- **Timestamp & Duration**: Start timestamp and elapsed wall-clock milliseconds.
- **Exit Status**: Exit code, termination signal, or spawn failure.
- **Artifact Handles**: URIs to full stdout/stderr CAS artifacts (`artifact://sha256/...`).
- **Fact & Assurance State**: Associated facts modified or invalidated, and actual platform assurance achieved (`ENFORCED`, `OBSERVED`).

---

## 2. `@last` Semantics & Actor Scoping

A critical architectural lesson from early design exploration:
> **`@last` must NOT mean "the most recent execution by anybody".**

In an agent-native environment, an autonomous agent may execute dozens of file transformations, test runs, or inspections in the background. If `@last` were global, the human's "up-arrow" or `@last` would unpredictably execute the agent's internal commands, destroying human workflow continuity.

### The Invariant Rule:
> **`@last` means: The most recent applicable operation initiated by the current interactive actor in the current interactive session.**

- **Personal Interaction History**: Scoped strictly by `InteractiveSessionId` in SQLite queries.
- **Shared Machine Reality**: Facts, resource generations, and CAS artifacts are shared across all processes. When an agent mutates files, facts transition to `DIRTY` across all sessions, but `@last` remains undisturbed.

### Future Explicit Scopes:
Future releases will support explicit qualified handles:
```text
@session.last             # Explicit current session handle (default @last)
@workspace.last           # Most recent execution in workspace by any actor
@agent.<id>.last          # Most recent operation by specific background agent
```
Ordinary `@last` and `@failed` remain strictly personal to the interactive human session.

---

## 3. Dynamic Reference Resolution

The `ReferenceResolver` evaluates dynamic `@` handles deterministically against SQLite session history:

| Dynamic Handle | SQL Resolution Predicate | Return Value |
|---|---|---|
| `@last` | `WHERE session_id = ? ORDER BY id DESC LIMIT 1` | Entered command string |
| `@last.failed` | `WHERE session_id = ? AND exit_code != 0 ORDER BY id DESC LIMIT 1` | Failed command string |
| `@failed` | Alias for `@last.failed` | Failed command string |
| `@last.artifact` | `SELECT stdout_artifact, stderr_artifact WHERE session_id = ? ORDER BY id DESC LIMIT 1` | `artifact://sha256/...` |
| `@errors` | `SELECT stderr_artifact WHERE session_id = ? AND exit_code != 0 ORDER BY id DESC LIMIT 1` | `artifact://sha256/...` |
| `@last.changed` | Files modified during `@last` execution | Array of file paths |

If no matching record exists in the current session, the resolver returns an explicit error (`No recent execution in current session`) rather than guessing or searching globally.

---

## 4. Subordinate vs Authoritative Replay

Omen's execution history is strictly **subordinate physical history**:
- It records physical evidence of what executed on the machine.
- It enables local human conveniences (`:rerun @last`, `:history`, `:show @failed`).
- **Authority Invariant**: Omen may deterministically reconstruct typed execution requests from subordinate history, but all consequential execution must be re-admitted under current Tethers authority. Omen never creates a competing authoritative replay or audit journal.
