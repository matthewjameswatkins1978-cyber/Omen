use clap::Parser;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "omen-gremlin",
    about = "Hostile and controlled testing fixture"
)]
struct GremlinArgs {
    #[arg(long)]
    stdout: Option<String>,

    #[arg(long)]
    stdout_bytes: Option<usize>,

    #[arg(long)]
    stderr: Option<String>,

    #[arg(long)]
    read_stdin: bool,

    #[arg(long)]
    echo_stdin: bool,

    /// Report stdin topology as JSON (`STDIN_TOPOLOGY:{...}`) without
    /// consuming a terminal. Reads stdin to EOF only when stdin is not a
    /// terminal, so harness pipes and null devices report byte counts while
    /// interactive runs never block.
    #[arg(long)]
    report_stdin_topology: bool,

    /// Read stdin to EOF and emit a single-line JSON report
    /// (`{"fixture":"stdin-report",...}`) with byte count and EOF observation.
    /// Skips consumption when stdin is a terminal so interactive runs never block.
    #[arg(long)]
    stdin_report: bool,

    /// Interleave large flushed chunks on stdout and stderr to expose
    /// sequential pipe-draining deadlocks.
    #[arg(long)]
    dual_stream: bool,

    /// Emit more bytes than the Compat inline capture buffer (256 KiB pattern).
    #[arg(long)]
    large_output: bool,

    /// Spawn one short-lived portable child and report identity as single-line JSON.
    #[arg(long)]
    spawn_child_portable: bool,

    /// Emit a single-line JSON identity report and exit.
    #[arg(long)]
    compat_report: bool,

    /// Spawn a short-lived child that inherits stdout/stderr, then exit
    /// promptly while the child keeps the pipe open for a bounded lifetime.
    #[arg(long)]
    descendant_holds_stdout: bool,

    /// Never consume stdin; stay alive long enough to block a large writer.
    #[arg(long)]
    ignore_stdin: bool,

    #[arg(long)]
    hostile_terminal_escapes: bool,

    #[arg(long)]
    spawn_child: bool,

    #[arg(long)]
    spawn_tree: Option<usize>,

    #[arg(long)]
    spawn_tree_delay_ms: Option<u64>,

    #[arg(long)]
    pty_echo: bool,

    /// POSIX: report machine-readable process/TTY identity as one JSON line.
    #[arg(long)]
    posix_report: bool,

    /// POSIX: report READY, stop self with SIGSTOP, report CONTINUED, exit.
    #[arg(long)]
    posix_stop_report: bool,

    /// POSIX: report READY, wait for SIGINT, report delivery, exit boundedly.
    #[arg(long)]
    posix_sigint_report: bool,

    /// POSIX: report READY + winsize, wait for SIGWINCH, report new size, exit.
    #[arg(long)]
    posix_winch_report: bool,

    /// POSIX: dirty selected termios flags then exit with `--exit` (abnormal).
    #[arg(long)]
    posix_termios_dirty_exit: bool,

    #[arg(long)]
    write: Option<PathBuf>,

    /// Sleep before exit. Alias `--sleep-bounded` matches Compat M0 naming.
    #[arg(long = "sleep-ms", alias = "sleep-bounded")]
    sleep_ms: Option<u64>,

    #[arg(long)]
    print_env: Option<String>,

    #[arg(long)]
    lsp_mode: Option<String>,

    /// Exit with this exact code. Alias `--exit-code` matches Compat M0 naming.
    #[arg(long = "exit", alias = "exit-code", default_value_t = 0)]
    exit: i32,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    trailing_args: Vec<String>,
}

