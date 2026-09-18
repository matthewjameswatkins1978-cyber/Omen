# Omen 0.3 — Platform & Terminal Compatibility Matrix

**Version**: Omen 0.3.0  
**Target Platforms**: Windows, Linux, macOS  

---

## 1. Platform Enforcement & Truthfulness Doctrine

Rule 7 of the Omen Operational Doctrine dictates:
> **Platform truthfulness**: If a platform cannot enforce a constraint (such as sandboxing), it reports `OBSERVED` or `UNSUPPORTED`. It never fakes `ENFORCED`.

This principle extends to terminal capabilities, process supervision, and file-change notifications.

---

## 2. Terminal Support Matrix

| Terminal / Host | Platform | Colors | Unicode | OSC 7 (cwd) | OSC 8 (links) | OSC 133 (prompts) | Status |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Windows Terminal** | Windows | Truecolor | Full | Supported | Supported | Supported | **Tier 1 (Full)** |
| **VSCode Integrated** | Win/Lin/Mac | Truecolor | Full | Supported | Supported | Supported | **Tier 1 (Full)** |
| **Alacritty** | Win/Lin/Mac | Truecolor | Full | Supported | Supported | Supported | **Tier 1 (Full)** |
| **WezTerm** | Win/Lin/Mac | Truecolor | Full | Supported | Supported | Supported | **Tier 1 (Full)** |
| **Kitty** | Linux/Mac | Truecolor | Full | Supported | Supported | Supported | **Tier 1 (Full)** |
| **iTerm2** | macOS | Truecolor | Full | Supported | Supported | Supported | **Tier 1 (Full)** |
| **Windows ConHost** | Windows | 16-color | Limited | Ignored | Ignored | Ignored | **Tier 2 (Graceful)** |
| **Apple Terminal.app** | macOS | 256-color | Full | Supported | Ignored | Limited | **Tier 2 (Graceful)** |
| **Dumb Terminal** (`TERM=dumb`)| Any | None | None | Disabled | Disabled | Disabled | **Tier 3 (Plain)** |

---

## 3. Platform Execution & Containment Matrix

| Feature | Windows | Linux | macOS | Assurance Level |
| :--- | :--- | :--- | :--- | :--- |
| **Process Tree Containment** | Windows Job Objects (`AssignProcessToJobObject`, `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) | Process Groups (`setpgid`, `killpg`) or cgroups v2 when available | Process Groups (`setpgid`, `killpg`) | Windows: `ENFORCED`<br>Linux (cgroups): `ENFORCED`<br>Linux/Mac (pgid): `OBSERVED` |
| **Child Process Handoff** | Win32 Console Mode suspension & inheritance | POSIX terminal raw mode restore & foreground pgid transfer | POSIX terminal raw mode restore & foreground pgid transfer | `ENFORCED` |
| **Process Supervision** | Bounded context, timeout kills, CAS output spooling | Bounded context, timeout kills, CAS output spooling | Bounded context, timeout kills, CAS output spooling | `ENFORCED` |
| **Closed Stdin by Default** | Closed handle / `NUL` redirect | `/dev/null` redirect | `/dev/null` redirect | `ENFORCED` |
| **Workspace SQLite WAL** | Strict file-locking, WAL journaling | Advisory POSIX locking, WAL journaling | Advisory POSIX locking, WAL journaling | `ENFORCED` |

---

## 4. Degradation Protocol for Minimal Environments

When `TerminalCapabilities::detect()` identifies a constrained environment:
1. **`NO_COLOR=1` or `TERM=dumb`**:
   - Palette switches to `ColorRoles::plain()`, zero escape sequences emitted.
   - Prompt renderer uses ASCII markers (`ok`, `dirty N dirty`, `X`).
2. **Missing OSC Support**:
   - OSC 7 directory updates are omitted.
   - OSC 8 hyperlinks degrade to inline plain-text URIs (`artifact://sha256/...`).
   - OSC 133 semantic markers are suppressed.
3. **Non-Interactive Execution (Piped / Subshell)**:
   - Reedline loop is bypassed; commands are evaluated in single-shot batch mode or delegated to headless CLI.
