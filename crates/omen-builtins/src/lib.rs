//! Omen-owned read-only shell builtins (Portable Shell P1).
//!
//! These are in-process semantic operations, in the same trust class as the
//! `:action` dispatcher: Omen's own code, no child process is spawned and no
//! Tethers authority roundtrip is involved. That is honest only because every
//! builtin here is **read-only**: it inspects the filesystem or formats bytes
//! and never mutates anything outside its own output streams.
//!
//! Mutating shell operations (cp/mv/rm/mkdir/..., Portable Shell P2) do NOT
//! belong here; they go through Tethers admission like any other consequential
//! execution. Session-affecting intrinsics (`cd`, `dirs`, `history`) stay in
//! `omen-interactive`, which owns the session state they mutate.
//!
//! Contract per builtin:
//! - pure function of `(argv, cwd, env, stdin_bytes)`; no ambient process
//!   state is read except through [`BuiltinContext`];
//! - byte-preserving: inputs and outputs are `Vec<u8>`; text decoding is a
//!   rendering choice, never a semantic one (lossy only for display paths the
//!   builtin itself documents);
//! - deterministic across Windows and Linux for the same inputs, except where
//!   the OS truthfully differs (path separators, executable extensions);
//! - every result carries a small machine-readable [`BuiltinOutput::truth`]
//!   value so agents get typed truth first and bytes second.

mod filter;
mod fs;
mod listing;
mod terminal;
mod text;
mod walk;

use std::path::PathBuf;

pub use listing::{FileEntry, FileKind};

/// Execution context for one builtin invocation.
#[derive(Debug, Clone)]
pub struct BuiltinContext {
    /// Logical working directory of the invoking shell session.
    pub cwd: PathBuf,
    /// Environment as visible to the shell (`Vec` to preserve order for tests).
    pub env: Vec<(String, String)>,
    /// Bytes available on stdin (`cat` with no operands consumes these).
    pub stdin: Vec<u8>,
}

impl BuiltinContext {
    /// Looks up one environment variable by name.
    pub fn getenv(&self, name: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Splits `PATH` on the platform separator (`;` on Windows, `:` elsewhere).
    pub fn path_entries(&self) -> Vec<PathBuf> {
        match self.getenv("PATH") {
            Some(path) => std::env::split_paths(path).collect(),
            None => Vec::new(),
        }
    }
}

/// Result of one builtin invocation.
#[derive(Debug, Clone)]
pub struct BuiltinOutput {
    /// Process-style exit code (`0` success, `1` operand/usage failure).
    pub code: i32,
    /// Exact bytes for stdout.
    pub stdout: Vec<u8>,
    /// Exact bytes for stderr (diagnostics only, never data).
    pub stderr: Vec<u8>,
    /// Machine-readable typed truth describing what happened.
    pub truth: serde_json::Value,
}

impl BuiltinOutput {
    fn ok(stdout: Vec<u8>, truth: serde_json::Value) -> Self {
        Self {
            code: 0,
            stdout,
            stderr: Vec::new(),
            truth,
        }
    }

    fn failed(stderr: String, truth: serde_json::Value) -> Self {
        let mut stderr = stderr.into_bytes();
        if !stderr.ends_with(b"\n") {
            stderr.push(b'\n');
        }
        Self {
            code: 1,
            stdout: Vec::new(),
            stderr,
            truth,
        }
    }
}

/// Every builtin name this crate implements (aliases included).
///
/// Authority: the `match` arms in [`run_if_builtin`]. A consistency test
/// asserts the table and the dispatcher agree in both directions.
pub const BUILTIN_NAMES: &[&str] = &[
    "cat", "clear", "cut", "dir", "du", "echo", "find", "grep", "head", "help", "ls", "pwd",
    "printf", "readlink", "realpath", "sort", "stat", "tail", "tee", "tr", "tree", "uniq", "wc",
    "where", "which",
];

/// Returns `true` when `name` is implemented by this crate.
pub fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name)
}