#[allow(clippy::zombie_processes)]
fn main() {
    let args = GremlinArgs::parse();

    if let Some(mode) = &args.lsp_mode {
        run_hostile_lsp(mode);
        return;
    }

    if let Ok(stall_str) = std::env::var("OMEN_GREMLIN_STALL_MS")
        && let Ok(ms) = stall_str.parse::<u64>()
    {
        thread::sleep(Duration::from_millis(ms));
    }

    if let Some(msg) = args.stdout {
        println!("{msg}");
        let _ = io::stdout().flush();
    }

    if let Some(count) = args.stdout_bytes {
        let chunk = "A".repeat(count);
        print!("{chunk}");
        let _ = io::stdout().flush();
    }

    if let Some(err) = args.stderr {
        eprintln!("{err}");
        let _ = io::stderr().flush();
    }

    if args.hostile_terminal_escapes {
        println!(
            "\x1b[2J\x1b[H\x1b[31;1mHOSTILE_CSI\x1b[0m\x1b]0;HostileWindowTitle\x07\x07HOSTILE_ALERT\x1b]2;HackedTitle\x1b\\LEGITIMATE_DATA"
        );
        let _ = io::stdout().flush();
    }

    if args.read_stdin {
        let mut buffer = String::new();
        let bytes_read = io::stdin().read_to_string(&mut buffer).unwrap_or(0);
        println!("READ_STDIN_BYTES:{bytes_read}");
        let _ = io::stdout().flush();
    }

    if args.report_stdin_topology {
        println!("STDIN_TOPOLOGY:{}", stdin_topology_json());
        let _ = io::stdout().flush();
    }

    if args.stdin_report {
        print_stdin_report();
    }

    if args.dual_stream {
        dual_stream_burst();
    }

    if args.large_output {
        large_output_burst();
    }

    if args.spawn_child_portable {
        spawn_child_portable();
    }

    if args.compat_report {
        println!(
            "{{\"fixture\":\"compat-report\",\"pid\":{},\"exit\":{}}}",
            std::process::id(),
            args.exit
        );
        let _ = io::stdout().flush();
    }

    if args.descendant_holds_stdout {
        descendant_holds_stdout();
        std::process::exit(args.exit);
    }

    if args.ignore_stdin {
        ignore_stdin_bounded();
        std::process::exit(args.exit);
    }

    if args.echo_stdin {
        let mut buffer = String::new();
        let bytes_read = io::stdin().read_to_string(&mut buffer).unwrap_or(0);
        println!("READ_STDIN_BYTES:{bytes_read}");
        print!("STDIN_ECHO:{buffer}");
        let _ = io::stdout().flush();
    }

    if let Some(path) = args.write {
        std::fs::write(&path, b"gremlin-write-payload").expect("gremlin write failed");
    }

    if let Some(env_name) = args.print_env {
        let val = std::env::var(&env_name).unwrap_or_else(|_| "<UNSET>".into());
        println!("{env_name}={val}");
        let _ = io::stdout().flush();
    }

    if args.spawn_child {
        let exe = std::env::current_exe().expect("failed to get current_exe");
        let child = Command::new(exe)
            .arg("--sleep-ms")
            .arg("10000")
            .spawn()
            .expect("gremlin failed to spawn child");
        println!("CHILD_SPAWNED:{}", child.id());
        let _ = io::stdout().flush();
    }

    if let Some(depth) = args.spawn_tree {
        if let Some(delay_ms) = args.spawn_tree_delay_ms {
            thread::sleep(Duration::from_millis(delay_ms));
        }
        if depth == 0 {
            // Leaf node
        } else {
            let exe = std::env::current_exe().expect("failed to get current_exe");
            let mut cmd = Command::new(exe);
            cmd.arg("--spawn-tree")
                .arg((depth - 1).to_string())
                .arg("--sleep-ms")
                .arg("60000");

            let child = cmd.spawn().expect("gremlin failed to spawn subtree");
            println!("TREE_SPAWNED:{}", child.id());
            let _ = io::stdout().flush();
        }
    }

    if args.pty_echo {
        let is_term = std::io::stdout().is_terminal();
        let (cols, rows) = crossterm::terminal::size().unwrap_or((0, 0));
        println!("PTY_READY");
        println!("IS_TERMINAL:{is_term}");
        println!("INITIAL_SIZE:{cols}x{rows}");
        let _ = io::stdout().flush();

        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    let trimmed = l.trim();
                    if trimmed == "exit" || trimmed == "quit" {
                        println!("PTY_EXITING");
                        let _ = io::stdout().flush();
                        std::process::exit(0);
                    }
                    if trimmed == "get_size" {
                        let (c, r) = crossterm::terminal::size().unwrap_or((0, 0));
                        println!("CURRENT_SIZE:{c}x{r}");
                        let _ = io::stdout().flush();
                        continue;
                    }
                    println!("ECHO:{trimmed}");
                    let _ = io::stdout().flush();
                }
                Err(_) => break,
            }
        }
    }

    #[cfg(unix)]
    {
        if args.posix_report {
            run_posix_report();
            std::process::exit(args.exit);
        }
        if args.posix_stop_report {
            run_posix_stop_report();
            std::process::exit(args.exit);
        }
        if args.posix_sigint_report {
            run_posix_sigint_report();
            std::process::exit(args.exit);
        }
        if args.posix_winch_report {
            run_posix_winch_report(args.exit);
        }
        if args.posix_termios_dirty_exit {
            run_posix_termios_dirty_exit();
            std::process::exit(args.exit);
        }
    }
    #[cfg(not(unix))]
    {
        let requested_posix = args.posix_report
            || args.posix_stop_report
            || args.posix_sigint_report
            || args.posix_winch_report
            || args.posix_termios_dirty_exit;
        if requested_posix {
            println!(
                "{{\"fixture\":\"posix-unsupported\",\"pid\":{},\"platform\":\"windows\"}}",
                std::process::id()
            );
            let _ = io::stdout().flush();
            std::process::exit(64);
        }
    }

    if let Some(ms) = args.sleep_ms {
        thread::sleep(Duration::from_millis(ms));
    }

    std::process::exit(args.exit);
}

