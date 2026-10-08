//! Read-only filesystem builtins: `pwd`, `cat`, `which`/`where`,
//! `realpath`, `readlink`.

use super::{BuiltinContext, BuiltinOutput};
use serde_json::json;
use std::path::{Path, PathBuf};

/// `pwd [-L|-P]`: prints the session working directory.
///
/// `-L` (default) prints the logical directory; `-P` prints the physical
/// directory with symlinks resolved. No filesystem access on `-L`.
pub fn pwd(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut physical = false;
    for arg in args {
        match arg.as_str() {
            "-P" => physical = true,
            "-L" => physical = false,
            _ => {
                return BuiltinOutput::failed(
                    format!("pwd: unexpected operand {arg:?}"),
                    json!({"builtin": "pwd", "error": "unexpected_operand"}),
                );
            }
        }
    }
    let dir = if physical {
        ctx.cwd.canonicalize().unwrap_or_else(|_| ctx.cwd.clone())
    } else {
        ctx.cwd.clone()
    };
    let mut out = dir.to_string_lossy().into_owned().into_bytes();
    out.push(b'\n');
    BuiltinOutput::ok(
        out,
        json!({"builtin": "pwd", "cwd": dir.to_string_lossy(), "physical": physical}),
    )
}

/// `cat [-n] [file ...]`: concatenates files (or stdin) to stdout, byte for
/// byte. No decoding, no translation: NULs, invalid UTF-8 and lone CRs pass
/// through untouched. `-n` numbers output lines (`%6d\t` prefix, GNU style).
///
/// Missing/unreadable files are diagnostics on stderr; `cat` continues with
/// the remaining operands and exits 1 if any operand failed.
pub fn cat(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut number_lines = false;
    let mut operands: &[String] = &[];
    for (index, arg) in args.iter().enumerate() {
        if !arg.starts_with('-') || arg == "-" || arg.len() == 1 {
            operands = &args[index..];
            break;
        }
        if arg == "--" {
            operands = &args[index + 1..];
            break;
        }
        let mut known = true;
        for flag in arg[1..].chars() {
            if flag == 'n' {
                number_lines = true;
            } else {
                known = false;
                break;
            }
        }
        if !known {
            operands = &args[index..];
            break;
        }
        if index + 1 == args.len() {
            operands = &[];
        }
    }

    let mut out = Vec::new();
    let mut missing = Vec::new();
    let mut bytes: u64 = 0;
    let mut files: u64 = 0;
    let mut line_number: u64 = 0;

    let emit = |data: &[u8], out: &mut Vec<u8>, line_number: &mut u64| {
        if !number_lines {
            out.extend_from_slice(data);
            return;
        }
        // Numbering is line-oriented over `\n`-terminated records; a final
        // unterminated tail still counts as a line (GNU behavior).
        let mut start = 0;
        for (index, byte) in data.iter().enumerate() {
            if *byte == b'\n' {
                *line_number += 1;
                out.extend(format!("{:>6}\t", *line_number).as_bytes());
                out.extend_from_slice(&data[start..=index]);
                start = index + 1;
            }
        }
        if start < data.len() {
            *line_number += 1;
            out.extend(format!("{:>6}\t", *line_number).as_bytes());
            out.extend_from_slice(&data[start..]);
        }
    };

    if operands.is_empty() {
        bytes += ctx.stdin.len() as u64;
        emit(&ctx.stdin.clone(), &mut out, &mut line_number);
    } else {
        for operand in operands {
            let path = resolve_operand(&ctx.cwd, operand);
            match std::fs::read(&path) {
                Ok(data) => {
                    files += 1;
                    bytes += data.len() as u64;
                    emit(&data, &mut out, &mut line_number);
                }
                Err(error) => missing.push(format!("cat: {}: {error}", path.display())),
            }
        }
    }
    if missing.is_empty() {
        BuiltinOutput::ok(
            out,
            json!({"builtin": "cat", "files": files, "bytes": bytes, "numbered": number_lines}),
        )
    } else {
        let mut result = BuiltinOutput::failed(
            missing.join("\n"),
            json!({"builtin": "cat", "files": files, "bytes": bytes, "missing": missing.len()}),
        );
        result.stdout = out;
        result
    }
}

