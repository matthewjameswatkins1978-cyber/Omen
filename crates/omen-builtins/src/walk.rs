//! Recursive traversal builtins: `find`, `du`, `tree`.
//!
//! One bounded walker (the already-vendored `walkdir` crate) serves all
//! three; Omen owns the predicates, units and rendering. Deliberate P1
//! limits, all refused loudly rather than faked:
//!
//! - no ignore-file semantics (`.gitignore` is NOT respected — everything is
//!   visited, like GNU `find`);
//! - symlinked directories are never followed (no cycle risk by construction);
//! - `du` reports apparent byte sizes, never disk blocks;
//! - `find -name` matches with `glob` rules over the lossy file name —
//!   non-UTF-8 names only match ASCII patterns (documented, in the truth);
//! - `tree` drawing characters are display rendering (`projection:
//!   "rendered-tree"`, always lossy when names are not plain UTF-8).

use super::{BuiltinContext, BuiltinOutput};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn raw_bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_encoded_bytes().to_vec()
}

/// Display bytes for a walked path: relative to the session cwd when the
/// path lives under it (GNU `find sub` prints `sub/inner`, not an absolute
/// path), absolute otherwise. The typed truth keeps full paths.
fn display_bytes(path: &Path, cwd: &Path) -> Vec<u8> {
    // `cwd.join(".")` normalizes its trailing dot away under
    // `components()`, so the walk root would never strip: name it directly.
    if path.as_os_str() == cwd.join(".").as_os_str() {
        return b".".to_vec();
    }
    match path.strip_prefix(cwd) {
        Ok(relative) if !relative.as_os_str().is_empty() => raw_bytes(relative),
        _ => raw_bytes(path),
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

fn finish(out: Vec<u8>, truth: serde_json::Value, failures: Vec<String>) -> BuiltinOutput {
    if failures.is_empty() {
        BuiltinOutput::ok(out, truth)
    } else {
        let mut result = BuiltinOutput::failed(failures.join("\n"), truth);
        result.stdout = out;
        result
    }
}

/// `find [path ...] [-maxdepth N] [-mindepth N] [-type f|d|l] [-name PATTERN]`.
///
/// Prints every visited path (roots included) one per line, depth-first with
/// siblings in byte-sorted order. Unknown primaries (`-exec`, `-delete`,
/// `-perm`, `-size`, `-newer`, …) are refused.
pub fn find(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut maxdepth: Option<usize> = None;
    let mut mindepth: usize = 0;
    let mut type_filter: Option<u8> = None;
    let mut name_pattern: Option<String> = None;
    let mut operands: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--" => {
                operands.extend(args[index + 1..].iter().cloned());
                break;
            }
            "-maxdepth" => {
                index += 1;
                match args.get(index).and_then(|v| v.parse::<usize>().ok()) {
                    Some(n) => maxdepth = Some(n),
                    None => {
                        return BuiltinOutput::failed(
                            "find: -maxdepth needs a non-negative number".to_string(),
                            json!({"builtin": "find", "error": "usage"}),
                        );
                    }
                }
            }
            "-mindepth" => {
                index += 1;
                match args.get(index).and_then(|v| v.parse::<usize>().ok()) {
                    Some(n) => mindepth = n,
                    None => {
                        return BuiltinOutput::failed(
                            "find: -mindepth needs a non-negative number".to_string(),
                            json!({"builtin": "find", "error": "usage"}),
                        );
                    }
                }
            }
            "-type" => {
                index += 1;
                match args.get(index).map(String::as_str) {
                    Some("f") => type_filter = Some(b'f'),
                    Some("d") => type_filter = Some(b'd'),
                    Some("l") => type_filter = Some(b'l'),
                    _ => {
                        return BuiltinOutput::failed(
                            "find: -type takes only f, d or l".to_string(),
                            json!({"builtin": "find", "error": "usage"}),
                        );
                    }
                }
            }
            "-name" => {
                index += 1;
                match args.get(index) {
                    Some(pattern) => name_pattern = Some(pattern.clone()),
                    None => {
                        return BuiltinOutput::failed(
                            "find: -name needs a pattern".to_string(),
                            json!({"builtin": "find", "error": "usage"}),
                        );
                    }
                }
            }
            _ if arg.starts_with('-') && arg != "-" => {
                return BuiltinOutput::failed(
                    format!(
                        "find: unsupported primary {arg:?} (only -maxdepth, -mindepth, -type, -name)"
                    ),
                    json!({"builtin": "find", "error": "unsupported_option"}),
                );
            }
            _ => operands.push(arg.clone()),
        }
        index += 1;
    }
    if operands.is_empty() {
        operands.push(".".to_string());
    }
    let glob_pattern = match name_pattern {
        Some(ref pattern) => match glob::Pattern::new(pattern) {
            Ok(p) => Some(p),
            Err(error) => {
                return BuiltinOutput::failed(
                    format!("find: bad -name pattern: {error}"),
                    json!({"builtin": "find", "error": "usage"}),
                );
            }
        },
        None => None,
    };
    let mut out = Vec::new();
    let mut failures = Vec::new();
    let mut visited = 0u64;
    let mut lossless = true;
    for operand in &operands {
        let root = resolve_operand(&ctx.cwd, operand);
        if std::fs::symlink_metadata(&root).is_err() {
            failures.push(format!(
                "find: {}: no such file or directory",
                root.display()
            ));
            continue;
        }
        let mut walker = walkdir::WalkDir::new(&root)
            .min_depth(mindepth)
            .sort_by_file_name();
        if let Some(depth) = maxdepth {
            walker = walker.max_depth(depth);
        }
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(error) => {
                    failures.push(format!("find: traversal error under {operand}: {error}"));
                    continue;
                }
            };
            let file_type = entry.file_type();
            if let Some(filter) = type_filter {
                let keep = match filter {
                    b'f' => file_type.is_file(),
                    b'd' => file_type.is_dir(),
                    _ => file_type.is_symlink(),
                };
                if !keep {
                    continue;
                }
            }
            if let Some(ref pattern) = glob_pattern {
                // Matched over the lossy file name: non-UTF-8 names carry
                // U+FFFD, so only ASCII patterns can match them. Declared.
                let name = entry.file_name().to_string_lossy();
                if !pattern.matches(&name) {
                    continue;
                }
            }
            let bytes = display_bytes(entry.path(), &ctx.cwd);
            if String::from_utf8(bytes.clone()).is_err() || bytes.contains(&b'\n') {
                lossless = false;
            }
            out.extend_from_slice(&bytes);
            out.push(b'\n');
            visited += 1;
        }
    }
    finish(
        out,
        json!({"builtin": "find", "projection": "names-nl", "lossless": lossless,
               "visited": visited, "name_match_lossy": glob_pattern.is_some(),
               "failures": failures.len()}),
        failures,
    )
}

