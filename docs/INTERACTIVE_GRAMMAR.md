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

## 2. The Four Explicit Symbolic Lanes

Input lines are categorized by `GrammarScanner` into four explicit lanes:

```
GrammarScanner
  ├─ Executable       → existing argv process dispatch
  ├─ Portable shell   → Omen-owned shell parser and dispatcher
  ├─ Semantic action  → SemanticDispatcher
  └─ AI reasoning     → AiLaneDispatcher
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
- Bare `history`, `jobs`, and `stop <id>` reach the session displays without a colon; `cd` navigates.

### Lane 2: Portable Shell Expressions

Shell expressions are interpreted by Omen's Omen-owned syntax tree and dispatcher rather than a host shell:
```text
echo "$HOME" && cargo test
cd src; rg "TODO" *.rs
ls | grep rs | wc -l
sort < in.txt | uniq -c
server &
```
Ordered command lists and `&&` / `||` short-circuiting are supported. Expansion currently includes variables, tilde, and deterministic pathname globs. Twenty-five read-only builtins (`ls`, `cat`, `grep`, `sort`, …) run in-process with byte-preserving stdout and typed truth; all-builtin pipelines chain with zero spawns. `< file` feeds bytes; `> file` / `>> file` validate then refuse closed until the P2 filesystem authority lands. A trailing `&` starts a standalone background job (`:jobs` lists, `:stop job-N` stops). Every simple command goes through Omen's existing execution boundary. Unsupported syntax is refused, not passed through as argv. The supported subset and explicit refusals are listed above.

### Lane 3: Semantic Actions (Prefix: `:`)
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

Omen action-composition planning is read-only; this interactive shell does not
execute action plans. The portable shell lane currently executes ordered simple
commands separated by `;`, with `&&` / `||` short-circuiting. Each command is
dispatched separately through Omen's existing execution boundary; shell syntax
does not create or grant action authority.

The lane supports supervised byte pipelines, command-local environment
assignments, variable and tilde expansion, deterministic pathname globbing,
and the visible one-level argv aliases listed by `:aliases` (for example,
`g` expands to `git`). Pipeline stages and ordinary commands use Omen's
existing execution broker; shell syntax does not create or grant authority.

Command substitution and interactive terminal handoff inside a shell
expression are still refused as unsupported. fd-duplicating redirects
(`2>`, `>&fd`), output-file writes (validated, refused-closed pending
P2 authority), and boolean-chain background (`a && b &`) are refused
with explicit errors. Unsupported shell syntax is never passed through
as literal argv.

### Lane 4: Optional AI Reasoning Lane (Prefix: `?`)
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