/// Spawn a child that inherits stdout/stderr, report identity, exit now.
/// Child sleeps only 700ms so no long-lived orphan remains.
fn descendant_holds_stdout() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            println!(
                "{{\"fixture\":\"descendant-holds-stdout\",\"pid\":{},\"error\":\"{err}\"}}",
                std::process::id()
            );
            let _ = io::stdout().flush();
            return;
        }
    };
    match Command::new(exe).arg("--sleep-ms").arg("700").spawn() {
        Ok(child) => {
            println!(
                "{{\"fixture\":\"descendant-holds-stdout\",\"pid\":{},\"child_pid\":{}}}",
                std::process::id(),
                child.id()
            );
            let _ = io::stdout().flush();
            // Root exits immediately; child keeps inherited pipes open ~700ms.
        }
        Err(err) => {
            println!(
                "{{\"fixture\":\"descendant-holds-stdout\",\"pid\":{},\"error\":\"{err}\"}}",
                std::process::id()
            );
            let _ = io::stdout().flush();
        }
    }
}

/// Do not read stdin; emit READY and remain alive for a bounded window so a
/// large parent write can block on a full pipe.
fn ignore_stdin_bounded() {
    println!(
        "{{\"fixture\":\"ignore-stdin\",\"pid\":{},\"note\":\"not reading stdin\"}}",
        std::process::id()
    );
    let _ = io::stdout().flush();
    thread::sleep(Duration::from_millis(1200));
}

/// Read stdin to EOF (unless a TTY) and print one JSON line of facts.
fn print_stdin_report() {
    let is_tty = io::stdin().is_terminal();
    if is_tty {
        println!(
            "{{\"fixture\":\"stdin-report\",\"pid\":{},\"stdin_eof\":false,\"bytes_read\":0,\"note\":\"tty\"}}",
            std::process::id()
        );
        let _ = io::stdout().flush();
        return;
    }
    let mut buf = Vec::new();
    let (bytes_read, eof) = match io::stdin().read_to_end(&mut buf) {
        Ok(n) => (n, true),
        Err(_) => (0, false),
    };
    println!(
        "{{\"fixture\":\"stdin-report\",\"pid\":{},\"stdin_eof\":{eof},\"bytes_read\":{bytes_read}}}",
        std::process::id()
    );
    let _ = io::stdout().flush();
}

/// Interleave flushed chunks on both streams so naive sequential draining
/// deadlocks once pipe buffers fill.
fn dual_stream_burst() {
    // 24 * 16 KiB = 384 KiB per stream; well above typical 64 KiB pipe buffers.
    let chunk = vec![b'X'; 16 * 1024];
    let err_chunk = vec![b'Y'; 16 * 1024];
    let mut out = io::stdout();
    let mut err = io::stderr();
    for i in 0..24 {
        let _ = out.write_all(&chunk);
        let _ = out.flush();
        let _ = err.write_all(&err_chunk);
        let _ = err.flush();
        if i % 4 == 3 {
            let _ = out.write_all(b"DUAL_TICK\n");
            let _ = out.flush();
            let _ = err.write_all(b"DUAL_TICK\n");
            let _ = err.flush();
        }
    }
}