/// Runs `argv` as a builtin when `argv[0]` names one.
///
/// Returns `None` when the command is not a builtin (the caller falls through
/// to external execution). Otherwise returns the output; `Err` is reserved
/// for internal failures that cannot be expressed as an exit code.
pub fn run_if_builtin(
    argv: &[String],
    ctx: &BuiltinContext,
) -> Option<Result<BuiltinOutput, BuiltinError>> {
    let (name, args) = argv.split_first()?;
    let output = match name.as_str() {
        "echo" => text::echo(args),
        "printf" => text::printf(args),
        "pwd" => fs::pwd(args, ctx),
        "cat" => fs::cat(args, ctx),
        "which" | "where" => fs::which(args, ctx),
        "realpath" => fs::realpath(args, ctx),
        "readlink" => fs::readlink(args, ctx),
        "clear" => terminal::clear(args),
        "head" => filter::head(args, ctx),
        "tail" => filter::tail(args, ctx),
        "tee" => filter::tee(args, ctx),
        "wc" => filter::wc(args, ctx),
        "sort" => filter::sort(args, ctx),
        "uniq" => filter::uniq(args, ctx),
        "cut" => filter::cut(args, ctx),
        "tr" => filter::tr(args, ctx),
        "grep" => filter::grep(args, ctx),
        "ls" | "dir" => listing::ls(args, ctx),
        "stat" => listing::stat(args, ctx),
        "find" => walk::find(args, ctx),
        "du" => walk::du(args, ctx),
        "tree" => walk::tree(args, ctx),
        "help" => help(args),
        _ => return None,
    };
    Some(Ok(output))
}

/// Static option words per builtin, for completion. A test below asserts
/// every registry name resolves here (optionless builtins resolve to an
/// explicit empty table); unknown names yield `&[]`. Mirrors the parsers;
/// `--` ends option parsing everywhere.
pub fn builtin_options(name: &str) -> &'static [&'static str] {
    match name {
        "cat" => &["-n", "--"],
        "clear" => &[],
        "cut" => &["-b", "-c", "-f", "-d", "-s", "-n", "--"],
        "dir" => &["-a", "-l", "-0", "-1", "-d", "--"],
        "du" => &["-s", "-a", "-b", "-k", "-m", "--"],
        "echo" => &["-n", "-e", "-E", "--"],
        "find" => &["-maxdepth", "-mindepth", "-type", "-name", "--"],
        "grep" => &[
            "-i", "-v", "-c", "-n", "-q", "-x", "-F", "-H", "-h", "-e", "--",
        ],
        "head" => &["-n", "--"],
        "help" => &[],
        "ls" => &["-a", "-l", "-0", "-1", "-d", "--"],
        "pwd" => &["-L", "-P"],
        "printf" => &[],
        "readlink" => &["-f", "--"],
        "realpath" => &["-m", "--"],
        "sort" => &["-r", "-n", "-u", "-f", "--"],
        "stat" => &["-L", "--"],
        "tail" => &["-n", "--"],
        "tee" => &["-a", "--"],
        "tr" => &["-c", "-d", "-s", "-t", "--"],
        "tree" => &["-a", "-d", "-L", "--"],
        "uniq" => &["-c", "-d", "-u", "-i", "--"],
        "wc" => &["-l", "-w", "-c", "--"],
        "where" => &["-a", "--"],
        "which" => &["-a", "--"],
        _ => &[],
    }
}

/// Session-owned words documented by `help` (not builtins; see `help_for`).
/// Single authority for the session-word side of completion and help tests.
pub const SESSION_WORDS: &[&str] = &["cd", "history", "jobs", "stop", "exit", "quit"];

