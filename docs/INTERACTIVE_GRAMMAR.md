# Omen Interactive Grammar Specification

> **Doctrine**: *Omen must not invent a new general-purpose scripting language.*  
> **Core Principle**: *Infer what has already been decided. Never invent what has not.*

---

## 1. Canonical Interaction Hierarchy

The human interface operates across four distinct semantic tiers:

```text
cargo test auth          <-- 1. Raw Executable: Direct PATH dispatch, argv-only, closed stdin
:test auth               <-- 2. Omen Semantic Operation: Substrate action dispatched via Tool Atlas
:why @last               <-- 3. Deterministic Explanation: Provenance inspection over known Omen truth
? why is auth flaky?     <-- 4. Optional AI Reasoning: Explicit advisory judgement
```

---

## 2. The Three Explicit Symbolic Lanes

Input lines are categorized lexically by `GrammarScanner` into three explicit symbolic lanes:

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

### Lane 1: Ordinary Executables (Prefix: None)
Standard shell commands dispatched directly to host PATH:
```bash
cargo build --release
git status --short
rg "TODO" src/
```
- Spawns process using argv-only execution with closed stdin by default.
- Standard output and error streams are captured, bounded, and spooled to SHA-256 CAS artifacts (`artifact://`).
- Emits structured execution block and updates session history.

### Lane 2: Semantic Actions (Prefix: `:`)
Direct, typed operations on the Omen runtime substrate:
```text
:status                  # Workspace cleanliness, dirty facts, active services
:doctor                  # Tool atlas validation and substrate health
:tools                   # Registered tool profiles, versions, and capabilities
:test <target>           # Dispatches semantic test runner via Tool Atlas profile
:inspect <ref>           # Structured inspection of execution records, facts, or tools
:why <ref>               # Deterministic causal explanation of fact invalidation
:history [filter]        # Subordinate physical history for current session
:rerun <ref>             # Re-runs a previous or failed command
:show <ref>              # Displays bounded stdout/stderr or CAS artifact summary
:orient                  # Canonical contract orientation
:capabilities [group]    # Canonical capability projections
:describe <capability>  # One capability definition and live status
:how <recipe>            # Advisory recipe projection
:actions                 # Workspace action listing
:plan <action>           # Read-only action plan
:services                # Lists background managed processes (proc://)
:stop <service>          # Gracefully terminates a managed service
:restart <service>       # Restarts a managed background service
```

Composition planning is read-only. Interactive composition execution is not
exposed; consequential execution remains on the explicitly contract-bound CLI
surface.

### Lane 3: Optional AI Reasoning Lane (Prefix: `?`)
Advisory reasoning invoked only when explicit human judgement is requested:
```text
? why did the last test fail?
? suggest a command to reformat modified files
```
- Advisory only: produces suggestions of typed Omen commands (`:show @failed`, `:why @last`).
- Never executes commands silently or bypasses Tethers authorization.
- Functions with deterministic local fallback when no LLM provider is configured.

---

## 3. Typed References (Prefix: `@`)

References provide deterministic handles into current runtime state, not natural-language guesses:

| Reference | Target Meaning | Example Target |
|---|---|---|
| `@last` | Most recent execution in current interactive session | `cargo test auth` |
| `@last.failed` | Most recent non-zero execution in current session | `cargo test auth` |
| `@last.artifact` | Most recent CAS artifact URI (stdout/stderr) | `artifact://sha256/3a1f...` |
| `@last.changed` | Files modified during the last execution | `src/auth.rs` |
| `@last.output` | Bounded text output of the last execution | (4 KiB preview) |
| `@failed` | Alias for `@last.failed` command | `cargo test auth` |
| `@errors` | Stderr CAS artifact of the most recent failure | `artifact://sha256/e90b...` |
| `@fact.test` | Current test status Fact in Fact Registry | `fact://test/status` |
| `@service.<name>` | Managed background service resource | `proc://service/dev` |

---

## 4. Resolution Rules

When resolving human input to execution semantics, Omen adheres to strict resolution precedence:

1. **Exact Explicit Input**: If input specifies an exact command, tool, or action, dispatch it immediately.
2. **One Deterministic Interpretation**: If input uniquely resolves against known tools or grammar rules, resolve deterministically.
3. **Multiple Legitimate Interpretations**: Offer explicit disambiguation choices to the user.
4. **Judgement Required**: Route to the AI advisory lane (`?`) or wait for user decision.

> **Rule on Fuzzy Matching**: Fuzzy matching may rank options in completion and suggestion menus. **Fuzzy matching must never silently decide execution semantics or grant authority.**

---

## 5. Cross-Platform Path Grammar

Omen rejects importing Bash-specific quoting or escape rules into portable grammar:
- **Windows Drive Letters**: `C:\path\to\file` is preserved as a contiguous path token, not parsed as an action `:` or escaped.
- **UNC Paths**: `\\server\share\file` is preserved without backslash stripping.
- **Quoted Paths**: `"C:\Program Files\App\bin.exe"` preserves embedded spaces cleanly.
- **Literal & Trailing Backslashes**: Directory paths such as `src\auth\` maintain their exact trailing backslash without escaping subsequent characters.
- **Incomplete Quotes**: Handled gracefully without panics or undefined parse states.
