# Omen 0.6 — Platform Matrix & Assurance

## 1. Physical Enforcement Matrix

In compliance with Rule 7 of machine reasoning ("Platform truthfulness: If a platform cannot enforce a constraint, it reports OBSERVED or UNSUPPORTED. It never fakes ENFORCED"):

| Platform / Backend | Backend ID | Process Tree Termination | Filesystem Isolation | Network Isolation | Memory Limits |
|---|---|---|---|---|---|
| **Windows Native** | `backend://native` | **ENFORCED** (Job Object `KILL_ON_JOB_CLOSE`) | **OBSERVED** (standard ACLs) | **OBSERVED** | **ENFORCED** (Job Object limits) |
| **Linux Native** | `backend://native` | **ENFORCED** (POSIX process groups / pidfd) | **ENFORCED** (Landlock LSM where available) | **OBSERVED** / **ENFORCED** (namespaces) | **ENFORCED** (cgroups v2 where available) |
| **macOS Native** | `backend://native` | **ENFORCED** (POSIX process groups) | **OBSERVED** / **MEDIATED** (sandbox-exec) | **OBSERVED** | **OBSERVED** |
| **Windows WSL** | `backend://wsl` | **MEDIATED** (WSL init / process tree) | **MEDIATED** (Linux VFS inside VM) | **MEDIATED** (Virtual switch) | **MEDIATED** (WSL VM memory cap) |

---

## 2. Fail-Closed Guarantee

Every backend exposes a `BackendCapabilities` descriptor declaring its actual enforcement level for each capability:
- `process_isolation`
- `filesystem_isolation`
- `network_isolation`
- `resource_limits`
- `pty_support`
- `secret_redaction`

When an execution contract requests a minimum assurance level (such as `RequiredAssurance::Enforced`), Omen performs a preflight check before process spawn. If the capability is `OBSERVED`, `BEST_EFFORT`, or `UNSUPPORTED`, Omen **refuses to run** and returns `CoreError::PreflightFailed`.