/// Emit 256 KiB so Compat's inline buffer must truncate explicitly.
fn large_output_burst() {
    let total = 256 * 1024usize;
    let pattern = b"OMEN_COMPAT_LARGE_OUTPUT_PATTERN\n";
    let mut out = io::stdout();
    let mut written = 0usize;
    while written < total {
        let take = pattern.len().min(total - written);
        let _ = out.write_all(&pattern[..take]);
        written += take;
    }
    let _ = out.flush();
}

/// Spawn one short-lived child and report identity; wait so no zombie remains.
fn spawn_child_portable() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            println!(
                "{{\"fixture\":\"spawn-child-portable\",\"pid\":{},\"error\":\"{err}\"}}",
                std::process::id()
            );
            let _ = io::stdout().flush();
            return;
        }
    };
    match Command::new(exe).arg("--sleep-ms").arg("250").spawn() {
        Ok(mut child) => {
            let child_id = child.id();
            println!(
                "{{\"fixture\":\"spawn-child-portable\",\"pid\":{},\"child_pid\":{child_id}}}",
                std::process::id()
            );
            let _ = io::stdout().flush();
            // Bounded wait: child sleeps only 250ms.
            let _ = child.wait();
        }
        Err(err) => {
            println!(
                "{{\"fixture\":\"spawn-child-portable\",\"pid\":{},\"error\":\"{err}\"}}",
                std::process::id()
            );
            let _ = io::stdout().flush();
        }
    }
}

#[cfg(unix)]
fn posix_identity_json() -> String {
    use rustix::process::{getpgid, getpid, getppid, getsid};
    use std::io::IsTerminal;
    use std::os::fd::BorrowedFd;

    fn stdin_fd() -> BorrowedFd<'static> {
        // SAFETY: fd 0 remains open for the process lifetime in fixture modes.
        unsafe { BorrowedFd::borrow_raw(0) }
    }
    fn stdout_fd() -> BorrowedFd<'static> {
        unsafe { BorrowedFd::borrow_raw(1) }
    }

    let pid = getpid();
    let ppid = getppid();
    let pgrp = getpgid(None).ok();
    let sid = getsid(None).ok();
    let stdin_tty = io::stdin().is_terminal();
    let stdout_tty = io::stdout().is_terminal();
    let stderr_tty = io::stderr().is_terminal();

    let fg = rustix::termios::tcgetpgrp(stdin_fd())
        .ok()
        .map(|p| p.as_raw_nonzero().get())
        .or_else(|| {
            rustix::termios::tcgetpgrp(stdout_fd())
                .ok()
                .map(|p| p.as_raw_nonzero().get())
        });

    let (cols, rows) = crossterm::terminal::size().unwrap_or((0, 0));
    let mut icanon = None;
    let mut echo = None;
    let mut isig = None;
    if let Ok(t) = rustix::termios::tcgetattr(stdin_fd()) {
        icanon = Some(t.local_modes.contains(rustix::termios::LocalModes::ICANON));
        echo = Some(t.local_modes.contains(rustix::termios::LocalModes::ECHO));
        isig = Some(t.local_modes.contains(rustix::termios::LocalModes::ISIG));
    }

    let (blocked, ignored, caught) = posix_signal_masks();

    format!(
        "{{\"fixture\":\"posix-report\",\"pid\":{},\"ppid\":{},\"pgrp\":{},\"sid\":{},\"stdin_isatty\":{stdin_tty},\"stdout_isatty\":{stdout_tty},\"stderr_isatty\":{stderr_tty},\"terminal_foreground_pgrp\":{},\"winsize_rows\":{rows},\"winsize_cols\":{cols},\"icanon\":{},\"echo\":{},\"isig\":{},\"blocked\":{},\"ignored\":{},\"caught\":{}}}",
        pid,
        ppid.map(|p| p.as_raw_nonzero().get()).unwrap_or(-1),
        pgrp.map(|p| p.as_raw_nonzero().get()).unwrap_or(-1),
        sid.map(|p| p.as_raw_nonzero().get()).unwrap_or(-1),
        fg.unwrap_or(-1),
        opt_bool(icanon),
        opt_bool(echo),
        opt_bool(isig),
        json_str_array(&blocked),
        json_str_array(&ignored),
        json_str_array(&caught),
    )
}

#[cfg(unix)]
fn opt_bool(v: Option<bool>) -> String {
    v.map(|b| b.to_string()).unwrap_or_else(|| "null".into())
}

