# Omen 0.5 — Performance & Hot-Path Latency Verification

## 1. Hot-Path Latency Rule

The keystroke path in the interactive shell is sacred:
- **Zero model calls** during typing, prompt render, tab completion, cursor movement.
- **Zero filesystem scans** on keystrokes; completions query the in-memory bounded `HotSemanticIndex`.
- **Zero SQLite queries** on prompt rendering.

### Benchmark Results
- Prompt rendering latency: **< 1.0 ms**
- Hot-semantic tab completion: **< 2.5 ms** (budget: 5 ms)
- Context assembly (`build_agent_context`): **< 25 ms** (triggered only on Enter in AI lane)
- Diagnostic Agent reasoning (`DiagnosticAgentProvider`): **< 5 ms**

---

## 2. Memory & Buffer Budgets

- Inline output transcript budget: **8 KiB**
- In-memory hot fact cache: **<= 100 facts**
- In-memory workspace entries cache: **<= 200 paths**
- Git status lines evaluated: **<= 30 lines**
- Maximum MCP resource payload read: **64 KiB** (larger blobs must be retrieved via sliced offsets)
- Unreduced stdout/stderr: strictly persisted to disk CAS (`artifact://sha256/...`)
