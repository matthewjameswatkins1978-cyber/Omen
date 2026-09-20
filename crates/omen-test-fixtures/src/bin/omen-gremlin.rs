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

    #[arg(long)]
    hostile_terminal_escapes: bool,

    #[arg(long)]
    spawn_child: bool,

    #[arg(long)]
    spawn_tree: Option<usize>,

    #[arg(long)]
    pty_echo: bool,

    #[arg(long)]
    write: Option<PathBuf>,

    #[arg(long)]
    sleep_ms: Option<u64>,

    #[arg(long)]
    print_env: Option<String>,

    #[arg(long, default_value_t = 0)]
    exit: i32,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    trailing_args: Vec<String>,
}

#[allow(clippy::zombie_processes)]
fn main() {
    let args = GremlinArgs::parse();

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

    if let Some(ms) = args.sleep_ms {
        thread::sleep(Duration::from_millis(ms));
    }

    std::process::exit(args.exit);
}
