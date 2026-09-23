use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// Platform classification for capability and evidence honesty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// Portable claims that are intentionally platform-neutral.
    Portable,
    /// POSIX-family claims (signals, process groups, termios, …).
    Posix,
    /// Windows-family claims (console, job objects, ConPTY, …).
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Posix
        }
    }

    pub fn stable_id(&self) -> &'static str {
        match self {
            Platform::Portable => "PORTABLE",
            Platform::Posix => "POSIX",
            Platform::Windows => "WINDOWS",
        }
    }
}

/// Observable capabilities. Portable kinds are implemented in M0; platform
/// kinds exist as schema only until their dedicated milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    ProcessSpawn,
    Stdin,
    Stdout,
    Stderr,
    ExitStatus,
    ProcessTree,
    Resize,
    PosixProcessGroup,
    PosixControllingTerminal,
    PosixForegroundProcessGroup,
    PosixSignals,
    PosixTermios,
    Win32Console,
    Win32ConsoleControl,
    Win32JobObject,
    Win32ConPty,
}

impl Capability {
    pub fn stable_id(&self) -> &'static str {
        match self {
            Capability::ProcessSpawn => "PROCESS_SPAWN",
            Capability::Stdin => "STDIN",
            Capability::Stdout => "STDOUT",
            Capability::Stderr => "STDERR",
            Capability::ExitStatus => "EXIT_STATUS",
            Capability::ProcessTree => "PROCESS_TREE",
            Capability::Resize => "RESIZE",
            Capability::PosixProcessGroup => "POSIX_PROCESS_GROUP",
            Capability::PosixControllingTerminal => "POSIX_CONTROLLING_TERMINAL",
            Capability::PosixForegroundProcessGroup => "POSIX_FOREGROUND_PROCESS_GROUP",
            Capability::PosixSignals => "POSIX_SIGNALS",
            Capability::PosixTermios => "POSIX_TERMIOS",
            Capability::Win32Console => "WIN32_CONSOLE",
            Capability::Win32ConsoleControl => "WIN32_CONSOLE_CONTROL",
            Capability::Win32JobObject => "WIN32_JOB_OBJECT",
            Capability::Win32ConPty => "WIN32_CONPTY",
        }
    }

    /// Capabilities this M0 portable runner claims to exercise.
    pub fn portable_initial_set() -> &'static [Capability] {
        &[
            Capability::ProcessSpawn,
            Capability::Stdin,
            Capability::Stdout,
            Capability::Stderr,
            Capability::ExitStatus,
        ]
    }
}

/// How the runner presents the child environment.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum EnvPolicy {
    /// Start from an empty environment and insert only the listed pairs.
    AllowList(Vec<(String, String)>),
    /// Inherit the parent environment, then apply listed overrides.
    InheritWith(Vec<(String, String)>),
    /// Completely empty child environment.
    #[default]
    Clear,
}

/// Child stdin presentation. No interactive scripting DSL in M0.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum StdinSpec {
    /// Close the child's stdin immediately (fixture observes EOF).
    #[default]
    Closed,
    /// Write exactly these bytes, then close stdin.
    Bytes(Vec<u8>),
    /// Leave the parent's stdin attached (only when an existing test needs it).
    Inherited,
}

/// Explicit bound for an external wait. Timeout is a failure bound, never a
/// sequencing primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deadline {
    pub timeout_ms: u64,
}

impl Deadline {
    pub fn from_duration(timeout: Duration) -> Self {
        Self {
            timeout_ms: timeout.as_millis() as u64,
        }
    }

    pub fn duration(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }

    pub fn is_zero(&self) -> bool {
        self.timeout_ms == 0
    }
}

impl Default for Deadline {
    fn default() -> Self {
        Self { timeout_ms: 10_000 }
    }
}

/// Process invocation without shell-string ambiguity. `argv` holds arguments
/// only; `program` is the executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: EnvPolicy,
    pub stdin: StdinSpec,
    pub deadline: Deadline,
}

