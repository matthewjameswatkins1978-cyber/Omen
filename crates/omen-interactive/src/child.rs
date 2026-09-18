use omen_core::{CoreError, ProcessExit};
use std::path::Path;
use std::process::Command;

pub struct ChildHandoff;

impl ChildHandoff {
    /// Determines whether an executable is an interactive application requiring terminal handoff.
    pub fn is_interactive_command(cmd: &str) -> bool {
        let name = Path::new(cmd)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(cmd)
            .to_lowercase();

        matches!(
            name.as_str(),
            "vi" | "vim"
                | "nvim"
                | "nano"
                | "emacs"
                | "less"
                | "more"
                | "top"
                | "htop"
                | "ssh"
                | "python"
                | "node"
        )
    }

    /// Spawns an interactive child with full terminal ownership.
    pub fn spawn_interactive(argv: &[String], cwd: &Path) -> Result<ProcessExit, CoreError> {
        if argv.is_empty() {
            return Err(CoreError::ExecutionFailed("argv cannot be empty".into()));
        }

        let mut cmd = Command::new(&argv[0]);
        if argv.len() > 1 {
            cmd.args(&argv[1..]);
        }
        cmd.current_dir(cwd);

        // Inherit stdin, stdout, and stderr directly from the console
        cmd.stdin(std::process::Stdio::inherit());
        cmd.stdout(std::process::Stdio::inherit());
        cmd.stderr(std::process::Stdio::inherit());

        let mut child = cmd.spawn().map_err(|e| {
            CoreError::ExecutionFailed(format!(
                "Failed to spawn interactive child {:?}: {e}",
                argv[0]
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
