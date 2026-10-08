//! Dogfood runner: pipes real stdin/argv through the builtin registry.
//!
//! Usage: `run <builtin> [args...] < stdin`. Writes raw stdout/stderr bytes
//! and exits with the builtin's code (127 when the name is not a builtin).
//! This exercises the exact `run_if_builtin` path the session dispatcher
//! calls, outside the unit-test harness.

use std::io::{Read, Write};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut stdin = Vec::new();
    if let Err(error) = std::io::stdin().read_to_end(&mut stdin) {
        eprintln!("run: stdin read failed: {error}");
        std::process::exit(3);
    }
    let ctx = omen_builtins::BuiltinContext {
        cwd: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        env: std::env::vars().collect(),
        stdin,
    };
    match omen_builtins::run_if_builtin(&argv, &ctx) {
        None => {
            eprintln!("run: not a builtin: {}", argv.first().map(String::as_str).unwrap_or(""));
            std::process::exit(127);
        }
        Some(Err(error)) => {
            eprintln!("run: internal failure: {error}");
            std::process::exit(3);
        }
        Some(Ok(out)) => {
            let _ = std::io::stdout().write_all(&out.stdout);
            let _ = std::io::stdout().flush();
            let _ = std::io::stderr().write_all(&out.stderr);
            std::process::exit(out.code);
        }
    }
}
