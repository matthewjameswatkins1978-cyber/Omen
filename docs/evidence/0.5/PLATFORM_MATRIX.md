# Omen 0.5 — Platform Matrix

## 1. Supported Platform Matrix

| Platform | Process Containment | IPC Transport | MCP Stdio | Test Suite Status |
|---|---|---|---|---|
| **Windows x86_64** | Job Objects (`AssignProcessToJobObject`) | Named Pipes (`\\.\pipe\omen-...`) | Supported | **VERIFIED PASS** |
| **Linux x86_64** | Process Groups & Landlock hooks | Unix Domain Sockets (`$XDG_RUNTIME_DIR`) | Supported | **VERIFIED PASS** |
| **macOS aarch64** | Process Groups & Sandbox hooks | Unix Domain Sockets (`~/.omen/run`) | Supported | **VERIFIED PASS** |

---

## 2. Platform Enforceability Truthfulness

In compliance with Rule 7 of machine reasoning:
- Windows explicitly reports `JobObjects` containment capability.
- Linux and macOS report `ProcessGroup` containment capability.
- In both modes, no unsupported platform sandboxing is falsely marked as `ENFORCED`.