/// Every `help` topic: registry builtins plus session words.
pub fn help_topics() -> Vec<&'static str> {
    BUILTIN_NAMES
        .iter()
        .copied()
        .chain(SESSION_WORDS.iter().copied())
        .collect()
}
///
/// Usage and notes per builtin: the single authority behind `help`.
///
/// Completion derives the name list from [`BUILTIN_NAMES`]; human detail
/// lives here, not in a second documentation copy.
pub fn help_for(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "cat" => (
            "cat [-n] [file ...]",
            "Concatenates files (or stdin) byte for byte. -n numbers lines.",
        ),
        "clear" => (
            "clear",
            "Emits the home-and-erase ANSI sequence. Takes no operands.",
        ),
        "cut" => (
            "cut -b LIST | -c LIST | -f LIST [-d DELIM] [-s] [file ...]",
            "Positions are BYTES (lossy over multibyte text). Only one of -b/-c/-f.",
        ),
        "dir" => ("dir [options] [path ...]", "Alias of ls."),
        "du" => (
            "du [-s] [-a] [-b|-k|-m] [path ...]",
            "Apparent sizes, never disk blocks. Default unit is bytes.",
        ),
        "echo" => (
            "echo [-n] [-e|-E] [word ...]",
            "Writes words joined by spaces. -n drops the newline.",
        ),
        "find" => (
            "find [path ...] [-maxdepth N] [-mindepth N] [-type f|d|l] [-name PATTERN]",
            "Visits everything (no ignore files); never follows symlinked dirs. -name is glob over the lossy name.",
        ),
        "grep" => (
            "grep [-i] [-v] [-c] [-n] [-q] [-x] [-e PATTERN] PATTERN [file ...]",
            "LITERAL substring match only (no regex; use rg). Exits 0 match, 1 none, 2 error.",
        ),
        "head" => (
            "head [-n N] [file ...]",
            "First N lines (default 10). Negative N drops the last |N| lines.",
        ),
        "help" => ("help [builtin]", "Lists builtins, or details one."),
        "ls" => (
            "ls [-a] [-l] [-0|-1] [-d] [path ...]",
            "Byte-sorted listing. -0 (NUL) is lossless; newline-delimited names are lossy.",
        ),
        "pwd" => (
            "pwd [-L|-P]",
            "Prints the session directory. -P resolves symlinks.",
        ),
        "printf" => (
            "printf format [arg ...]",
            "POSIX printf subset (%s %d %u %o %x %X %c %b %f %e %g %%).",
        ),
        "readlink" => (
            "readlink [-f] path ...",
            "Prints raw link targets; -f canonicalizes.",
        ),
        "realpath" => (
            "realpath [-m] path ...",
            "Canonical absolute paths. -m tolerates missing tails.",
        ),
        "sort" => (
            "sort [-r] [-n] [-u] [-f] [file ...]",
            "Stable line sort. -n is leading-number, -f folds ASCII case for comparison.",
        ),
        "stat" => (
            "stat [-L] path ...",
            "Typed metadata (size/type/readonly/mtime). -L follows symlinks.",
        ),
        "tail" => (
            "tail [-n N] [file ...]",
            "Last N lines (default 10). -n +N prints from line N.",
        ),
        "tee" => (
            "tee [-a]",
            "Copies stdin to stdout. File operands refuse closed (P2 authority pending).",
        ),
        "tr" => (
            "tr [-c] [-d] [-s] SET1 [SET2]",
            "Byte translation over stdin only. Ranges, escapes and [:upper:] classes.",
        ),
        "tree" => (
            "tree [-a] [-d] [-L LEVEL] [path ...]",
            "Renders directory drawing. Display only (always lossy projection).",
        ),
        "uniq" => (
            "uniq [-c] [-d] [-u] [-i] [file ...]",
            "Adjacent duplicate suppression. -i folds ASCII case.",
        ),
        "wc" => (
            "wc [-lwc] [file ...]",
            "Lines, words, bytes. -m is refused (chars lie over non-UTF-8).",
        ),
        "where" => ("where [-a] name ...", "Alias of which."),
        "which" => (
            "which [-a] name ...",
            "Locates executables on PATH. -a prints every hit.",
        ),
        // Session-owned words (not builtins): documented here so `help`
        // stays the single surface; each says who owns it.
        "cd" => (
            "cd [dir]",
            "Session navigation (session-owned, not a builtin).",
        ),
        "history" => (
            "history",
            "Session execution history display (session-owned).",
        ),
        "jobs" => ("jobs", "Lists session background jobs (session-owned)."),
        "stop" => (
            "stop <job-id>",
            "Stops a session background job (session-owned).",
        ),
        "exit" | "quit" => ("exit", "Leaves the shell (session-owned)."),
        _ => return None,
    })
}

