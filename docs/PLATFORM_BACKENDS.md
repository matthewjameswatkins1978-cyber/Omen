# Platform Execution Backends

## Philosophy
Omen targets Linux, Windows, and macOS with cross-platform semantic consistency. Physical execution is abstracted via the pluggable `ExecutionBackend` SPI. Where platform features diverge (such as filesystem sandboxing or job containment), Omen truthfully reports the available assurance level rather than simulating guarantees.

## Execution Backend SPI (`crates/omen-engine`)

All physical process execution passes through the `ExecutionBackend` trait:

```rust
pub trait ExecutionBackend: Send + Sync {
    fn id(&self) -> BackendId;
    fn descriptor(&self) -> BackendDescriptor;
    fn spawn(&self, req: &ExecutionRequest) -> Result<Box<dyn ExecutionHandle>, CoreError>;
    fn spawn_pty(&self, req: &PtyExecutionRequest) -> Result<Box<dyn PtyExecutionHandle>, CoreError>;
}
```

Backends are registered in `BackendRegistry` with runtime discovery:
- `:backend list`: Enumerate available execution backends.
- `:backend status`: Inspect the active backend and its capabilities.
- `:backend use <id>`: Switch the active backend dynamically.

---

## 1. Native Execution Backend (`backend://native`)

The default execution backend on the host operating system.

### Windows Native
- **Process Supervision**: Uses Windows Job Objects via `windows-sys`.
- **Descendant Containment**: Configured with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` to guarantee that entire descendant process trees terminate on completion or timeout.
- **Assurance**: Process tree termination is `ENFORCED`; filesystem boundary is `OBSERVED`.

### Linux Native
- **Process Supervision**: POSIX process groups (`setpgid(0, 0)`), pidfd, and cgroups v2 where available.
- **Filesystem Containment**: Landlock LSM for unprivileged filesystem read/write restriction.
- **Assurance**: Where Landlock is supported, filesystem containment is `ENFORCED`. Where unavailable, Omen honestly degrades to `OBSERVED`.

### macOS Native
- **Process Supervision**: POSIX process groups and signals.
- **Assurance**: Basic containment reported as `OBSERVED`.

---

## 2. WSL Execution Backend (`backend://wsl`)

Non-native execution backend bridging Windows host commands to Windows Subsystem for Linux (WSL).

- **Availability**: Dynamically verified via `wsl.exe --status` probe.
- **Path Translation**: Translates Windows paths (`C:\...`, `D:\...`) into WSL mount paths (`/mnt/c/...`, `/mnt/d/...`).
- **Execution Forwarding**: Wraps requests into `wsl.exe --cd <wsl_cwd> -e <argv...>`.
- **Assurance**: Process supervision is `MEDIATED` through WSL init. Network and filesystem boundaries are governed by the Linux subsystem.

