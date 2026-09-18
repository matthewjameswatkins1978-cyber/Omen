# Platform Execution Backends

## Philosophy
Omen targets Linux, Windows, and macOS with cross-platform semantic consistency. Where platform features diverge (such as filesystem sandboxing or job containment), Omen truthfully reports the available assurance level rather than simulating guarantees.

## Windows Backend
- **Process Supervision**: Uses Windows Job Objects via `windows-sys`.
- **Descendant Containment**: Configured with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` to ensure descendant process trees terminate on completion or timeout.
- **Assurance**: Filesystem containment without driver filter is classified as `OBSERVED`; process tree termination is `ENFORCED`.

## Linux Backend
- **Process Supervision**: Process groups, pidfd, and cgroups v2 discovery where available.
- **Filesystem Containment**: Landlock LSM for unprivileged filesystem read/write restriction.
- **Assurance**: Where Landlock is supported, filesystem containment is `ENFORCED`. Where unavailable, Omen honestly degrades to `OBSERVED`.

## macOS Backend
- **Process Supervision**: POSIX process groups and signals.
- **Assurance**: Basic containment reported as `OBSERVED`.