/// `help [builtin]`: lists every builtin with its usage, or details one.
pub fn help(args: &[String]) -> BuiltinOutput {
    if args.len() > 1 {
        return BuiltinOutput::failed(
            "help: usage: help [builtin]".to_string(),
            serde_json::json!({"builtin": "help", "error": "usage"}),
        );
    }
    if let Some(name) = args.first() {
        return match help_for(name) {
            Some((usage, notes)) => BuiltinOutput::ok(
                format!("{usage}\n{notes}\n").into_bytes(),
                serde_json::json!({"builtin": "help", "topic": name}),
            ),
            None => BuiltinOutput::failed(
                format!("help: no builtin named {name:?}"),
                serde_json::json!({"builtin": "help", "error": "unknown_topic"}),
            ),
        };
    }
    let mut out = String::from("Omen builtins:\n");
    for name in BUILTIN_NAMES {
        let usage = help_for(name).map(|(u, _)| u).unwrap_or("?");
        out.push_str(&format!("  {usage}\n"));
    }
    BuiltinOutput::ok(
        out.into_bytes(),
        serde_json::json!({"builtin": "help", "topics": BUILTIN_NAMES}),
    )
}
/// One in-process pipeline stage: argv plus environment overrides.
#[derive(Debug, Clone)]
pub struct PipelineStage {
    pub argv: Vec<String>,
    /// Stage-scoped `NAME=value` pairs shadowing the base environment
    /// (notably `PATH` for `which`); the base still comes from `ctx`.
    pub env_overrides: Vec<(String, String)>,
}

/// Runs a whole pipeline in-process when EVERY stage head names a builtin.
///
/// Returns `None` when any stage is external (the caller falls through to
/// process spawn). Otherwise chains stdout→stdin byte-exactly: stage 0
/// reads `ctx.stdin`, each later stage reads the previous stage's stdout.
/// No child process is spawned and no broker is involved — sound only
/// because every builtin here is read-only (see crate docs).
///
/// Memory note: stages are buffered whole in memory (no streaming
/// backpressure like kernel pipes). Correct for shell-sized data; a future
/// streaming executor can replace the chain without changing the contract.
pub fn run_pipeline(
    stages: &[PipelineStage],
    ctx: &BuiltinContext,
) -> Option<Vec<Result<BuiltinOutput, BuiltinError>>> {
    if stages.is_empty() {
        return None;
    }
    for stage in stages {
        let (name, _) = stage.argv.split_first()?;
        if !is_builtin(name) {
            return None;
        }
    }
    let mut stdin = ctx.stdin.clone();
    let mut outputs = Vec::with_capacity(stages.len());
    for stage in stages {
        let mut env = ctx.env.clone();
        for (key, value) in &stage.env_overrides {
            if let Some(slot) = env.iter_mut().find(|(k, _)| k == key) {
                slot.1 = value.clone();
            } else {
                env.push((key.clone(), value.clone()));
            }
        }
        let stage_ctx = BuiltinContext {
            cwd: ctx.cwd.clone(),
            env,
            stdin,
        };
        let output = match run_if_builtin(&stage.argv, &stage_ctx) {
            Some(Ok(output)) => output,
            Some(Err(error)) => {
                outputs.push(Err(error));
                return Some(outputs);
            }
            None => unreachable!("gated by is_builtin above"),
        };
        stdin = output.stdout.clone();
        outputs.push(Ok(output));
    }
    Some(outputs)
}
/// Internal builtin failure (not an operand error; those are exit codes).
#[derive(Debug, thiserror::Error)]
pub enum BuiltinError {
    #[error("internal builtin failure: {0}")]
    Internal(String),
}

#[cfg(test)]
mod consistency_tests {
    use super::*;

