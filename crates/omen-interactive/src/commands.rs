//! Omen-owned command and action authority.
//!
//! This module is the single source of truth for:
//! - shell intrinsic commands handled by the interactive session itself
//! - canonical `:action` names accepted by [`crate::actions::SemanticDispatcher`]
//! - static subcommand syntax metadata for well-known external tools
//!
//! Completion and dispatch MUST consume these definitions rather than
//! maintaining parallel handwritten lists.
//!
//! External executables (cargo, git, ls, ...) are NOT listed here. Their
//! truthful source is executable availability on `PATH`; see
//! [`list_path_commands`] for the bounded out-of-hot-path discovery used by
//! completion.

/// Shell intrinsic commands handled by the interactive session itself.
///
/// Authority: `InteractiveSession::dispatch_input` (cd navigation, exit/quit).
pub const SHELL_INTRINSICS: &[&str] = &["cd", "exit", "quit"];

/// Canonical Omen semantic action names accepted by `SemanticDispatcher`.
///
/// Authority: the `match action` arms in `SemanticDispatcher::dispatch`.
/// The dispatcher's unknown-action diagnostic is generated from this list so
/// the names cannot drift apart silently. A consistency test asserts every
/// entry is accepted by dispatch and every unlisted name is rejected.
pub const OMEN_ACTIONS: &[&str] = &[
    "actions",
    "agent",
    "backend",
    "capabilities",
    "def",
    "describe",
    "doctor",
    "history",
    "how",
    "inspect",
    "orient",
    "packages",
    "plan",
    "refs",
    "rerun",
    "services",
    "show",
    "status",
    "stop",
    "structure",
    "symbol",
    "tasks",
    "tools",
    "why",
];

/// Returns `true` when `name` is a canonical Omen semantic action.
pub fn is_omen_action(name: &str) -> bool {
    OMEN_ACTIONS.contains(&name)
}

/// Returns `true` when `name` is a shell intrinsic handled by the session.
pub fn is_shell_intrinsic(name: &str) -> bool {
    SHELL_INTRINSICS.contains(&name)
}

/// Renders the unknown-action diagnostic from the shared authority.
pub fn unknown_action_message(action: &str) -> String {
    let mut listed: Vec<String> = OMEN_ACTIONS.iter().map(|a| format!(":{a}")).collect();
    listed.sort();
    format!(
        "Unknown Omen semantic action ':{}'. Available: {}",
        action,
        listed.join(", ")
    )
}

/// First-argument subcommands for multi-arity Omen semantic actions.
///
/// These are syntax metadata only (which words are canonical arguments), not
/// effect semantics. Empty slice means the action takes free-form arguments.
pub fn omen_action_subcommands(action: &str) -> &'static [&'static str] {
    match action {
        "agent" => &["providers", "status", "use"],
        "backend" => &["list", "status", "use"],
        _ => &[],
    }
}

/// Static subcommand syntax for well-known external tools.
///
/// This is syntax knowledge only (the shape of canonical argv after the tool
/// name). It does not assert that the tool exists; command-position
/// availability comes from the bounded PATH cache. Unlisted tools simply yield
/// no subcommand candidates.
pub fn tool_subcommands(tool: &str) -> &'static [&'static str] {
    match tool {
        "cargo" => &[
            "bench", "build", "check", "clean", "clippy", "doc", "fmt", "run", "test",
        ],
        "git" => &[
            "add", "branch", "checkout", "commit", "diff", "fetch", "log", "merge", "pull", "push",
            "status", "switch",
        ],
        "rg" | "ripgrep" => &["--files", "--help", "--version"],
        "threadmoth" => &["apply-plan", "doctor", "mutate", "plan", "replace-exact"],
        _ => &[],
    }
}

/// Bounded PATH executable discovery.
///
/// Out-of-hot-path only. Never call this on a keystroke. Results are
/// deterministic (sorted, deduplicated) and truncated to the supplied bounds.
/// On Windows only PATHEXT-recognised executable forms are accepted and the
/// extension is stripped to form the command name. PowerShell aliases are NOT
/// invented: a name appears only when a matching executable actually exists.
pub fn list_path_commands(
    max_dirs: usize,
    max_entries_per_dir: usize,
    max_commands: usize,
) -> Vec<String> {
    use std::collections::BTreeSet;
    use std::path::Path;

    let mut names: BTreeSet<String> = BTreeSet::new();
    let Some(path_var) = std::env::var_os("PATH") else {
        return Vec::new();
    };

    let path_ext: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase())
            .collect()
    } else {
        Vec::new()
    };

    for dir in std::env::split_paths(&path_var).take(max_dirs) {
        if names.len() >= max_commands {
            break;
        }
        if !Path::new(&dir).is_dir() {
            continue;
        }
        let Ok(read_dir) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read_dir.flatten().take(max_entries_per_dir) {
            if names.len() >= max_commands {
                break;
            }
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            if !entry.path().is_file() {
                continue;
            }
            if cfg!(windows) {
                let Some(dot) = name.rfind('.') else {
                    continue;
                };
                let ext = name[dot..].to_ascii_lowercase();
                if !path_ext.contains(&ext) {
                    continue;
                }
                let stem = &name[..dot];
                if stem.is_empty() {
                    continue;
                }
                names.insert(stem.to_string());
            } else {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let Ok(meta) = entry.metadata() else {
                        continue;
                    };
                    if meta.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
                names.insert(name.to_string());
            }
        }
    }

    names.into_iter().take(max_commands).collect()
}