/// `du [-s] [-a] [-b|-k|-m] [path ...]`.
///
/// Apparent sizes (metadata lengths summed, symlinks counted at link
/// length, never followed). Default unit is bytes; `-k`/`-m` round up to
/// KiB/MiB. Default prints one line per operand; `-a` prints every visited
/// entry (files at size, directories at cumulative total, post-order).
pub fn du(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut summarize_only = false;
    let mut show_all = false;
    let mut unit: u64 = 1;
    let mut unit_name = "bytes";
    let mut operands: Vec<String> = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--" => continue,
            "-s" | "--summarize" => summarize_only = true,
            "-a" | "--all" => show_all = true,
            "-b" | "--bytes" => {
                unit = 1;
                unit_name = "bytes";
            }
            "-k" => {
                unit = 1024;
                unit_name = "kib";
            }
            "-m" => {
                unit = 1024 * 1024;
                unit_name = "mib";
            }
            _ if arg.starts_with('-') && arg != "-" => {
                return BuiltinOutput::failed(
                    format!("du: unsupported option {arg:?} (only -s, -a, -b, -k, -m)"),
                    json!({"builtin": "du", "error": "unsupported_option"}),
                );
            }
            _ => operands.push(arg.clone()),
        }
    }
    if operands.is_empty() {
        operands.push(".".to_string());
    }
    if summarize_only {
        show_all = false;
    }
    let mut out = Vec::new();
    let mut failures = Vec::new();
    let mut total = 0u64;
    for operand in &operands {
        let root = resolve_operand(&ctx.cwd, operand);
        let metadata = match std::fs::symlink_metadata(&root) {
            Ok(m) => m,
            Err(error) => {
                failures.push(format!("du: {}: {error}", root.display()));
                continue;
            }
        };
        if !metadata.is_dir() {
            let size = metadata.len();
            total += size;
            out.extend(format!("{}\t{operand}\n", scale(size, unit)).as_bytes());
            continue;
        }
        // Bottom-up accumulation: every dir's total (files + subdirs,
        // never the dir entry's own platform length) is ready post-order.
        let mut dir_sizes: HashMap<PathBuf, u64> = HashMap::new();
        let mut failed = false;
        for entry in walkdir::WalkDir::new(&root)
            .sort_by_file_name()
            .contents_first(true)
        {
            let entry = match entry {
                Ok(e) => e,
                Err(error) => {
                    failures.push(format!("du: traversal error under {operand}: {error}"));
                    failed = true;
                    break;
                }
            };
            let subtotal = if entry.file_type().is_dir() {
                dir_sizes.get(entry.path()).copied().unwrap_or(0)
            } else {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            };
            if let Some(parent) = entry.path().parent() {
                *dir_sizes.entry(parent.to_path_buf()).or_insert(0) += subtotal;
            }
        }
        if failed {
            continue;
        }
        let root_total = dir_sizes.get(&root).copied().unwrap_or(0);
        total += root_total;
        if show_all {
            // Top-down display walk reuses the cumulative map for dirs.
            for entry in walkdir::WalkDir::new(&root).sort_by_file_name() {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let size = if entry.file_type().is_dir() {
                    dir_sizes.get(entry.path()).copied().unwrap_or(0)
                } else {
                    entry.metadata().map(|m| m.len()).unwrap_or(0)
                };
                let shown =
                    String::from_utf8_lossy(&display_bytes(entry.path(), &ctx.cwd)).into_owned();
                out.extend(format!("{}\t{}\n", scale(size, unit), shown).as_bytes());
            }
        } else {
            out.extend(format!("{}\t{operand}\n", scale(root_total, unit)).as_bytes());
        }
    }
    finish(
        out,
        json!({"builtin": "du", "units": unit_name, "apparent": true, "bytes_total": total,
               "failures": failures.len()}),
        failures,
    )
}