#[cfg(unix)]
fn json_str_array(items: &[String]) -> String {
    let parts: Vec<String> = items
        .iter()
        .map(|s| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    format!("[{}]", parts.join(","))
}

/// Report blocked/ignored/caught signal names via `/proc/self/status` on Linux.
/// Empty vectors on platforms without that file (UNAVAILABLE, not guessed).
#[cfg(unix)]
fn posix_signal_masks() -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut blocked = Vec::new();
    let mut ignored = Vec::new();
    let mut caught = Vec::new();
    if let Ok(text) = std::fs::read_to_string("/proc/self/status") {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("SigBlk:") {
                blocked = decode_sigmask_hex(rest.trim());
            } else if let Some(rest) = line.strip_prefix("SigIgn:") {
                ignored = decode_sigmask_hex(rest.trim());
            } else if let Some(rest) = line.strip_prefix("SigCgt:") {
                caught = decode_sigmask_hex(rest.trim());
            }
        }
    }
    (blocked, ignored, caught)
}

#[cfg(unix)]
fn decode_sigmask_hex(hex: &str) -> Vec<String> {
    let Ok(bits) = u64::from_str_radix(hex, 16) else {
        return Vec::new();
    };
    const NAMES: &[(i32, &str)] = &[
        (1, "SIGHUP"),
        (2, "SIGINT"),
        (3, "SIGQUIT"),
        (4, "SIGILL"),
        (5, "SIGTRAP"),
        (6, "SIGABRT"),
        (7, "SIGBUS"),
        (8, "SIGFPE"),
        (9, "SIGKILL"),
        (10, "SIGUSR1"),
        (11, "SIGSEGV"),
        (12, "SIGUSR2"),
        (13, "SIGPIPE"),
        (14, "SIGALRM"),
        (15, "SIGTERM"),
        (17, "SIGCHLD"),
        (18, "SIGCONT"),
        (19, "SIGSTOP"),
        (20, "SIGTSTP"),
        (21, "SIGTTIN"),
        (22, "SIGTTOU"),
        (23, "SIGURG"),
        (24, "SIGXCPU"),
        (25, "SIGXFSZ"),
        (26, "SIGVTALRM"),
        (27, "SIGPROF"),
        (28, "SIGWINCH"),
        (29, "SIGIO"),
        (30, "SIGPWR"),
        (31, "SIGSYS"),
    ];
    NAMES
        .iter()
        .filter(|(n, _)| {
            if *n >= 32 {
                return false;
            }
            bits & (1u64 << (n - 1)) != 0
        })
        .map(|(_, name)| name.to_string())
        .collect()
}

#[cfg(unix)]
fn run_posix_report() {
    let json = posix_identity_json();
    println!("OMEN_COMPAT_READY");
    println!("{json}");
    let _ = io::stdout().flush();
}

#[cfg(unix)]
fn run_posix_stop_report() {
    use rustix::process::{Signal, getpid, kill_process};

    println!("OMEN_COMPAT_READY");
    println!("{}", posix_identity_json());
    let _ = io::stdout().flush();
    println!("OMEN_COMPAT_STOPPING");
    let _ = io::stdout().flush();
    // Real stop: SIGSTOP is not catchable; execution resumes on SIGCONT.
    let _ = kill_process(getpid(), Signal::STOP);
    println!("OMEN_COMPAT_CONTINUED");
    let _ = io::stdout().flush();
}

