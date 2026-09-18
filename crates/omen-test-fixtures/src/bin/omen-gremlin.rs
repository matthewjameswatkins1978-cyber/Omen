use clap::Parser;
use std::io::{self, Read};
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
    stderr: Option<String>,

    #[arg(long)]
    read_stdin: bool,

    #[arg(long)]
    spawn_child: bool,

    #[arg(long)]
    write: Option<PathBuf>,

    #[arg(long)]
    sleep_ms: Option<u64>,

    #[arg(long)]
    print_env: Option<String>,

    #[arg(long, default_value_t = 0)]
    exit: i32,
}

#[allow(clippy::zombie_processes)]
fn main() {
    let args = GremlinArgs::parse();

    if let Some(msg) = args.stdout {
        println!("{msg}");
    }

    if let Some(err) = args.stderr {
        eprintln!("{err}");
    }

    if args.read_stdin {
        let mut buffer = String::new();
        let bytes_read = io::stdin().read_to_string(&mut buffer).unwrap_or(0);
        println!("READ_STDIN_BYTES:{bytes_read}");
    }

    if let Some(path) = args.write {
        std::fs::write(&path, b"gremlin-write-payload").expect("gremlin write failed");
    }

    if let Some(env_name) = args.print_env {
        let val = std::env::var(&env_name).unwrap_or_else(|_| "<UNSET>".into());
        println!("{env_name}={val}");
    }

    if args.spawn_child {
        let exe = std::env::current_exe().expect("failed to get current_exe");
        let child = Command::new(exe)
            .arg("--sleep-ms")
            .arg("10000")
            .spawn()
            .expect("gremlin failed to spawn child");
        println!("CHILD_SPAWNED:{}", child.id());
    }

    if let Some(ms) = args.sleep_ms {
        thread::sleep(Duration::from_millis(ms));
    }

    std::process::exit(args.exit);
}
