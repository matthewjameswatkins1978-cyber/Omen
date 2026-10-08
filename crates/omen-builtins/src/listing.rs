//! Directory listing truth: `ls`/`dir` and `stat`.
//!
//! Reference architecture for native semantic commands (packet phase E1):
//!
//! - canonical truth is [`FileEntry`] (typed, per-path) and every command
//!   carries it in `BuiltinOutput::truth` so agents get records first;
//! - human output is a rendering of those records, never a second source;
//! - the process-byte projection is DECLARED per invocation:
//!   `names-nul` is lossless, `names-nl` and `long` are lossy (a `\n` or
//!   non-UTF-8 name does not survive them). The truth always says which.
//!
//! P1 subset limits (honest, not silent): no owner/group/mode columns yet
//! (those need per-platform truth work), no `-R` recursion (`find` will own
//! traversal), long format shows mtime as unix seconds.

use super::{BuiltinContext, BuiltinOutput};
use serde_json::json;
use std::path::{Path, PathBuf};

/// Canonical typed truth for one filesystem path.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Raw filename bytes (exact on every platform via `as_encoded_bytes`).
    pub name_bytes: Vec<u8>,
    /// Lossy display name (only for human rendering and JSON truth).
    pub name_lossy: String,
    /// `true` when the lossy name lost information.
    pub name_is_lossy: bool,
    /// Full path as listed.
    pub path: PathBuf,
    /// What the path is (symlink is never followed here).
    pub file_type: FileKind,
    /// Metadata length (files: bytes; dirs/symlinks: platform value).
    pub len: u64,
    /// Readonly flag from metadata permissions.
    pub readonly: bool,
    /// mtime as unix seconds (`None` when unavailable or pre-epoch).
    pub modified_unix: Option<i64>,
    /// Raw link target bytes when `file_type == Symlink`.
    pub symlink_target: Option<Vec<u8>>,
}

/// What a path is, without following a final symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File,
    Dir,
    Symlink,
    Other,
}

impl FileKind {
    fn as_str(self) -> &'static str {
        match self {
            FileKind::File => "file",
            FileKind::Dir => "dir",
            FileKind::Symlink => "symlink",
            FileKind::Other => "other",
        }
    }

    fn long_char(self) -> u8 {
        match self {
            FileKind::File => b'f',
            FileKind::Dir => b'd',
            FileKind::Symlink => b'l',
            FileKind::Other => b'o',
        }
    }
}

fn raw_name(path: &Path) -> (Vec<u8>, String, bool) {
    let name = path.file_name().unwrap_or(path.as_os_str());
    let bytes = name.as_encoded_bytes().to_vec();
    let lossy = name.to_string_lossy().into_owned();
    let is_lossy = String::from_utf8(bytes.clone()).is_err();
    (bytes, lossy, is_lossy)
}

fn unix_secs(time: std::io::Result<std::time::SystemTime>) -> Option<i64> {
    time.ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Stats `path` itself (never follows a final symlink).
fn stat_entry(path: &Path) -> Result<FileEntry, String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        FileKind::Symlink
    } else if file_type.is_dir() {
        FileKind::Dir
    } else if file_type.is_file() {
        FileKind::File
    } else {
        FileKind::Other
    };
    let (name_bytes, name_lossy, name_is_lossy) = raw_name(path);
    let symlink_target = if kind == FileKind::Symlink {
        std::fs::read_link(path)
            .ok()
            .map(|t| t.as_os_str().as_encoded_bytes().to_vec())
    } else {
        None
    };
    Ok(FileEntry {
        name_bytes,
        name_lossy,
        name_is_lossy,
        path: path.to_path_buf(),
        file_type: kind,
        len: metadata.len(),
        readonly: metadata.permissions().readonly(),
        modified_unix: unix_secs(metadata.modified()),
        symlink_target,
    })
}

fn entry_json(entry: &FileEntry) -> serde_json::Value {
    json!({
        "name": entry.name_lossy,
        "name_lossy": entry.name_is_lossy,
        "path": entry.path.to_string_lossy(),
        "type": entry.file_type.as_str(),
        "size": entry.len,
        "readonly": entry.readonly,
        "modified_unix": entry.modified_unix,
        "symlink_target_lossy": entry.symlink_target.as_ref().map(|t| String::from_utf8_lossy(t).into_owned()),
    })
}

