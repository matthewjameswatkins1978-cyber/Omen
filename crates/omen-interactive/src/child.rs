use omen_core::{CoreError, ProcessExit};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildClassification {
    /// Full-screen interactive application requiring terminal handoff
    InteractiveHandoff,
    /// Standard supervised non-interactive execution
    StandardSupervised,
}

pub struct ChildHandoff;

impl ChildHandoff {
    /// Classifies an invocation based on Tool Atlas knowledge, subcommand semantics,
    /// and invocation-specific TTY requirements.
    pub fn classify(argv: &[String]) -> ChildClassification {
        if argv.is_empty() {
            return ChildClassification::StandardSupervised;
        }

        // 1. Explicit truthful override: user prefixed with `--interactive` or `--handoff`
        if argv[0] == "--interactive" || argv[0] == "--handoff" {
            return ChildClassification::InteractiveHandoff;
        }

        let cmd = &argv[0];
        let name = Path::new(cmd)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(cmd)
            .to_lowercase();

        // 2. Always interactive full-screen curses/terminal apps
        match name.as_str() {
            "vi" | "vim" | "nvim" | "nano" | "emacs" | "helix" | "hx" | "kak" | "less" | "more"
            | "man" | "top" | "htop" | "btop" | "ssh" | "telnet" | "tmux" | "screen" => {
                return ChildClassification::InteractiveHandoff;
            }
            _ => {}
        }

        // 3. Invocation-specific interpreter classification (python, node, ruby, etc.)
        // When invoked with no script arguments, it's an interactive REPL session.
        // When invoked with script arguments (e.g. `python script.py`), it is non-interactive!
        if matches!(
            name.as_str(),
            "python" | "python3" | "py" | "node" | "ruby" | "perl" | "lua"
        ) {
            let args = &argv[1..];
            if args.is_empty()
                || (args.len() == 1 && (args[0] == "-i" || args[0] == "--interactive"))
            {
                return ChildClassification::InteractiveHandoff;
            }
            return ChildClassification::StandardSupervised;
        }

        // 4. Invocation-specific Git commands
        // `git commit` without `-m`/`-F` launches the configured editor interactively!
        // `git rebase -i` launches interactive rebase editor!
        // `git add -p` / `git add -i` launches interactive staging!
        if name == "git" && argv.len() > 1 {
            let sub = &argv[1];
            if sub == "commit" {
                let has_msg = argv[2..].iter().any(|a| {
                    a == "-m"
                        || a.starts_with("-m")
                        || a == "--message"
                        || a.starts_with("--message=")
                        || a == "-F"
                        || a == "--file"
                        || a.starts_with("--file=")
                });
                if !has_msg {
                    return ChildClassification::InteractiveHandoff;
                }
            } else if (sub == "rebase"
                && argv[2..].iter().any(|a| a == "-i" || a == "--interactive"))
                || (sub == "add"
                    && argv[2..]
                        .iter()
                        .any(|a| a == "-p" || a == "--patch" || a == "-i" || a == "--interactive"))
            {
                return ChildClassification::InteractiveHandoff;
            }
        }

        // 5. Default truthful stance: where Omen cannot know, do not guess
        ChildClassification::StandardSupervised
    }

    /// Backwards-compatible helper
    pub fn is_interactive_command(cmd: &str) -> bool {
        Self::classify(&[cmd.to_string()]) == ChildClassification::InteractiveHandoff
    }

    /// Spawns an interactive child with full terminal ownership.
    pub fn spawn_interactive(argv: &[String], cwd: &Path) -> Result<ProcessExit, CoreError> {
        if argv.is_empty() {
            return Err(CoreError::ExecutionFailed("argv cannot be empty".into()));
        }

        // Filter out explicit override flag if present
        let actual_argv: Vec<String> = if argv[0] == "--interactive" || argv[0] == "--handoff" {
            argv[1..].to_vec()
        } else {
            argv.to_vec()
        };

        if actual_argv.is_empty() {
            return Err(CoreError::ExecutionFailed(
                "No command specified after override".into(),
            ));
        }

        let mut cmd = Command::new(&actual_argv[0]);
        if actual_argv.len() > 1 {
            cmd.args(&actual_argv[1..]);
        }
        cmd.current_dir(cwd);

        // Inherit stdin, stdout, and stderr directly from the console
        cmd.stdin(std::process::Stdio::inherit());
        cmd.stdout(std::process::Stdio::inherit());
        cmd.stderr(std::process::Stdio::inherit());

        let mut child = cmd.spawn().map_err(|e| {
            CoreError::ExecutionFailed(format!(
                "Failed to spawn interactive child {:?}: {e}",
                actual_argv[0]
            ))
        })?;

        let status = child.wait().map_err(|e| {
            CoreError::ExecutionFailed(format!("Failed to wait on interactive child: {e}"))
        })?;

        Ok(ProcessExit {
            code: status.code(),
            signal: None,
        })
    }
}
