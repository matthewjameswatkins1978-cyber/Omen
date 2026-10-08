//! Portable-shell I/O redirect resolution (`<`, `>`, `>>`).
//!
//! Bounded subset, honestly refused remainder:
//!
//! - `< file` (stdin from file) is RESOLVED AND APPLIED: reads are safe.
//! - `> file` / `>> file` (stdout to file) are RESOLVED AND VALIDATED, but
//!   applying the write is REFUSED-CLOSED: file writes are consequential
//!   mutations and need the admitted Tethers host-filesystem capability
//!   (P2, owner decision pending — same posture as `omen-mutation`).
//!   The refusal names the missing capability instead of failing obscurely.
//! - fd forms (`2>`, `&>`, `>&2`, `<&0`) are refused: no fd plumbing yet.
//! - Input redirects are honored on single commands and on the FIRST
//!   pipeline stage only; elsewhere they are refused (documented subset).
//! - Output redirects are honored NOWHERE yet (refused-closed); when the
//!   capability lands, `PendingWrite` is the exact hookup point.
//!
//! Resolution never touches the target beyond reading (input) or
//! validating (output parent exists, overwrite noted). All paths resolve
//! against the session cwd; `..` is normalized lexically.

use crate::shell_grammar::{ShellCommand, ShellRedirectOperation, ShellRedirectTarget};
use omen_core::CoreError;
use std::path::{Path, PathBuf};

/// A validated stdout-file write waiting for filesystem authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingWrite {
    pub path: PathBuf,
    pub append: bool,
    /// The target already exists (informational for the future prompt).
    pub overwrites: bool,
}

/// What one command's redirects resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRedirects {
    /// Stdin bytes from the last `< file` (`None` when no input redirect;
    /// `Some(empty)` for an empty file — presence matters, not length).
    pub stdin: Option<Vec<u8>>,
    /// Stdout file write (refused-closed at apply time until P2 admission).
    pub output: Option<PendingWrite>,
}

fn unsupported(message: String) -> CoreError {
    CoreError::ExecutionFailedCode {
        code: omen_core::ErrorCode::Unsupported,
        message,
    }
}

fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// Expands a redirect target word to exactly one path value.
fn expand_target(
    word: &crate::shell_grammar::ShellWord,
    cwd: &Path,
    op: &str,
) -> Result<PathBuf, CoreError> {
    let values = crate::shell_grammar::expand_word(word, cwd)
        .map_err(|error| unsupported(format!("{op}: target expansion failed: {error}")))?;
    if values.len() != 1 {
        return Err(unsupported(format!(
            "{op}: target must expand to exactly one path (got {})",
            values.len()
        )));
    }
    let path = PathBuf::from(&values[0]);
    Ok(normalize(&if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }))
}

/// Resolves one command's redirects. Pure except for reading `<` files:
/// resolution fails closed on missing input, ambiguous targets, and any
/// fd form.
pub fn resolve_command_redirects(
    command: &ShellCommand,
    cwd: &Path,
) -> Result<ResolvedRedirects, CoreError> {
    let mut stdin: Option<Vec<u8>> = None;
    let mut output: Option<PendingWrite> = None;
    for redirect in &command.redirects {
        if redirect.fd.is_some() {
            return Err(unsupported(
                "redirection with explicit fds (2>, &>, >&fd) is not supported in P3".to_string(),
            ));
        }
        match (&redirect.operation, &redirect.target) {
            (ShellRedirectOperation::Input, ShellRedirectTarget::Word(word)) => {
                let path = expand_target(word, cwd, "input redirect")?;
                stdin = Some(std::fs::read(&path).map_err(|error| {
                    CoreError::ExecutionFailed(format!(
                        "input redirect: {}: {error}",
                        path.display()
                    ))
                })?);
            }
            (
                ShellRedirectOperation::Overwrite | ShellRedirectOperation::Append,
                ShellRedirectTarget::Word(word),
            ) => {
                let op = if redirect.operation == ShellRedirectOperation::Overwrite {
                    ">"
                } else {
                    ">>"
                };
                let path = expand_target(word, cwd, "output redirect")?;
                if let Some(parent) = path.parent()
                    && !parent.exists()
                {
                    return Err(unsupported(format!(
                        "{op}: destination directory {} does not exist",
                        parent.display()
                    )));
                }
                let _ = op;
                output = Some(PendingWrite {
                    overwrites: path.exists(),
                    path,
                    append: redirect.operation == ShellRedirectOperation::Append,
                });
            }
            (_, ShellRedirectTarget::Fd(_)) => {
                return Err(unsupported(
                    "fd-duplicating redirects (>&fd, <&fd) are not supported in P3".to_string(),
                ));
            }
        }
    }
    Ok(ResolvedRedirects { stdin, output })
}