#[cfg(unix)]
fn run_posix_sigint_report() {
    println!("OMEN_COMPAT_READY");
    println!("{}", posix_identity_json());
    let _ = io::stdout().flush();
    // Default SIGINT disposition terminates with signal 2 (signal-faithful).
    // Sleep only as a wait for an external signal under test; outer harness
    // bounds the scenario deadline.
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

#[cfg(unix)]
fn run_posix_winch_report(exit_code: i32) {
    use tokio::signal::unix::{SignalKind, signal};

    let (cols, rows) = crossterm::terminal::size().unwrap_or((0, 0));
    println!("OMEN_COMPAT_READY");
    println!(
        "{{\"fixture\":\"posix-winch-report\",\"pid\":{},\"winsize_rows\":{rows},\"winsize_cols\":{cols}}}",
        std::process::id()
    );
    let _ = io::stdout().flush();
    println!("OMEN_COMPAT_SIGWINCH_WAIT");
    let _ = io::stdout().flush();

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("winch runtime: {err}");
            std::process::exit(exit_code);
        }
    };
    rt.block_on(async {
        let mut winch = match signal(SignalKind::window_change()) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("winch signal: {err}");
                return;
            }
        };
        // Bounded by outer harness; also hard-bound here at 10s.
        match tokio::time::timeout(Duration::from_secs(10), winch.recv()).await {
            Ok(Some(())) => {
                let (c, r) = crossterm::terminal::size().unwrap_or((0, 0));
                println!("OMEN_COMPAT_SIGWINCH");
                println!(
                    "{{\"fixture\":\"posix-winch-report\",\"signal\":\"SIGWINCH\",\"winsize_rows\":{r},\"winsize_cols\":{c}}}"
                );
                let _ = io::stdout().flush();
            }
            _ => {
                println!("OMEN_COMPAT_SIGWINCH_TIMEOUT");
                let _ = io::stdout().flush();
            }
        }
    });
    std::process::exit(exit_code);
}

#[cfg(unix)]
fn run_posix_termios_dirty_exit() {
    use rustix::termios::{LocalModes, OptionalActions, tcgetattr, tcsetattr};
    use std::os::fd::BorrowedFd;

    fn stdin_fd() -> BorrowedFd<'static> {
        unsafe { BorrowedFd::borrow_raw(0) }
    }

    println!("OMEN_COMPAT_READY");
    println!("{}", posix_identity_json());
    let _ = io::stdout().flush();
    println!("OMEN_COMPAT_DIRTYING");
    let _ = io::stdout().flush();

    let mut applied = false;
    if let Ok(mut t) = tcgetattr(stdin_fd()) {
        // Controlled dirty subset: clear ICANON/ECHO/ISIG.
        let mut modes = t.local_modes;
        modes.remove(LocalModes::ICANON | LocalModes::ECHO | LocalModes::ISIG);
        t.local_modes = modes;
        for action in [
            OptionalActions::Now,
            OptionalActions::Flush,
            OptionalActions::Drain,
        ] {
            if tcsetattr(stdin_fd(), action, &t).is_ok() {
                applied = true;
                break;
            }
        }
    }
    let verify = tcgetattr(stdin_fd())
        .map(|t| {
            !t.local_modes.contains(LocalModes::ICANON)
                || !t.local_modes.contains(LocalModes::ECHO)
                || !t.local_modes.contains(LocalModes::ISIG)
        })
        .unwrap_or(false);
    println!("OMEN_COMPAT_DIRTY applied={applied} verify={verify}");
    let _ = io::stdout().flush();
    // Abnormal exit is chosen by `--exit`. Exit without restoring.
}

/// Classify what the child observes on stdin without blocking.
///
/// `kind` is one of `tty`, `pipe`, `char` (console or null device), `disk`
/// (file redirection), `socket`, `unknown`, or `invalid`. `stdin_bytes` is
/// the number of bytes readable to EOF, or 0 when stdin is a terminal (never
/// consumed, so interactive runs cannot hang the fixture).
fn stdin_topology_json() -> String {
    let is_tty = io::stdin().is_terminal();
    let kind = stdin_kind();
    let stdin_bytes = if is_tty {
        0
    } else {
        let mut buffer = Vec::new();
        io::stdin().read_to_end(&mut buffer).unwrap_or(0)
    };
    format!("{{\"is_tty\":{is_tty},\"kind\":\"{kind}\",\"stdin_bytes\":{stdin_bytes}}}")
}

#[cfg(windows)]
fn stdin_kind() -> &'static str {
    use std::os::windows::io::AsRawHandle;
    // GetFileType via direct FFI (no extra dependency for a fixture).
    // FILE_TYPE_UNKNOWN = 0, FILE_TYPE_DISK = 1, FILE_TYPE_CHAR = 2, FILE_TYPE_PIPE = 3.
    unsafe extern "system" {
        fn GetFileType(hFile: isize) -> u32;
    }
    let handle = io::stdin().as_raw_handle() as isize;
    if handle == 0 {
        return "invalid";
    }
    match unsafe { GetFileType(handle) } {
        1 => "disk",
        2 => "char",
        3 => "pipe",
        _ => "unknown",
    }
}

