# Omen 0.4 Evidence — Platform Matrix

**Document**: `docs/evidence/0.4/PLATFORM_MATRIX.md`  
**Status**: VERIFIED ACROSS WINDOWS, LINUX, MACOS  
**Rule**: Platform Truthfulness & Cross-Platform by Default

---

## 1. Feature Support Matrix

| Subsystem / Feature | Windows | Linux | macOS | Truthfulness / Fallback |
| :--- | :--- | :--- | :--- | :--- |
| **Local IPC Transport** | Windows Named Pipes (`\\.\pipe\...`) | Unix Domain Socket | Unix Domain Socket | Zero network exposure; platform-native IPC streams |
| **Peer Admission Verification** | `EqualSid` on `TokenUser` tokens | `SO_PEERCRED` UID match | `getpeereid` UID match | Rejects cross-user connection with `PermissionDenied` |
| **Process Containment** | Windows Job Objects (`JOBOBJECT_EXTENDED_LIMIT_INFORMATION`) | Process group (`setpgid`) / PR_SET_PDEATHSIG | Process group (`setpgid`) | OS-native process tree cleanup upon exit |
| **Filesystem Watcher** | ReadDirectoryChangesW (`notify`) | inotify (`notify`) | FSEvents / kqueue (`notify`) | Invalidation events dispatched to hot index |
| **Canonical Database Path** | `<root>\.omen\state\state.sqlite` | `<root>/.omen/state/state.sqlite` | `<root>/.omen/state/state.sqlite` | One canonical store; fails closed if unwritable |
| **Interactive Terminal UI** | ConPTY + ANSI VT100 (`reedline`) | ANSI VT100 / termios | ANSI VT100 / termios | Semantic markers (OSC 7/8/133) with graceful dumb fallback |
| **Process Sandbox Enforceability** | `OBSERVED` / `UNSUPPORTED` | `ENFORCED` (Landlock) or `OBSERVED` | `OBSERVED` | Never fakes `ENFORCED` if platform cannot constrain |

---

## 2. Platform-Specific Implementation Details

### Windows
- **Endpoint**: `format!(r"\\.\pipe\omen-{username}-1")`
- **Peer Token Admission**: Uses `windows-sys` to retrieve process token, queries `TokenUser`, and runs `EqualSid` against the current process SID.
- **Process Supervisor**: Assigns child process to an anonymous Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.

### Linux
- **Endpoint**: `$XDG_RUNTIME_DIR/omen/1/omend.sock` (fallback: `~/.omen/run/1/omend.sock`).
- **Peer Token Admission**: Uses `rustix::process::getuid()` compared with `stream.peer_cred()?.uid()`. Directory permissions set to `0700`.
- **Containment**: Process group isolation with parent death signals.

### macOS
- **Endpoint**: `$XDG_RUNTIME_DIR/omen/1/omend.sock` (fallback: `~/.omen/run/1/omend.sock`).
- **Peer Token Admission**: Uses `rustix::process::getuid()` compared with socket peer credentials.
- **Containment**: Process group isolation.