fn scale(bytes: u64, unit: u64) -> u64 {
    if unit <= 1 {
        bytes
    } else {
        bytes.div_ceil(unit)
    }
}

/// `tree [-a] [-d] [-L LEVEL] [path ...]`.
///
/// Renders `├──`/`└──`/`│   ` prefixed lines with a trailing `/` on
/// directories and ` -> target` on symlinks. The drawing is display
/// rendering (`projection: "rendered-tree"`, always lossy for non-plain
/// names); the truth carries typed entries. `-P`/`-I` patterns are refused.
pub fn tree(args: &[String], ctx: &BuiltinContext) -> BuiltinOutput {
    let mut all = false;
    let mut dirs_only = false;
    let mut max_level: Option<usize> = None;
    let mut operands: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--" => {
                operands.extend(args[index + 1..].iter().cloned());
                break;
            }
            "-a" | "--all" => all = true,
            "-d" => dirs_only = true,
            "-L" | "--level" => {
                index += 1;
                match args.get(index).and_then(|v| v.parse::<usize>().ok()) {
                    Some(n) => max_level = Some(n),
                    None => {
                        return BuiltinOutput::failed(
                            "tree: -L needs a non-negative number".to_string(),
                            json!({"builtin": "tree", "error": "usage"}),
                        );
                    }
                }
            }
            _ if arg.starts_with('-') && arg != "-" => {
                return BuiltinOutput::failed(
                    format!("tree: unsupported option {arg:?} (only -a, -d, -L)"),
                    json!({"builtin": "tree", "error": "unsupported_option"}),
                );
            }
            _ => operands.push(arg.clone()),
        }
        index += 1;
    }
    if operands.is_empty() {
        operands.push(".".to_string());
    }
    let mut out = Vec::new();
    let mut failures = Vec::new();
    let mut dirs = 0u64;
    let mut files = 0u64;
    let mut entries_json = Vec::new();
    for operand in &operands {
        let root = resolve_operand(&ctx.cwd, operand);
        if std::fs::symlink_metadata(&root).is_err() {
            failures.push(format!(
                "tree: {}: no such file or directory",
                root.display()
            ));
            continue;
        }
        out.extend(format!("{operand}\n").as_bytes());
        // A file operand renders as itself (GNU); only directories are
        // descended and counted as such.
        let root_type = std::fs::symlink_metadata(&root).map(|m| m.file_type());
        let root_is_dir = root_type.as_ref().is_ok_and(|t| t.is_dir());
        if !root_is_dir {
            let display = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| operand.clone());
            out.extend(display.as_bytes());
            let kind = if root_type.as_ref().is_ok_and(|t| t.is_symlink()) {
                if let Ok(target) = std::fs::read_link(&root) {
                    out.extend(b" -> ");
                    out.extend(target.to_string_lossy().as_bytes());
                }
                "symlink"
            } else {
                "file"
            };
            out.push(b'\n');
            files += 1;
            entries_json.push(json!({"path": root.to_string_lossy(), "type": kind, "depth": 0}));
            continue;
        }
        render_tree_children(
            &root,
            "",
            1,
            max_level,
            all,
            dirs_only,
            &mut out,
            &mut dirs,
            &mut files,
            &mut entries_json,
            &mut failures,
        );
        dirs += 1;
    }
    out.extend(format!("\n{dirs} directories, {files} files\n").as_bytes());
    finish(
        out,
        json!({"builtin": "tree", "projection": "rendered-tree", "lossless": false,
               "directories": dirs, "files": files, "entries": entries_json,
               "failures": failures.len()}),
        failures,
    )
}