/// Applies a resolved output write. Currently ALWAYS refuses closed: file
/// writes need the admitted host-filesystem capability (P2). The single
/// choke point makes the future hookup one branch.
pub fn apply_output_write(_pending: &PendingWrite, _bytes: &[u8]) -> Result<(), CoreError> {
    Err(unsupported(
        "redirecting stdout to a file needs admitted filesystem authority: \
         no Tethers host-execution filesystem capability is admitted yet \
         (P2 owner decision pending)"
            .to_string(),
    ))
}

/// Resolves redirects across a whole pipeline: input from the FIRST stage
/// only (it feeds the chain), output validated last-wins across stages.
/// The caller applies the output refusal BEFORE running anything.
pub fn resolve_pipeline_redirects(
    pipeline: &crate::shell_grammar::ShellPipeline,
    cwd: &Path,
) -> Result<(Option<Vec<u8>>, Option<PendingWrite>), CoreError> {
    let mut stdin: Option<Vec<u8>> = None;
    let mut output: Option<PendingWrite> = None;
    for (index, command) in pipeline.commands.iter().enumerate() {
        let resolved = resolve_command_redirects(command, cwd)?;
        if let Some(bytes) = resolved.stdin {
            if index != 0 {
                return Err(unsupported(
                    "input redirects (<) are honored on the first pipeline stage only in P3"
                        .to_string(),
                ));
            }
            stdin = Some(bytes);
        }
        if resolved.output.is_some() {
            output = resolved.output;
        }
    }
    Ok((stdin, output))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_command(line: &str) -> ShellCommand {
        let parsed = crate::shell_grammar::parse(line).expect("parses");
        assert_eq!(parsed.items.len(), 1);
        match parsed.items.into_iter().next().expect("item").sequence {
            crate::shell_grammar::ShellSequence::BooleanChain { first, .. } => {
                assert_eq!(first.commands.len(), 1);
                first.commands.into_iter().next().expect("command")
            }
        }
    }

    fn ctx() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn input_redirect_reads_bytes() {
        let dir = ctx();
        std::fs::write(dir.path().join("in.txt"), b"a\0b\n").expect("write");
        let command = parse_command("cat < in.txt");
        let resolved = resolve_command_redirects(&command, dir.path()).expect("resolves");
        assert_eq!(resolved.stdin, Some(b"a\0b\n".to_vec()));
        assert!(resolved.output.is_none());
    }

    #[test]
    fn empty_input_file_feeds_empty_not_chain() {
        // Presence matters: `< empty` must feed empty bytes, not fall
        // through to chain/terminal stdin.
        let dir = ctx();
        std::fs::write(dir.path().join("empty.txt"), b"").expect("write");
        let command = parse_command("cat < empty.txt");
        let resolved = resolve_command_redirects(&command, dir.path()).expect("resolves");
        assert_eq!(resolved.stdin, Some(Vec::new()));
    }

    #[test]
    fn missing_input_fails_closed() {
        let dir = ctx();
        let command = parse_command("cat < absent.txt");
        let error = resolve_command_redirects(&command, dir.path()).expect_err("must refuse");
        assert!(format!("{error:?}").contains("absent.txt"));
    }

    #[test]
    fn output_redirect_validates_but_apply_refuses() {
        let dir = ctx();
        let command = parse_command("echo hi > out.txt");
        let resolved = resolve_command_redirects(&command, dir.path()).expect("resolves");
        let pending = resolved.output.expect("pending write");
        assert!(!pending.append && !pending.overwrites);
        let error = apply_output_write(&pending, b"hi\n").expect_err("refused closed");
        assert!(format!("{error:?}").contains("admitted filesystem authority"));
        // Missing parent is a resolution error, not an authority error.
        let command = parse_command("echo hi > nodir/out.txt");
        assert!(resolve_command_redirects(&command, dir.path()).is_err());
    }

    #[test]
    fn fd_forms_are_refused() {
        let dir = ctx();
        for line in ["echo hi 2> err.txt", "cat >&2", "cat <&0"] {
            let parsed = crate::shell_grammar::parse(line);
            match parsed {
                Err(_) => {}
                Ok(_) => {
                    let command = parse_command(line);
                    assert!(
                        resolve_command_redirects(&command, dir.path()).is_err(),
                        "{line}"
                    );
                }
            }
        }
    }
}