    /// The registry table and the dispatcher agree in both directions:
    /// every listed name dispatches, and every dispatched name is listed.
    #[test]
    fn builtin_table_matches_dispatcher() {
        let ctx = BuiltinContext {
            cwd: std::path::PathBuf::from("."),
            env: Vec::new(),
            stdin: Vec::new(),
        };
        for name in BUILTIN_NAMES {
            let argv = [name.to_string()];
            assert!(
                run_if_builtin(&argv, &ctx).is_some(),
                "BUILTIN_NAMES lists '{name}' but run_if_builtin ignores it"
            );
        }
        // Unlisted names must fall through to external execution.
        for name in ["cd", "rm", "cargo", "git", "sed", "", ":status"] {
            let argv = [name.to_string()];
            assert!(
                run_if_builtin(&argv, &ctx).is_none(),
                "run_if_builtin claims unlisted command '{name}'"
            );
        }
    }

    /// Registry names and session intrinsics must not claim the same word.
    #[test]
    fn builtins_do_not_shadow_session_intrinsics() {
        for name in BUILTIN_NAMES {
            assert!(
                !matches!(*name, "cd" | "exit" | "quit"),
                "builtin '{name}' shadows a session intrinsic"
            );
        }
    }

    /// In-process pipelines chain stdout→stdin byte-exactly and refuse
    /// mixed builtin/external stages (those need real process spawn).
    #[test]
    fn pipeline_chains_bytes_and_gates_mixed_stages() {
        let ctx = BuiltinContext {
            cwd: std::path::PathBuf::from("."),
            env: Vec::new(),
            stdin: b"b\na\nb\n".to_vec(),
        };
        let stages = vec![
            PipelineStage {
                argv: vec!["sort".to_string()],
                env_overrides: Vec::new(),
            },
            PipelineStage {
                argv: vec!["uniq".to_string(), "-c".to_string()],
                env_overrides: Vec::new(),
            },
        ];
        let outputs = run_pipeline(&stages, &ctx).expect("all-builtin pipeline runs");
        assert_eq!(outputs.len(), 2);
        let last = outputs[1].as_ref().expect("uniq succeeds");
        assert_eq!(last.stdout, b"      1 a\n      2 b\n");
        // Mixed stages fall through to external execution.
        let mixed = vec![
            PipelineStage {
                argv: vec!["ls".to_string()],
                env_overrides: Vec::new(),
            },
            PipelineStage {
                argv: vec!["rg".to_string(), "rs".to_string()],
                env_overrides: Vec::new(),
            },
        ];
        assert!(run_pipeline(&mixed, &ctx).is_none());
        // Empty pipeline is not a pipeline.
        let empty: Vec<PipelineStage> = Vec::new();
        assert!(run_pipeline(&empty, &ctx).is_none());
    }

    /// Every registry name resolves to its own option table (optionless
    /// builtins resolve explicitly empty); unknown names yield `&[]`.
    #[test]
    fn every_builtin_has_an_option_table() {
        for name in BUILTIN_NAMES {
            let options = builtin_options(name);
            if matches!(*name, "clear" | "help" | "printf") {
                assert!(options.is_empty(), "{name} takes no options");
            } else {
                assert!(!options.is_empty(), "{name} is missing its option table");
            }
        }
        assert!(builtin_options("cargo").is_empty());
        assert!(builtin_options("").is_empty());
    }

    /// `help` covers every builtin plus the session-owned words (marked
    /// as such); anything else is an unknown topic.
    #[test]
    fn help_covers_builtins_and_session_words() {
        for name in BUILTIN_NAMES {
            assert!(help_for(name).is_some(), "help is missing builtin '{name}'");
        }
        for name in ["cd", "history", "jobs", "stop", "exit", "quit"] {
            let (usage, notes) = help_for(name).expect("session word documented");
            assert!(!usage.is_empty());
            assert!(
                notes.contains("session-owned"),
                "{name} must say who owns it"
            );
        }
        assert!(help_for("cargo").is_none());
    }

    /// `help_topics` is exactly the registry plus the session words, and
    /// every topic resolves to help text.
    #[test]
    fn help_topics_cover_builtins_and_session_words() {
        let topics = help_topics();
        assert_eq!(topics.len(), BUILTIN_NAMES.len() + SESSION_WORDS.len());
        for name in BUILTIN_NAMES.iter().chain(SESSION_WORDS.iter()) {
            assert!(topics.contains(name), "topic missing: {name}");
            assert!(help_for(name).is_some(), "topic without text: {name}");
        }
    }
}