impl CommandSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            argv: Vec::new(),
            cwd: None,
            env: EnvPolicy::Clear,
            stdin: StdinSpec::Closed,
            deadline: Deadline::default(),
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.argv.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.argv.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn env(mut self, env: EnvPolicy) -> Self {
        self.env = env;
        self
    }

    pub fn stdin(mut self, stdin: StdinSpec) -> Self {
        self.stdin = stdin;
        self
    }

    pub fn deadline(mut self, deadline: Deadline) -> Self {
        self.deadline = deadline;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.deadline = Deadline::from_duration(timeout);
        self
    }
}

/// Evidence quality. Never silently upgraded.
/// Declared weakest → strongest so derived `Ord` matches strength ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceGrade {
    /// Platform or environment cannot establish the claim.
    Unavailable,
    /// Indirect evidence only.
    Weak,
    /// Some relevant truth observed, but not enough for a full claim.
    Partial,
    /// Directly observed through an appropriate independent mechanism.
    Strong,
}

impl EvidenceGrade {
    pub fn stable_id(&self) -> &'static str {
        match self {
            EvidenceGrade::Strong => "STRONG",
            EvidenceGrade::Partial => "PARTIAL",
            EvidenceGrade::Weak => "WEAK",
            EvidenceGrade::Unavailable => "UNAVAILABLE",
        }
    }

    /// Never upgrade: take the weaker of two grades.
    pub fn weaker(self, other: Self) -> Self {
        if self <= other { self } else { other }
    }
}

/// Portable termination shape. Does not collapse every exit into a bare
/// integer, and does not fake one platform using the other's terminology.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ExitCause {
    /// Ordinary portable exit code.
    Code { code: i32 },
    /// Runner killed the process after the deadline.
    TimeoutKill,
    /// Caller cancelled the run before natural completion.
    OmenCancel,
    /// POSIX signal termination (platform extension).
    Signal { signal: i32 },
    /// Windows raw status when a normal code cannot be recovered.
    WindowsStatus { code: u32 },
    /// Termination observed but not classified without inventing a code.
    Unknown { evidence: String },
}

impl ExitCause {
    pub fn code(code: i32) -> Self {
        ExitCause::Code { code }
    }

    /// Best-effort mapping from a standard exit status without inventing
    /// portable codes for non-code termination.
    pub fn from_exit_status(status: std::process::ExitStatus) -> Self {
        if let Some(code) = status.code() {
            return ExitCause::Code { code };
        }

        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = status.signal() {
                return ExitCause::Signal { signal };
            }
        }

        ExitCause::Unknown {
            evidence: format!("exit status without portable code: {status:?}"),
        }
    }

    pub fn as_code(&self) -> Option<i32> {
        match self {
            ExitCause::Code { code } => Some(*code),
            _ => None,
        }
    }

    pub fn stable_id(&self) -> String {
        match self {
            ExitCause::Code { code } => format!("code:{code}"),
            ExitCause::TimeoutKill => "timeout_kill".into(),
            ExitCause::OmenCancel => "omen_cancel".into(),
            ExitCause::Signal { signal } => format!("signal:{signal}"),
            ExitCause::WindowsStatus { code } => format!("windows_status:{code}"),
            ExitCause::Unknown { .. } => "unknown".into(),
        }
    }
}

/// Bounded capture policy for a single stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputBounds {
    pub max_stdout_bytes: usize,
    pub max_stderr_bytes: usize,
}

impl Default for OutputBounds {
    fn default() -> Self {
        Self {
            max_stdout_bytes: 64 * 1024,
            max_stderr_bytes: 64 * 1024,
        }
    }
}

/// Process-tree identity substrate for later containment work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProcessTreeObservation {
    pub root_pid: u32,
    pub child_pids: Vec<u32>,
    pub source: String,
}

/// Convenience map type used by fixture reports.
pub type JsonMap = BTreeMap<String, serde_json::Value>;
