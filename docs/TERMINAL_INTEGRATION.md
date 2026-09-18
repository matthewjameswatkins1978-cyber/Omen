# Omen Terminal Integration Specification

> **Doctrine**: *Terminal protocols enhance rendering and input; they are not the semantic model.*  
> **Terminal Strategy**: *Inhabit existing terminal emulators. Do not build a proprietary terminal.*

---

## 1. Structured Semantic Blocks

Every execution in Omen possesses a typed domain identity independently of terminal styling:

```text
cargo test auth · op_8f12
13 passed · 1 failed · 1.12s
FAIL tests/auth.rs:88
artifact://sha256/3c8f...
```

- **The Box is Optional; The Structured Object is Not**:
  - The execution block exists as a strongly typed data structure in memory.
  - It can be rendered to ordinary ANSI terminals, enhanced OSC-aware terminals, an MCP client, a web UI, or machine-readable JSON logs.
  - The terminal UI must **never** become the source of truth; terminal text is an ephemeral projection of underlying runtime truth.

---

## 2. Terminal Escape Protocols & Progressive Enhancement

Omen dynamically interrogates terminal capabilities on startup (`TerminalCapabilities::detect()`) and negotiates features:

### 2.1 Baseline Support
- **Standard VT/ANSI & Stdio**: Supported across all POSIX and Windows console hosts.
- **Unicode with ASCII Fallback**: If `NO_COLOR=1` or `TERM=dumb`, glyphs degrade to plain ASCII (`ok`, `X`, `dirty`).

### 2.2 Operating System Command (OSC) Extensions
Where supported (Windows Terminal, WezTerm, Alacritty, Kitty, iTerm2, VSCode Integrated Terminal):
- **OSC 7 (Working Directory Sync)**:
  Emits `\x1b]7;file://localhost/<escaped_path>\x1b\` on directory transitions, keeping terminal tabs and windows in sync.
- **OSC 8 (Hyperlinked Artifacts)**:
  Emits clickable hyperlinks for CAS digests: `\x1b]8;;artifact://sha256/<hash>\x1b\<hash>\x1b]8;;\x1b\`. Clicking opens the CAS artifact viewer.
- **OSC 133 / FTCS (Semantic Shell Boundaries)**:
  - `\x1b]133;A\x1b\` — Prompt start
  - `\x1b]133;B\x1b\` — Command input start
  - `\x1b]133;C\x1b\` — Command execution start
  - `\x1b]133;D;{exit_code}\x1b\` — Command completion with exit status
  This allows host terminals to navigate between command outputs, fold blocks, and inspect exit codes.

### 2.3 Kitty Keyboard Protocol Roadmap
Modern terminals increasingly support the Kitty keyboard protocol for unambiguous modifier keys (differentiating `Ctrl+I` from `Tab`, detecting key-up events, etc.).
- Omen will negotiate Kitty protocol support where available via terminal queries.
- Legacy terminal input processing remains fully supported as a permanent fallback.

---

## 3. Interactive Child Process Handoff

Developer workflows frequently involve interactive programs that require direct TTY ownership:
- Text editors: `vim`, `nvim`, `nano`, `emacs`, `micro`.
- Pagers: `less`, `more`, `man`.
- Interactive REPLs: `python`, `node`, `ghci`, `irb`.
- Remote shells: `ssh`, `tmux`.
- Git interactive subcommands: `git commit` (without `-m`), `git rebase -i`, `git add -p`.

### 3.1 Invocation-Specific Classification
Omen rejects simplistic executable-name matching. An invocation is classified based on its full argument list:
- `python script.py` ➔ Standard supervised execution (closed stdin, bounded output, CAS capture).
- `python` ➔ Interactive child handoff.
- `git commit -m "feat: msg"` ➔ Supervised execution.
- `git commit` ➔ Interactive child handoff (opens configured editor).
- Explicit flag override: Any command can be forced into handoff mode using `--interactive` or `--handoff`.

### 3.2 Terminal Lifecycle Handover
When an interactive child is spawned:
1. Reedline suspends terminal raw mode and restores terminal settings.
2. Standard input, output, and error handles are connected directly to the inherited console.
3. The process runs to completion with uninhibited human interaction.
4. Upon process termination, Omen reacquires console ownership, re-enables raw mode, captures duration and exit code, and records the event in `execution_history`.