/// `which [-a] name ...` (alias `where`): locates executables on `PATH`.
///
/// A name containing a path separator is tested directly (executable bit on
/// Unix, extension/PATHEXT presence on Windows). Otherwise each `PATH` entry
/// is probed in order; `-a` prints every hit, otherwise only the first.
/// Exit 0 iff every name resolved to at least one executable.
pub fn which(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut all = false;
    let mut names: &[String] = &[];
    for (index, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "-a" => {
                all = true;
                if index + 1 == args.len() {
                    names = &[];
                }
            }
            "--" => {
                names = &args[index + 1..];
                break;
            }
            _ => {
                names = &args[index..];
                break;
            }
        }
    }
    if names.is_empty() {
        return BuiltinOutput::failed(
            "which: usage: which [-a] name ...".to_string(),
            json!({"builtin": "which", "error": "missing_name"}),
        );
    }
    let path_entries = ctx.path_entries();
    let mut out = Vec::new();
    let mut found: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    let mut absent = Vec::new();
    for name in names {
        let hits = locate_executable(name, &path_entries, &ctx.cwd);
        if hits.is_empty() {
            absent.push(format!("which: no {name} in PATH"));
        } else {
            let display: Vec<String> = if all {
                hits.iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect()
            } else {
                vec![hits[0].to_string_lossy().into_owned()]
            };
            for hit in &display {
                out.extend(hit.as_bytes());
                out.push(b'\n');
            }
            found.insert(name.clone(), json!(display));
        }
    }
    if absent.is_empty() {
        BuiltinOutput::ok(out, json!({"builtin": "which", "found": found}))
    } else {
        let mut result = BuiltinOutput::failed(
            absent.join("\n"),
            json!({"builtin": "which", "found": found, "absent": absent.len()}),
        );
        result.stdout = out;
        result
    }
}

fn locate_executable(name: &str, path_entries: &[PathBuf], cwd: &Path) -> Vec<PathBuf> {
    let candidate = PathBuf::from(name);
    let has_separator = name.contains('/') || name.contains('\\');
    if has_separator {
        let path = if candidate.is_absolute() {
            candidate
        } else {
            cwd.join(candidate)
        };
        return extend_with_windows_exts(&path)
            .into_iter()
            .filter(|path| is_executable_file(path))
            .collect();
    }
    let mut hits = Vec::new();
    for dir in path_entries {
        let base = dir.join(name);
        for probe in extend_with_windows_exts(&base) {
            if is_executable_file(&probe) {
                hits.push(probe);
                // First hit per directory wins; further directories are
                // still probed so `-a` reports every PATH location.
                break;
            }
        }
    }
    hits
}

/// On Windows, bare names resolve through `PATHEXT`; elsewhere the name is
/// probed verbatim. Returns the candidate list in probe order.
fn extend_with_windows_exts(base: &Path) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        if base.extension().is_some() {
            return vec![base.to_path_buf()];
        }
        let pathext = std::env::var_os("PATHEXT")
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
        let mut probes = Vec::new();
        for ext in pathext.split(';') {
            let ext = ext.trim();
            if ext.is_empty() {
                continue;
            }
            probes.push(base.with_extension(ext.trim_start_matches('.')));
        }
        probes
    }
    #[cfg(not(windows))]
    {
        vec![base.to_path_buf()]
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// `realpath [-m] path ...`: prints the canonical absolute path.
///
/// Without `-m` every operand must exist (canonicalize). With `-m`
/// (`--missing-ok`) nonexistent tails are resolved lexically against the
/// existing prefix. All operands must be given; exit 1 lists failures.
pub fn realpath(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut missing_ok = false;
    let mut operands: &[String] = &[];
    for (index, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "-m" | "--missing-ok" => {
                missing_ok = true;
                if index + 1 == args.len() {
                    operands = &[];
                }
            }
            "--" => {
                operands = &args[index + 1..];
                break;
            }
            _ => {
                operands = &args[index..];
                break;
            }
        }
    }
    resolve_paths("realpath", operands, missing_ok, ctx, |path| {
        path.canonicalize()
            .map_err(|error| format!("{}: {error}", path.display()))
    })
}