fn resolve_operand(cwd: &Path, operand: &str) -> PathBuf {
    let path = PathBuf::from(operand);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

/// `ls [-a] [-l] [-0|-1] [-d] [path ...]` (alias `dir`).
///
/// Default projection is one name per line (`names-nl`, LOSSY when a name
/// holds `\n` or non-UTF-8). `-0` switches to NUL-delimited names
/// (`names-nul`, LOSSLESS). `-l` renders long records (LOSSY rendering of
/// the same typed truth) and refuses `-0`. Entries sort by raw name bytes.
pub fn ls(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut all = false;
    let mut long = false;
    let mut nul = false;
    let mut dirs_as_files = false;
    let mut operands: &[String] = &[];
    for (index, arg) in args.iter().enumerate() {
        if arg == "--" {
            operands = &args[index + 1..];
            break;
        }
        if arg.starts_with('-') && arg.len() > 1 && arg != "-" {
            for flag in arg[1..].chars() {
                match flag {
                    'a' => all = true,
                    'l' => long = true,
                    '0' => nul = true,
                    '1' => {}
                    'd' => dirs_as_files = true,
                    _ => {
                        return BuiltinOutput::failed(
                            format!("ls: unsupported option '-{flag}' (only -a, -l, -0, -1, -d)"),
                            json!({"builtin": "ls", "error": "unsupported_option"}),
                        );
                    }
                }
            }
            if index + 1 == args.len() {
                operands = &[];
            }
            continue;
        }
        operands = &args[index..];
        break;
    }
    if long && nul {
        return BuiltinOutput::failed(
            "ls: -l and -0 cannot be combined".to_string(),
            json!({"builtin": "ls", "error": "usage"}),
        );
    }
    let operands: Vec<String> = if operands.is_empty() {
        vec![".".to_string()]
    } else {
        operands.to_vec()
    };
    let show_headers = operands.len() > 1;
    let mut out = Vec::new();
    let mut entries_json = Vec::new();
    let mut failures = Vec::new();
    let mut lossless = !long;
    let mut listed = 0u64;
    for (operand_index, operand) in operands.iter().enumerate() {
        let path = resolve_operand(&ctx.cwd, operand);
        let entry = match stat_entry(&path) {
            Ok(e) => e,
            Err(message) => {
                failures.push(format!("ls: {message}"));
                continue;
            }
        };
        let mut batch: Vec<FileEntry> = Vec::new();
        if entry.file_type == FileKind::Dir && !dirs_as_files {
            let read_dir = match std::fs::read_dir(&path) {
                Ok(rd) => rd,
                Err(error) => {
                    failures.push(format!("ls: {}: {error}", path.display()));
                    continue;
                }
            };
            for child in read_dir {
                // Unreadable entries are diagnostics, never silent drops.
                let child = match child {
                    Ok(child) => child,
                    Err(error) => {
                        failures.push(format!("ls: {}: {error}", path.display()));
                        continue;
                    }
                };
                match stat_entry(&child.path()) {
                    Ok(e) => batch.push(e),
                    Err(message) => failures.push(format!("ls: {message}")),
                }
            }
            if !all {
                batch.retain(|e| !e.name_lossy.starts_with('.'));
            }
            batch.sort_by(|a, b| a.name_bytes.cmp(&b.name_bytes));
        } else {
            batch.push(entry);
        }
        if show_headers {
            if operand_index > 0 {
                out.push(b'\n');
            }
            out.extend(format!("{operand}:\n").as_bytes());
        }
        for child in &batch {
            listed += 1;
            entries_json.push(entry_json(child));
            if child.name_is_lossy {
                lossless = false;
            }
            if long {
                out.push(child.file_type.long_char());
                out.extend(format!(" {:>12} ", child.len).as_bytes());
                match child.modified_unix {
                    Some(secs) => out.extend(format!("{secs:>12} ").as_bytes()),
                    None => out.extend(b"         unk "),
                }
                out.extend_from_slice(child.name_lossy.as_bytes());
                if child.file_type == FileKind::Symlink
                    && let Some(target) = &child.symlink_target
                {
                    out.extend(b" -> ");
                    out.extend_from_slice(String::from_utf8_lossy(target).as_bytes());
                }
                out.push(b'\n');
            } else if nul {
                out.extend_from_slice(&child.name_bytes);
                out.push(0);
            } else {
                // Newline-delimited names: lossy when a name holds `\n`
                // (it renders as two records) — declared, not hidden.
                if child.name_bytes.contains(&b'\n') {
                    lossless = false;
                }
                out.extend_from_slice(&child.name_bytes);
                out.push(b'\n');
            }
        }
    }
    let projection = if long {
        "long"
    } else if nul {
        "names-nul"
    } else {
        "names-nl"
    };
    let truth = json!({"builtin": "ls", "projection": projection, "lossless": lossless && !long,
                       "entries": entries_json, "listed": listed, "failures": failures.len()});
    if failures.is_empty() {
        BuiltinOutput::ok(out, truth)
    } else {
        let mut result = BuiltinOutput::failed(failures.join("\n"), truth);
        result.stdout = out;
        result
    }
}

/// `stat [-L] path ...`: typed metadata per operand.
///
/// `-L` follows a final symlink (otherwise `lstat` semantics). Human output
/// is a fixed small record (File/Size/Type/Readonly/Modified/Target);
/// owner/group/mode columns are out of the P1 subset and refused via the
/// unsupported-option error, not faked. Operands are required.
pub fn stat(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut follow = false;
    let mut operands: &[String] = &[];
    for (index, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "-L" | "--dereference" => {
                follow = true;
                if index + 1 == args.len() {
                    operands = &[];
                }
            }
            "--" => {
                operands = &args[index + 1..];
                break;
            }
            _ if arg.starts_with('-') && arg != "-" => {
                return BuiltinOutput::failed(
                    format!("stat: unsupported option {arg:?} (only -L)"),
                    json!({"builtin": "stat", "error": "unsupported_option"}),
                );
            }
            _ => {
                operands = &args[index..];
                break;
            }
        }
    }
    if operands.is_empty() {
        return BuiltinOutput::failed(
            "stat: usage: stat [-L] path ...".to_string(),
            json!({"builtin": "stat", "error": "usage"}),
        );
    }
    let mut out = Vec::new();
    let mut entries_json = Vec::new();
    let mut failures = Vec::new();
    for operand in operands {
        let path = resolve_operand(&ctx.cwd, operand);
        let entry = if follow {
            match std::fs::metadata(&path) {
                Ok(metadata) => {
                    let kind = if metadata.is_dir() {
                        FileKind::Dir
                    } else if metadata.is_file() {
                        FileKind::File
                    } else {
                        FileKind::Other
                    };
                    let (name_bytes, name_lossy, name_is_lossy) = raw_name(&path);
                    FileEntry {
                        name_bytes,
                        name_lossy,
                        name_is_lossy,
                        path: path.clone(),
                        file_type: kind,
                        len: metadata.len(),
                        readonly: metadata.permissions().readonly(),
                        modified_unix: unix_secs(metadata.modified()),
                        symlink_target: None,
                    }
                }
                Err(error) => {
                    failures.push(format!("stat: {}: {error}", path.display()));
                    continue;
                }
            }
        } else {
            match stat_entry(&path) {
                Ok(e) => e,
                Err(message) => {
                    failures.push(format!("stat: {message}"));
                    continue;
                }
            }
        };
        entries_json.push(entry_json(&entry));
        out.extend(format!("  File: {}\n", entry.path.display()).as_bytes());
        out.extend(
            format!(
                "  Size: {}\tType: {}\n",
                entry.len,
                entry.file_type.as_str()
            )
            .as_bytes(),
        );
        out.extend(format!("Readonly: {}\n", entry.readonly).as_bytes());
        match entry.modified_unix {
            Some(secs) => out.extend(format!("Modified: {secs}\n").as_bytes()),
            None => out.extend(b"Modified: unknown\n"),
        }
        if let Some(target) = &entry.symlink_target {
            out.extend(b"Target: ");
            out.extend_from_slice(String::from_utf8_lossy(target).as_bytes());
            out.push(b'\n');
        }
    }
    let truth = json!({"builtin": "stat", "follow": follow,
                       "entries": entries_json, "failures": failures.len()});
    if failures.is_empty() {
        BuiltinOutput::ok(out, truth)
    } else {
        let mut result = BuiltinOutput::failed(failures.join("\n"), truth);
        result.stdout = out;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BuiltinContext;

    fn ctx_for(dir: &Path) -> BuiltinContext {
        BuiltinContext {
            cwd: dir.to_path_buf(),
            env: Vec::new(),
            stdin: Vec::new(),
        }
    }

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("b.txt"), b"bb").expect("write");
        std::fs::write(dir.path().join("a.txt"), b"a").expect("write");
        std::fs::write(dir.path().join(".hidden"), b"h").expect("write");
        std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
        dir
    }

    #[test]
    fn ls_lists_sorted_without_dotfiles_by_default() {
        let dir = fixture();
        let out = ls(&s(&[]), &ctx_for(dir.path()));
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, b"a.txt\nb.txt\nsub\n");
        assert_eq!(out.truth["projection"], json!("names-nl"));
    }

    #[test]
    fn ls_all_shows_dotfiles_and_long_declares_lossy() {
        let dir = fixture();
        let out = ls(&s(&["-a"]), &ctx_for(dir.path()));
        assert_eq!(out.stdout, b".hidden\na.txt\nb.txt\nsub\n");
        let out = ls(&s(&["-l"]), &ctx_for(dir.path()));
        assert_eq!(out.code, 0);
        assert_eq!(out.truth["lossless"], json!(false));
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("a.txt") && text.contains('f') && text.contains('d'));
    }

    #[test]
    fn ls_nul_is_lossless_bytes() {
        let dir = fixture();
        let out = ls(&s(&["-0"]), &ctx_for(dir.path()));
        assert_eq!(out.code, 0);
        assert_eq!(out.truth["lossless"], json!(true));
        assert_eq!(out.stdout, b"a.txt\0b.txt\0sub\0");
    }

    /// A newline inside a name renders as two `names-nl` records: the
    /// projection must declare itself lossy while `-0` stays lossless.
    /// Unix-only: Windows forbids control characters in file names.
    #[cfg(unix)]
    #[test]
    fn ls_newline_name_declares_lossy_nl() {
        use std::os::unix::ffi::OsStrExt;
        let dir = fixture();
        let tricky = dir
            .path()
            .join(std::ffi::OsStr::from_bytes(b"with\nnewline"));
        std::fs::write(&tricky, b"x").expect("write");
        let out = ls(&s(&["-0"]), &ctx_for(dir.path()));
        assert_eq!(out.code, 0);
        assert_eq!(out.truth["lossless"], json!(true));
        assert!(out.stdout.windows(13).any(|w| w == b"with\nnewline\0"));
        // Same tree through names-nl declares itself lossy.
        let out = ls(&s(&[]), &ctx_for(dir.path()));
        assert_eq!(out.truth["lossless"], json!(false));
    }

    #[test]
    fn ls_missing_operand_keeps_partial_output() {
        let dir = fixture();
        let out = ls(&s(&["a.txt", "absent"]), &ctx_for(dir.path()));
        assert_eq!(out.code, 1);
        assert!(String::from_utf8_lossy(&out.stderr).contains("absent"));
        assert_eq!(out.truth["listed"], json!(1));
    }

    #[test]
    fn ls_d_lists_the_dir_itself() {
        let dir = fixture();
        let out = ls(&s(&["-d", "sub"]), &ctx_for(dir.path()));
        assert_eq!(out.stdout, b"sub\n");
    }

    #[test]
    fn stat_reports_typed_truth() {
        let dir = fixture();
        let out = stat(&s(&["a.txt"]), &ctx_for(dir.path()));
        assert_eq!(out.code, 0);
        assert_eq!(out.truth["entries"][0]["type"], json!("file"));
        assert_eq!(out.truth["entries"][0]["size"], json!(1));
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("Size: 1"));
        assert_eq!(stat(&s(&[]), &ctx_for(dir.path())).code, 1);
        assert_eq!(stat(&s(&["-c", "a.txt"]), &ctx_for(dir.path())).code, 1);
        assert_eq!(stat(&s(&["absent"]), &ctx_for(dir.path())).code, 1);
    }

    #[cfg(unix)]
    #[test]
    fn stat_sees_symlinks_without_following() {
        use std::os::unix::fs::symlink;
        let dir = fixture();
        symlink("a.txt", dir.path().join("link")).expect("symlink");
        let out = stat(&s(&["link"]), &ctx_for(dir.path()));
        assert_eq!(out.truth["entries"][0]["type"], json!("symlink"));
        let out = stat(&s(&["-L", "link"]), &ctx_for(dir.path()));
        assert_eq!(out.truth["entries"][0]["type"], json!("file"));
    }
}