#[cfg(unix)]
fn stdin_kind() -> &'static str {
    use std::os::unix::fs::FileTypeExt;
    match std::fs::metadata("/dev/stdin") {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            if file_type.is_fifo() {
                "pipe"
            } else if file_type.is_socket() {
                "socket"
            } else if file_type.is_char_device() {
                "char"
            } else if file_type.is_block_device() || file_type.is_file() {
                "disk"
            } else {
                "unknown"
            }
        }
        Err(_) => "unknown",
    }
}

#[cfg(not(any(windows, unix)))]
fn stdin_kind() -> &'static str {
    "unknown"
}

fn run_hostile_lsp(mode: &str) {
    use std::io::{BufRead, Read, Write};
    let stdin = std::io::stdin();
    let mut stdin_lock = stdin.lock();
    let mut stdout = std::io::stdout();

    let write_response = |body: &str| {
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut out = std::io::stdout();
        let _ = out.write_all(header.as_bytes());
        let _ = out.write_all(body.as_bytes());
        let _ = out.flush();
    };

    loop {
        let mut header_line = String::new();
        let mut content_length = None;
        loop {
            header_line.clear();
            if stdin_lock.read_line(&mut header_line).unwrap_or(0) == 0 {
                return;
            }
            let trimmed = header_line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                content_length = rest.trim().parse::<usize>().ok();
            }
        }

        let Some(len) = content_length else { continue };
        let mut body = vec![0u8; len];
        if stdin_lock.read_exact(&mut body).is_err() {
            return;
        }

        let Ok(val) = serde_json::from_slice::<serde_json::Value>(&body) else {
            continue;
        };

        let method = val.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = val.get("id").and_then(|i| i.as_u64());

        match method {
            "initialize" => match mode {
                "slow-init" => {
                    thread::sleep(Duration::from_millis(5000));
                    let resp = format!(
                        r#"{{"jsonrpc":"2.0","id":{},"result":{{"capabilities":{{}}}}}}"#,
                        id.unwrap_or(1)
                    );
                    write_response(&resp);
                }
                "never-init" => {
                    thread::sleep(Duration::from_secs(3600));
                }
                "semantic-filtered" => {
                    let resp = format!(
                        r#"{{"jsonrpc":"2.0","id":{},"result":{{"capabilities":{{"experimental":{{"workspaceSymbolScopeKindFiltering":true}}}}}}}}"#,
                        id.unwrap_or(1)
                    );
                    write_response(&resp);
                }
                _ => {
                    let resp = format!(
                        r#"{{"jsonrpc":"2.0","id":{},"result":{{"capabilities":{{}}}}}}"#,
                        id.unwrap_or(1)
                    );
                    write_response(&resp);
                }
            },
            "initialized" => match mode {
                "never-quiescent" => {
                    write_response(
                        r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":false,"message":"indexing"}}"#,
                    );
                }
                "warning-ready" => {
                    write_response(
                        r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"warning","quiescent":false,"message":"background diagnostics incomplete"}}"#,
                    );
                    write_response(
                        r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"warning","quiescent":true,"message":"usable with warning"}}"#,
                    );
                }
                "exit-before-ready" => {
                    std::process::exit(1);
                }
                "ready-sequence" => {
                    write_response(
                        r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":false,"message":"indexing"}}"#,
                    );
                    write_response(
                        r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":true,"message":"ready"}}"#,
                    );
                }
                _ => {
                    write_response(
                        r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":true,"message":"ready"}}"#,
                    );
                }
            },
            "textDocument/didOpen" => {}
            "shutdown" => {
                let resp = format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":null}}"#,
                    id.unwrap_or(1)
                );
                write_response(&resp);
            }
            "exit" => {
                std::process::exit(0);
            }
            "$/cancelRequest" => {}
            _ => {
                let req_id = id.unwrap_or(1);
                match mode {
                    "malformed" => {
                        let header = "Content-Length: 25\r\n\r\n";
                        let _ = stdout.write_all(header.as_bytes());
                        let _ = stdout.write_all(b"this is not valid json!!!");
                        let _ = stdout.flush();
                    }
                    "wrong-id" => {
                        let resp = r#"{"jsonrpc":"2.0","id":99999,"result":[]}"#;
                        write_response(resp);
                    }
                    "late-response" => {
                        thread::sleep(Duration::from_millis(3000));
                        let resp = format!(r#"{{"jsonrpc":"2.0","id":{req_id},"result":[]}}"#);
                        write_response(&resp);
                    }
                    "huge-response" => {
                        let header = "Content-Length: 52428800\r\n\r\n";
                        let _ = stdout.write_all(header.as_bytes());
                        let _ = stdout.flush();
                    }
                    "flood" => {
                        for i in 0..100 {
                            let notif = format!(
                                r#"{{"jsonrpc":"2.0","method":"window/logMessage","params":{{"type":3,"message":"flood_{i}"}}}}"#
                            );
                            write_response(&notif);
                        }
                        let resp = format!(r#"{{"jsonrpc":"2.0","id":{req_id},"result":[]}}"#);
                        write_response(&resp);
                    }
                    "exit-mid" => {
                        std::process::exit(1);
                    }
                    "semantic-filtered" | "semantic-fallback" | "ready-sequence"
                    | "never-quiescent" | "warning-ready" | "exit-before-ready" => {
                        let params = val.get("params").cloned().unwrap_or_default();
                        match method {
                            "workspace/symbol" => {
                                let query =
                                    params.get("query").and_then(|q| q.as_str()).unwrap_or("");

                                let (valid_request, canonical_query) = if mode
                                    == "semantic-filtered"
                                {
                                    let valid = params.get("searchScope").and_then(|v| v.as_str())
                                        == Some("workspace")
                                        && params.get("searchKind").and_then(|v| v.as_str())
                                            == Some("allSymbols");
                                    (valid, query)
                                } else {
                                    (
                                        query.ends_with('#'),
                                        query.strip_suffix('#').unwrap_or(query),
                                    )
                                };

                                let symbols = if !valid_request {
                                    serde_json::json!([])
                                } else {
                                    match canonical_query {
                                        "refresh_token" => serde_json::json!([
                                            {
                                                "name": "refresh_token",
                                                "kind": 12,
                                                "location": {
                                                    "uri": "file:///src/lib.rs",
                                                    "range": {
                                                        "start": {"line": 0, "character": 7},
                                                        "end": {"line": 0, "character": 20}
                                                    }
                                                }
                                            }
                                        ]),
                                        "duplicate" => serde_json::json!([
                                            {
                                                "name": "duplicate",
                                                "kind": 12,
                                                "location": {
                                                    "uri": "file:///src/lib.rs",
                                                    "range": {
                                                        "start": {"line": 4, "character": 7},
                                                        "end": {"line": 4, "character": 16}
                                                    }
                                                }
                                            },
                                            {
                                                "name": "duplicate",
                                                "kind": 12,
                                                "location": {
                                                    "uri": "file:///src/lib.rs",
                                                    "range": {
                                                        "start": {"line": 6, "character": 7},
                                                        "end": {"line": 6, "character": 16}
                                                    }
                                                }
                                            }
                                        ]),
                                        "SessionToken" => serde_json::json!([
                                            {
                                                "name": "SessionToken",
                                                "kind": 5,
                                                "location": {
                                                    "uri": "file:///src/lib.rs",
                                                    "range": {
                                                        "start": {"line": 8, "character": 11},
                                                        "end": {"line": 8, "character": 23}
                                                    }
                                                }
                                            }
                                        ]),
                                        _ => serde_json::json!([]),
                                    }
                                };

                                let resp = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "result": symbols
                                });
                                write_response(&resp.to_string());
                            }
                            "textDocument/references" => {
                                let refs = serde_json::json!([
                                    {
                                        "uri": "file:///src/lib.rs",
                                        "range": {
                                            "start": {"line": 0, "character": 7},
                                            "end": {"line": 0, "character": 20}
                                        }
                                    },
                                    {
                                        "uri": "file:///src/lib.rs",
                                        "range": {
                                            "start": {"line": 2, "character": 4},
                                            "end": {"line": 2, "character": 17}
                                        }
                                    }
                                ]);
                                let resp = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "result": refs
                                });
                                write_response(&resp.to_string());
                            }
                            _ => {
                                let resp =
                                    format!(r#"{{"jsonrpc":"2.0","id":{req_id},"result":[]}}"#);
                                write_response(&resp);
                            }
                        }
                    }
                    _ => {
                        let resp = format!(r#"{{"jsonrpc":"2.0","id":{req_id},"result":[]}}"#);
                        write_response(&resp);
                    }
                }
            }
        }
    }
}
