# Omen 0.6 — Platform Matrix & Assurance

## 1. Physical Enforcement Matrix

In compliance with Rule 7 of machine reasoning ("Platform truthfulness: If a platform cannot enforce a constraint, it reports OBSERVED or UNSUPPORTED. It never fakes ENFORCED"):

| Platform / Backend | Backend ID | Descendant Process Containment | Symlink Escape Prevention | Filesystem Isolation | Network Isolation |
|---|---|---|---|---|---|
| **Windows Native** | `backend://native` | **ENFORCED** (Win32 Job Object `KILL_ON_JOB_CLOSE`) | **OBSERVED** (Path prefix check without sandbox) | **OBSERVED** (NTFS ACLs) | **OBSERVED** |
| **Linux Native** | `backend://native` | **BEST_EFFORT** (POSIX `setpgid` / `killpg`) | **OBSERVED** (Realpath check without namespace/chroot) | **OBSERVED** (Linux VFS) | **OBSERVED** |
| **macOS Native** | `backend://native` | **BEST_EFFORT** (POSIX `setpgid` / `killpg`) | **OBSERVED** (Realpath check without sandbox-exec) | **OBSERVED** | **OBSERVED** |
| **Windows WSL** | `backend://wsl` | **MEDIATED** (WSL2 hypervisor boundary) | **MEDIATED** (WSL2 VFS boundary) | **MEDIATED** (WSL2 ext4 VFS) | **MEDIATED** (Hyper-V virtual switch) |

---

## 2. Descendant Process Containment Analysis

### Windows Native: `ENFORCED`
Windows Native uses the Win32 Job Object kernel subsystem with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. When the parent execution handle terminates or the timeout expires, closing the Job Object handle causes the Windows NT kernel to forcefully terminate all processes assigned to the job, including any grandchild or deeply nested processes. Child processes cannot escape the Job Object even by spawning detached processes or creating suspended process hierarchies.

### Linux & macOS Native: `BEST_EFFORT`
On Linux and macOS, Omen configures child processes with POSIX process groups via `setpgid(0, 0)` and terminates the tree using `killpg(pgid, SIGKILL)`. Under standard execution, this terminates all processes in the process group. However, POSIX process groups do not provide kernel-enforced containment: a rogue or adversarial descendant process can invoke `setsid()` or `setpgid()` to establish a new session or process group, detaching itself from the supervisor's signal target. Kernel-enforced descendant termination on Linux requires cgroups v2 (`cgroup.kill` or systemd transient scopes), which requires delegated root or systemd management. In accordance with Rule 7, Omen reports `BEST_EFFORT` rather than claiming `ENFORCED`.

---

## 3. Symlink Escape Prevention Analysis

### Native Host Platforms: `OBSERVED`
On Windows, Linux, and macOS, Omen performs path canonicalization and prefix validation before process execution. However, during runtime, an unconfined process running with standard user permissions can create, modify, or follow symbolic links pointing outside the workspace unless enclosed within a physical filesystem sandbox (such as Linux mount namespaces / Landlock, macOS sandbox-exec, or Windows AppContainer). Because native execution runs without active container boundaries, Omen truthfully reports `OBSERVED` for symlink escape prevention.

### WSL Subsystem: `MEDIATED`
Under WSL, commands execute inside the WSL2 lightweight utility VM. Host filesystem access is mediated through the 9P or virtio-fs boundary, and Linux filesystem operations are contained within the VM's ext4 root. Therefore, symlink escape across the VM boundary is reported as `MEDIATED`.

---

## 4. Fail-Closed Assurance Guarantee

Every execution backend exposes a `BackendCapabilities` descriptor declaring its actual enforcement levels.

When an `ExecutionRequest` specifies a required assurance constraint via `RequiredAssurance` (e.g. `filesystem: Some(Assurance::Enforced)`), the `ProcessSupervisor` performs a preflight check before any process is spawned. If the backend cannot satisfy the requirement, execution fails closed immediately with `CoreError::AssuranceNotSatisfied`, guaranteeing that no unconfined command is ever executed under false assurance.