/// `readlink [-f] path ...`: prints the target of each symlink.
///
/// Without `-f` the raw link target is printed (relative targets are NOT
/// resolved). With `-f` the target is canonicalized like `realpath`.
pub fn readlink(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut canonical = false;
    let mut operands: &[String] = &[];
    for (index, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "-f" => {
                canonical = true;
                if index + 1 == args.len() {
                    operands = &[];
                }
            }
            "--" => {
                operands = &args[index + 1..];
                break;
            }
            _ => {
                operands = &args[index..];
                break;
            }
        }
    }
    if canonical {
        return resolve_paths("readlink", operands, false, ctx, |path| {
            path.canonicalize()
                .map_err(|error| format!("{}: {error}", path.display()))
        });
    }
    if operands.is_empty() {
        return BuiltinOutput::failed(
            "readlink: usage: readlink [-f] path ...".to_string(),
            json!({"builtin": "readlink", "error": "missing_operand"}),
        );
    }
    let mut out = Vec::new();
    let mut failures = Vec::new();
    let mut resolved = serde_json::Map::new();
    for operand in operands {
        let path = resolve_operand(&ctx.cwd, operand);
        match std::fs::read_link(&path) {
            Ok(target) => {
                out.extend(target.to_string_lossy().as_bytes());
                out.push(b'\n');
                resolved.insert(
                    operand.clone(),
                    json!(target.to_string_lossy().into_owned()),
                );
            }
            Err(error) => failures.push(format!("readlink: {}: {error}", path.display())),
        }
    }
    if failures.is_empty() {
        BuiltinOutput::ok(out, json!({"builtin": "readlink", "resolved": resolved}))
    } else {
        let mut result = BuiltinOutput::failed(
            failures.join("\n"),
            json!({"builtin": "readlink", "resolved": resolved, "failures": failures.len()}),
        );
        result.stdout = out;
        result
    }
}

fn resolve_paths(
    builtin: &str,
    operands: &[String],
    missing_ok: bool,
    ctx: &BuiltinContext,
    resolve: impl Fn(&Path) -> Result<PathBuf, String>,
) -> BuiltinOutput {
    if operands.is_empty() {
        return BuiltinOutput::failed(
            format!("{builtin}: usage: {builtin} path ..."),
            json!({"builtin": builtin, "error": "missing_operand"}),
        );
    }
    let mut out = Vec::new();
    let mut failures = Vec::new();
    let mut resolved = serde_json::Map::new();
    for operand in operands {
        let path = resolve_operand(&ctx.cwd, operand);
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            ctx.cwd.join(&path)
        };
        let outcome = if missing_ok {
            canonicalize_missing_ok(&absolute)
        } else {
            resolve(&absolute)
        };
        match outcome {
            Ok(target) => {
                out.extend(target.to_string_lossy().as_bytes());
                out.push(b'\n');
                resolved.insert(
                    operand.clone(),
                    json!(target.to_string_lossy().into_owned()),
                );
            }
            Err(message) => failures.push(format!("{builtin}: {message}")),
        }
    }
    if failures.is_empty() {
        BuiltinOutput::ok(out, json!({"builtin": builtin, "resolved": resolved}))
    } else {
        let mut result = BuiltinOutput::failed(
            failures.join("\n"),
            json!({"builtin": builtin, "resolved": resolved, "failures": failures.len()}),
        );
        result.stdout = out;
        result
    }
}

/// Lexical absolute normalization for `--missing-ok`: resolves `.`/`..`
/// textually without touching the filesystem.
/// Canonicalizes `path`, tolerating nonexistent trailing components:
/// the longest existing ancestor is canonicalized and the remainder
/// reattached lexically (`.`/`..` resolved textually, no symlink
/// resolution past the existing prefix).
fn canonicalize_missing_ok(path: &Path) -> Result<PathBuf, String> {
    let mut cursor = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if cursor.exists() {
            let canonical = cursor
                .canonicalize()
                .map_err(|error| format!("{}: {error}", cursor.display()))?;
            let mut rebuilt = canonical;
            for name in tail.into_iter().rev() {
                rebuilt.push(name);
            }
            return Ok(normalize_lexical(&rebuilt));
        }
        let parent = cursor.parent().map(|parent| parent.to_path_buf());
        let file_name = cursor.file_name().map(|name| name.to_os_string());
        match (parent, file_name) {
            (Some(parent), Some(name)) if parent != cursor => {
                tail.push(name);
                cursor = parent;
            }
            _ => return Err(format!("{}: cannot resolve", path.display())),
        }
    }
}

/// Resolves `.` and `..` components textually (no symlink resolution).
fn normalize_lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
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

fn resolve_operand(cwd: &Path, operand: &str) -> PathBuf {
    let path = PathBuf::from(operand);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}
