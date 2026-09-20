# Omen 0.6 — Platform Matrix & Assurance

## 1. Physical Enforcement Matrix

In compliance with Rule 7 of machine reasoning ("Platform truthfulness: If a platform cannot enforce a constraint, it reports OBSERVED or UNSUPPORTED. It never fakes ENFORCED"):

| Platform / Backend | Backend ID | Process Tree Termination (Descendants) | Symlink Escape Prevention | Filesystem Isolation | Network Isolation |
|---|---|---|---|---|---|
| **Windows Native** | `backend://native` | **ENFORCED** (Win32 Job Object `KILL_ON_JOB_CLOSE`) | **ENFORCED** (Path canonicalization & prefix lock) | **OBSERVED** (NTFS ACLs) | **OBSERVED** |
| **Linux Native** | `backend://native` | **BEST_EFFORT** (POSIX `setpgid` / `killpg` without cgroups v2) | **ENFORCED** (Realpath & prefix check) | **OBSERVED** (Linux VFS) | **OBSERVED** |
| **macOS Native** | `backend://native` | **BEST_EFFORT** (POSIX `setpgid` / `killpg`) | **ENFORCED** (Realpath & prefix check) | **OBSERVED** | **OBSERVED** |
| **Windows WSL** | `backend://wsl` | **MEDIATED** (WSL2 hypervisor boundary) | **ENFORCED** (Canonical prefix check) | **MEDIATED** (WSL2 ext4 VFS) | **MEDIATED** (Hyper-V virtual switch) |

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
