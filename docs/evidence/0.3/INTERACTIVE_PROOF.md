# Omen 0.3 — Interactive Interface Proof

**Version**: Omen 0.3.0  
**Test Harness**: `crates/omen-interactive/tests/` & `crates/omen-ui/tests/`  
**Target Environment**: Windows 11 x86_64, Linux, macOS  

---

## 1. Overview

This document demonstrates the interactive capabilities implemented in Omen 0.3, detailing the terminal behaviors, semantic grammar parsing, diagnostic progressive disclosure, and protective mechanisms.

---

## 2. Interactive REPL & Terminal Lifecycle

### 2.1 Prompt Rendering and Terminal Capabilities
Omen detects terminal capabilities on startup using `TerminalCapabilities::detect()`:
- **Rich Terminal** (e.g. Windows Terminal, WezTerm, Alacritty):
  - Emits OSC 133 semantic prompt anchors: `\x1b]133;A\x1b\` (prompt start), `\x1b]133;B\x1b\` (command input start), `\x1b]133;C\x1b\` (execution start), `\x1b]133;D;{exit}\x1b\` (command finish).
  - Emits OSC 7 current working directory: `\x1b]7;file://localhost/... \x1b\`.
  - Emits OSC 8 hyperlinks for CAS artifact hashes.
  - Displays Unicode status indicators (`✓` clean, `! N dirty` warning, `✕` failure).
- **Dumb Terminal** (`TERM=dumb` or `NO_COLOR=1`):
  - Strips all ANSI styling and escape sequences.
  - Replaces Unicode glyphs with ASCII equivalents (`ok`, `dirty N dirty`, `X`).

### 2.2 Child Process Handoff
When an interactive command is entered (e.g. `vim`, `nano`, `less`, `more`, `man`, `git commit`):
1. `ChildHandoff::is_interactive_command(cmd)` matches the binary name.
2. The Reedline line editor suspends raw mode and releases stdin/stdout.
3. The process is spawned with inherited standard input, output, and error streams.
4. Upon process exit, the terminal state is restored, raw mode is re-enabled, and the exit code is captured into prompt history.

---

## 3. Semantic Grammar (3 Lanes)

Omen rejects ambiguous string parsing in favor of a 3-lane grammar scanned by `GrammarScanner`:

```
┌─────────────────────────────────────────────────────────────┐
│                       GrammarScanner                        │
└──────────────┬───────────────────┬───────────────────┬──────┘
               │                   │                   │
               ▼                   ▼                   ▼
      Executable Lane     Semantic Action Lane    AI Lane
       "cargo test"            ":status"        "? why failed"
             │                     │                   │
    ProcessSupervisor     SemanticDispatcher   AiLaneDispatcher
```

### Lane 1: Executable Lane
Standard shell command invocations:
```bash
cargo build --release
git status --short
```
- Resolved dynamic typed references (`@last`, `@failed`).
- Preflight blast-radius inspection.
- Supervised execution with stdout/stderr spooled directly to CAS artifacts.

### Lane 2: Semantic Action Lane (`:`)
Direct machine substrate control without process overhead:
- `:status` — Workspace health, dirty facts count, active services.
- `:doctor` — Substrate integrity and tool atlas discovery.
- `:tools` — Registered tool profiles and versions.
- `:inspect <target>` — Structured inspection of execution records or facts.
- `:why <target>` — Deep causal explanation of fact validity.
- `:history` — Interactive execution history for the current session.
- `:rerun <target>` — Re-executes previous or failed commands.
- `:services` — Lists running background services.
- `:stop <service>` — Gracefully terminates a managed background service.

### Lane 3: AI Reasoning Lane (`?`)
Advisory assistance with deterministic local fallback:
```bash
? why did the last build fail
```
- Inspects recent failure artifacts (`stderr_artifact`) and fact registry state.
- Suggests concrete Omen commands (`:show @failed`, `:why @last`, `:rerun @failed`).

---

## 4. Subordinate Physical History & Reference Resolution

Execution history is stored in SQLite within `execution_history`, scoped strictly by `InteractiveSessionId`:

| Reference | Resolution Logic | Example Output |
| :--- | :--- | :--- |
| `@last` | Most recent execution command in current session | `cargo test auth` |
| `@last.failed` | Most recent non-zero execution command | `cargo test auth` |
| `@last.artifact` | Most recent stdout or stderr CAS artifact URI | `artifact://sha256/a1b2...` |
| `@failed` | Most recent non-zero exit command | `cargo test auth` |
| `@errors` | Stderr artifact URI of the last failed execution | `artifact://sha256/f9e8...` |

---

## 5. Protective Controls

### 5.1 Blast-Radius Preflight
Before executing destructive commands, Omen computes the blast radius:
- `rm -rf <path>`: Severity `CRITICAL`, displays targets to be destroyed.
- `cargo clean`: Severity `HIGH`, displays build target directory removal.
- `git reset --hard`: Severity `HIGH`, displays uncommitted work loss.
- `threadmoth mutate`: Severity `MEDIUM`, structural file mutation warning.

### 5.2 Multiline Paste Guard
When a multiline buffer is pasted into the terminal, `PasteGuard`:
1. Intercepts the buffer before execution.
2. Displays the number of detected lines and formats each line with a numbered index.
3. Requires explicit human review before multi-command dispatch.

---

## 6. Progressive Diagnostics (Levels 0–3)

Output diagnostics scale based on context:

### Level 0 (Minimal)
```
✓ cargo test --all (exit 0, 340ms)
```

### Level 1 (Compact)
```
✓ Command: cargo test --all
  Outcome: SUCCESS
  Errors:  artifact://sha256/error1
```

### Level 2 (Detailed)
```
--- Execution Diagnostic: exec-diag-1 ---
  Session:   sess-1
  Command:   cargo test --all
  Exit Code: Some(0)
  Duration:  340 ms
  Stdout CAS: artifact://sha256/abcd
  Recorded:  2026-09-18T14:00:00Z
```

### Level 3 (Machine JSON)
```json
{
  "execution_id": "exec-diag-1",
  "session_id": "sess-1",
  "command": "cargo test --all",
  "exit_code": 0,
  "duration_ms": 340,
  "stdout_artifact": "artifact://sha256/abcd",
  "stderr_artifact": null,
  "created_at": "2026-09-18T14:00:00Z"
}
```