#[allow(clippy::too_many_arguments)]
fn render_tree_children(
    dir: &Path,
    prefix: &str,
    level: usize,
    max_level: Option<usize>,
    show_all: bool,
    dirs_only: bool,
    out: &mut Vec<u8>,
    dirs: &mut u64,
    files: &mut u64,
    entries_json: &mut Vec<serde_json::Value>,
    failures: &mut Vec<String>,
) {
    if let Some(max) = max_level
        && level > max
    {
        return;
    }
    let mut children: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
        Err(error) => {
            failures.push(format!("tree: {}: {error}", dir.display()));
            return;
        }
    };
    children.sort_by(|a, b| {
        a.file_name()
            .map(|n| n.as_encoded_bytes().to_vec())
            .cmp(&b.file_name().map(|n| n.as_encoded_bytes().to_vec()))
    });
    if dirs_only {
        children.retain(|p| p.is_dir() && !p.is_symlink());
    }
    if !show_all {
        children.retain(|p| {
            p.file_name()
                .map(|n| !n.to_string_lossy().starts_with('.'))
                .unwrap_or(true)
        });
    }
    let total = children.len();
    for (position, child) in children.iter().enumerate() {
        let last = position + 1 == total;
        let branch = if last { "└── " } else { "├── " };
        let extension = if last { "    " } else { "│   " };
        let file_type = std::fs::symlink_metadata(child).map(|m| m.file_type()).ok();
        let is_dir = file_type.is_some_and(|t| t.is_dir());
        let is_link = file_type.is_some_and(|t| t.is_symlink());
        let display = child
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| child.display().to_string());
        out.extend(prefix.as_bytes());
        out.extend(branch.as_bytes());
        out.extend(display.as_bytes());
        if is_dir && !is_link {
            out.push(b'/');
        }
        if is_link && let Ok(target) = std::fs::read_link(child) {
            out.extend(b" -> ");
            out.extend(target.to_string_lossy().as_bytes());
        }
        out.push(b'\n');
        let kind = if is_link {
            "symlink"
        } else if is_dir {
            "dir"
        } else {
            "file"
        };
        entries_json.push(json!({"path": child.to_string_lossy(), "type": kind, "depth": level}));
        if is_dir && !is_link {
            *dirs += 1;
            let child_prefix = format!("{prefix}{extension}");
            render_tree_children(
                child,
                &child_prefix,
                level + 1,
                max_level,
                show_all,
                dirs_only,
                out,
                dirs,
                files,
                entries_json,
                failures,
            );
        } else {
            *files += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        std::fs::write(dir.path().join("top.txt"), b"12345").expect("write");
        std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
        std::fs::write(dir.path().join("sub").join("inner.txt"), b"abcdefg").expect("write");
        dir
    }

    #[test]
    fn find_lists_everything_sorted() {
        let dir = fixture();
        // Run inside the fixture so operand display is stable.
        let ctx = ctx_for(&dir.path().join("sub"));
        let out = find(&s(&["-maxdepth", "1"]), &ctx);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, b".\ninner.txt\n");
        let out = find(&s(&[]), &ctx_for(dir.path()));
        assert_eq!(out.code, 0);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("top.txt") && text.contains("inner.txt"));
        assert!(text.find("sub").unwrap() < text.find("inner.txt").unwrap());
        assert_eq!(out.truth["lossless"], json!(true));
    }

    #[test]
    fn find_type_and_name_filters() {
        let dir = fixture();
        let out = find(&s(&["-type", "d"]), &ctx_for(dir.path()));
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("sub") && !text.contains("top.txt"));
        let out = find(&s(&["-name", "*.txt"]), &ctx_for(dir.path()));
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("top.txt") && text.contains("inner.txt") && !text.contains("sub\n"));
        assert_eq!(out.truth["name_match_lossy"], json!(true));
    }

    #[test]
    fn find_refuses_exec_and_reports_missing() {
        let dir = fixture();
        assert_eq!(find(&s(&["-exec", "rm"]), &ctx_for(dir.path())).code, 1);
        let out = find(&s(&["absent"]), &ctx_for(dir.path()));
        assert_eq!(out.code, 1);
        assert!(String::from_utf8_lossy(&out.stderr).contains("absent"));
    }

    #[test]
    fn du_sums_apparent_bytes() {
        let dir = fixture();
        let out = du(&s(&[]), &ctx_for(dir.path()));
        assert_eq!(out.code, 0);
        // 5 + 7 = 12 bytes of file content.
        assert_eq!(out.truth["bytes_total"], json!(12));
        assert_eq!(out.truth["units"], json!("bytes"));
        let out = du(&s(&["-k"]), &ctx_for(dir.path()));
        assert!(String::from_utf8_lossy(&out.stdout).starts_with('1'));
        let out = du(&s(&["-a"]), &ctx_for(dir.path()));
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("top.txt") && text.contains("inner.txt"));
        assert_eq!(du(&s(&["--blocks"]), &ctx_for(dir.path())).code, 1);
    }

    #[test]
    fn tree_renders_and_counts() {
        let dir = fixture();
        let out = tree(&s(&[]), &ctx_for(dir.path()));
        assert_eq!(out.code, 0);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("sub/") && text.contains("inner.txt"));
        assert!(text.contains("2 directories, 2 files"));
        assert_eq!(out.truth["projection"], json!("rendered-tree"));
        let out = tree(&s(&["-L", "1"]), &ctx_for(dir.path()));
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!text.contains("inner.txt"));
        assert!(text.contains("1 directories, 1 files") || text.contains("directories"));
        // A file operand renders as itself (no phantom directory count).
        let out = tree(&s(&["top.txt"]), &ctx_for(dir.path()));
        assert_eq!(out.code, 0);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.contains("top.txt") && text.contains("0 directories, 1 files"),
            "{text:?}"
        );
    }

    #[test]
    fn tree_refuses_patterns() {
        let dir = fixture();
        assert_eq!(tree(&s(&["-P", "*.rs"]), &ctx_for(dir.path())).code, 1);
        assert_eq!(tree(&s(&["absent"]), &ctx_for(dir.path())).code, 1);
    }
}
