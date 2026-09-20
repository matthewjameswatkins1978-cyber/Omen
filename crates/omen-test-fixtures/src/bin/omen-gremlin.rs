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

    #[arg(long)]
    lsp_mode: Option<String>,

    #[arg(long, default_value_t = 0)]
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
