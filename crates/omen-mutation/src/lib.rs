//! Omen-owned consequential filesystem mutation: planning, estimation,
//! admission seam, physical execution, observation, evidence.
//!
//! Pipeline (packet phase F1):
//!
//! ```text
//! parse → resolve → estimate → preflight → ADMISSION → bind → execute
//! → observe → invalidate Facts → evidence
//! ```
//!
//! Authority truth: there is currently NO admitted Tethers host-execution
//! filesystem capability (the existing `file.move`/`file.metadata`
//! manifests bind MCP reference providers under narrow test prefixes —
//! repurposing them for shell mutations would be a wrong-executor,
//! wrong-scope violation). Therefore the shell path always resolves to
//! [`Admission::RefusedClosed`], which performs ZERO filesystem effects by
//! construction: [`execute`] checks admission before touching anything.
//!
//! [`Admission::Admitted`] exists for the future `commit_bundle` hookup and
//! for physical-logic tests, which construct it explicitly and say so.
//! Authority acceptance itself lives in `omen-authority`, never here.

use serde_json::json;
use std::path::{Path, PathBuf};

/// Bounded traversal cap for effect estimation. Past the cap the estimate
/// stops enumerating and reports `truncated: true` with High severity —
/// never a silent undercount.
pub const ESTIMATE_ENTRY_CAP: usize = 10_000;

/// One consequential filesystem operation parsed from shell argv.
#[derive(Debug, Clone, PartialEq)]
pub enum MutationOp {
    Copy {
        sources: Vec<String>,
        dest: String,
        recursive: bool,
        no_clobber: bool,
    },
    Move {
        sources: Vec<String>,
        dest: String,
        no_clobber: bool,
    },
    Remove {
        targets: Vec<String>,
        recursive: bool,
        force: bool,
    },
    MakeDir {
        dirs: Vec<String>,
        parents: bool,
    },
    RemoveDir {
        dirs: Vec<String>,
    },
    Touch {
        files: Vec<String>,
        no_create: bool,
    },
    Link {
        sources: Vec<String>,
        dest: String,
        symbolic: bool,
    },
}

/// Parse failures: usage errors, never filesystem access.
#[derive(Debug, thiserror::Error)]
pub enum MutationError {
    #[error("usage: {0}")]
    Usage(String),
    #[error("refused: {0}")]
    Refused(String),
    #[error("execute failed: {0}")]
    Execute(String),
}

/// Parses shell argv into an operation. Supported subsets are deliberately
/// small; anything else is a usage error, not a reinterpretation.
pub fn parse(argv: &[String]) -> Result<MutationOp, MutationError> {
    let (name, args) = argv
        .split_first()
        .ok_or_else(|| MutationError::Usage("empty argv".to_string()))?;
    let usage = |text: &str| MutationError::Usage(text.to_string());
    match name.as_str() {
        "cp" => {
            let mut recursive = false;
            let mut no_clobber = false;
            let mut operands: Vec<String> = Vec::new();
            for arg in args {
                match arg.as_str() {
                    "-r" | "-R" | "--recursive" => recursive = true,
                    "-n" | "--no-clobber" => no_clobber = true,
                    "--" => continue,
                    _ if arg.starts_with('-') && arg != "-" => {
                        return Err(usage(
                            "cp: only -r, -n (usage: cp [-r] [-n] SOURCE... DEST)",
                        ));
                    }
                    _ => operands.push(arg.clone()),
                }
            }
            if operands.len() < 2 {
                return Err(usage("cp: usage: cp [-r] [-n] SOURCE... DEST"));
            }
            let dest = operands.pop().expect("len >= 2");
            Ok(MutationOp::Copy {
                sources: operands,
                dest,
                recursive,
                no_clobber,
            })
        }
        "mv" => {
            let mut no_clobber = false;
            let mut operands: Vec<String> = Vec::new();
            for arg in args {
                match arg.as_str() {
                    "-n" | "--no-clobber" => no_clobber = true,
                    "--" => continue,
                    _ if arg.starts_with('-') && arg != "-" => {
                        return Err(usage("mv: only -n (usage: mv [-n] SOURCE... DEST)"));
                    }
                    _ => operands.push(arg.clone()),
                }
            }
            if operands.len() < 2 {
                return Err(usage("mv: usage: mv [-n] SOURCE... DEST"));
            }
            let dest = operands.pop().expect("len >= 2");
            Ok(MutationOp::Move {
                sources: operands,
                dest,
                no_clobber,
            })
        }
        "rm" => {
            let mut recursive = false;
            let mut force = false;
            let mut targets: Vec<String> = Vec::new();
            for arg in args {
                match arg.as_str() {
                    "-r" | "-R" | "--recursive" => recursive = true,
                    "-f" | "--force" => force = true,
                    "--" => continue,
                    _ if arg.starts_with('-') && arg != "-" => {
                        // Combined short flags (`-rf`, `-fr`).
                        if arg[1..].chars().all(|c| c == 'r' || c == 'R' || c == 'f') {
                            recursive |= arg.contains('r') || arg.contains('R');
                            force |= arg.contains('f');
                        } else {
                            return Err(usage("rm: only -r, -f (usage: rm [-r] [-f] TARGET...)"));
                        }
                    }
                    _ => targets.push(arg.clone()),
                }
            }
            if targets.is_empty() {
                return Err(usage("rm: usage: rm [-r] [-f] TARGET..."));
            }
            Ok(MutationOp::Remove {
                targets,
                recursive,
                force,
            })
        }
        "mkdir" => {
            let mut parents = false;
            let mut dirs: Vec<String> = Vec::new();
            for arg in args {
                match arg.as_str() {
                    "-p" | "--parents" => parents = true,
                    "--" => continue,
                    _ if arg.starts_with('-') && arg != "-" => {
                        return Err(usage("mkdir: only -p (usage: mkdir [-p] DIR...)"));
                    }
                    _ => dirs.push(arg.clone()),
                }
            }
            if dirs.is_empty() {
                return Err(usage("mkdir: usage: mkdir [-p] DIR..."));
            }
            Ok(MutationOp::MakeDir { dirs, parents })
        }
        "rmdir" => {
            let mut dirs: Vec<String> = Vec::new();
            for arg in args {
                if arg == "--" {
                    continue;
                }
                if arg.starts_with('-') && arg != "-" {
                    return Err(usage("rmdir: takes no options (usage: rmdir DIR...)"));
                }
                dirs.push(arg.clone());
            }
            if dirs.is_empty() {
                return Err(usage("rmdir: usage: rmdir DIR..."));
            }
            Ok(MutationOp::RemoveDir { dirs })
        }
        "touch" => {
            let mut no_create = false;
            let mut files: Vec<String> = Vec::new();
            for arg in args {
                match arg.as_str() {
                    "-c" | "--no-create" => no_create = true,
                    "--" => continue,
                    _ if arg.starts_with('-') && arg != "-" => {
                        return Err(usage("touch: only -c (usage: touch [-c] FILE...)"));
                    }
                    _ => files.push(arg.clone()),
                }
            }
            if files.is_empty() {
                return Err(usage("touch: usage: touch [-c] FILE..."));
            }
            Ok(MutationOp::Touch { files, no_create })
        }
        "ln" => {
            let mut symbolic = false;
            let mut operands: Vec<String> = Vec::new();
            for arg in args {
                match arg.as_str() {
                    "-s" | "--symbolic" => symbolic = true,
                    "--" => continue,
                    _ if arg.starts_with('-') && arg != "-" => {
                        return Err(usage("ln: only -s (usage: ln [-s] SOURCE... DEST)"));
                    }
                    _ => operands.push(arg.clone()),
                }
            }
            if operands.len() < 2 {
                return Err(usage("ln: usage: ln [-s] SOURCE... DEST"));
            }
            let dest = operands.pop().expect("len >= 2");
            Ok(MutationOp::Link {
                sources: operands,
                dest,
                symbolic,
            })
        }
        other => Err(usage(&format!(
            "not a mutation command: {other:?} (cp mv rm mkdir rmdir touch ln)"
        ))),
    }
}

/// Absolute-path resolution of every operand against the session cwd.
#[derive(Debug, Clone)]
pub struct ResolvedOp {
    pub op: MutationOp,
    pub cwd: PathBuf,
}

pub fn resolve(op: MutationOp, cwd: &Path) -> ResolvedOp {
    ResolvedOp {
        op,
        cwd: cwd.to_path_buf(),
    }
}

fn absolutize(cwd: &Path, operand: &str) -> PathBuf {
    let path = PathBuf::from(operand);
    let joined = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    normalize_lexical(&joined)
}

/// Resolves `.`/`..` textually (no symlink resolution, no filesystem
/// access) so prefix checks (`starts_with(cwd)`) are truthful for
/// `../sibling` operands. Preserves Windows prefixes and root.
fn normalize_lexical(path: &Path) -> PathBuf {
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

/// One affected path found during estimation.
#[derive(Debug, Clone)]
pub struct AffectedPath {
    pub path: PathBuf,
    /// What is there now (symlinks reported as links, never followed).
    pub kind: &'static str,
    pub bytes: u64,
}

/// The estimated blast radius, computed before any admission decision.
#[derive(Debug, Clone)]
pub struct EffectEstimate {
    pub affected: Vec<AffectedPath>,
    /// Enumeration stopped at the cap: counts are lower bounds.
    pub truncated: bool,
    /// Destination paths that already exist (overwrite candidates).
    pub overwrites: Vec<PathBuf>,
    /// Affected paths outside the session cwd.
    pub outside_cwd: Vec<PathBuf>,
    /// Symlinked directories met during traversal (never followed).
    pub symlinked_dirs: Vec<PathBuf>,
    pub total_bytes: u64,
}

fn kind_of(path: &Path) -> (&'static str, u64) {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            let kind = if file_type.is_symlink() {
                "link"
            } else if file_type.is_dir() {
                "dir"
            } else if file_type.is_file() {
                "file"
            } else {
                "other"
            };
            (kind, metadata.len())
        }
        Err(_) => ("absent", 0),
    }
}

/// Enumerates the estimated effects without mutating anything. Pure
/// observation: every filesystem call here is a read (metadata, read_dir).
pub fn estimate(resolved: &ResolvedOp) -> EffectEstimate {
    let mut estimate = EffectEstimate {
        affected: Vec::new(),
        truncated: false,
        overwrites: Vec::new(),
        outside_cwd: Vec::new(),
        symlinked_dirs: Vec::new(),
        total_bytes: 0,
    };
    let mut push = |estimate: &mut EffectEstimate, path: PathBuf| {
        if estimate.affected.len() >= ESTIMATE_ENTRY_CAP {
            estimate.truncated = true;
            return;
        }
        if !path.starts_with(&resolved.cwd) {
            estimate.outside_cwd.push(path.clone());
        }
        let (kind, bytes) = kind_of(&path);
        if kind == "file" || kind == "link" {
            estimate.total_bytes = estimate.total_bytes.saturating_add(bytes);
        }
        estimate.affected.push(AffectedPath { path, kind, bytes });
    };
    let note_overwrite = |estimate: &mut EffectEstimate, path: PathBuf| {
        if kind_of(&path).0 != "absent" && !estimate.overwrites.contains(&path) {
            estimate.overwrites.push(path);
        }
    };
    match &resolved.op {
        MutationOp::Copy {
            sources,
            dest,
            recursive,
            ..
        } => {
            let dest_path = absolutize(&resolved.cwd, dest);
            let dest_is_dir = dest_path.is_dir() && !dest_path.is_symlink();
            for source in sources {
                let src = absolutize(&resolved.cwd, source);
                walk_operand(&src, *recursive, &mut estimate, &mut push);
                if sources.len() > 1 || dest_is_dir {
                    let _ = dest_is_dir;
                } else {
                    note_overwrite(&mut estimate, dest_path.clone());
                }
            }
            if dest_is_dir || sources.len() > 1 {
                for source in sources {
                    let src = absolutize(&resolved.cwd, source);
                    let name = src.file_name().map(PathBuf::from).unwrap_or_default();
                    note_overwrite(&mut estimate, dest_path.join(name));
                }
            }
            push(&mut estimate, dest_path);
        }
        MutationOp::Move { sources, dest, .. } => {
            let dest_path = absolutize(&resolved.cwd, dest);
            let dest_is_dir = dest_path.is_dir() && !dest_path.is_symlink();
            for source in sources {
                walk_operand(
                    &absolutize(&resolved.cwd, source),
                    true,
                    &mut estimate,
                    &mut push,
                );
            }
            if !(dest_is_dir || sources.len() > 1) {
                note_overwrite(&mut estimate, dest_path.clone());
            }
            push(&mut estimate, dest_path);
        }
        MutationOp::Remove {
            targets, recursive, ..
        } => {
            for target in targets {
                walk_operand(
                    &absolutize(&resolved.cwd, target),
                    *recursive,
                    &mut estimate,
                    &mut push,
                );
            }
        }
        MutationOp::MakeDir { dirs, .. } | MutationOp::RemoveDir { dirs } => {
            for dir in dirs {
                push(&mut estimate, absolutize(&resolved.cwd, dir));
            }
        }
        MutationOp::Touch { files, .. } => {
            for file in files {
                push(&mut estimate, absolutize(&resolved.cwd, file));
            }
        }
        MutationOp::Link { sources, dest, .. } => {
            for source in sources {
                push(&mut estimate, absolutize(&resolved.cwd, source));
            }
            note_overwrite(&mut estimate, absolutize(&resolved.cwd, dest));
            push(&mut estimate, absolutize(&resolved.cwd, dest));
        }
    }
    estimate
}

/// Records one operand and, for directories when `recursive`, its whole
/// subtree (symlinked dirs recorded, never descended).
fn walk_operand(
    path: &Path,
    recursive: bool,
    estimate: &mut EffectEstimate,
    push: &mut impl FnMut(&mut EffectEstimate, PathBuf),
) {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => {
            push(estimate, path.to_path_buf());
            return;
        }
    };
    if metadata.file_type().is_symlink() {
        push(estimate, path.to_path_buf());
        // A link to a directory is recorded (never descended): one
        // metadata call classifies the target without traversing it.
        if std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false) {
            estimate.symlinked_dirs.push(path.to_path_buf());
        }
        return;
    }
    if !metadata.is_dir() {
        push(estimate, path.to_path_buf());
        return;
    }
    if !recursive {
        push(estimate, path.to_path_buf());
        return;
    }
    push(estimate, path.to_path_buf());
    for entry in walkdir::WalkDir::new(path).sort_by_file_name().min_depth(1) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                estimate.truncated = true;
                break;
            }
        };
        if entry.file_type().is_symlink() {
            // Never followed (walkdir is not in follow mode): dir links are
            // recorded so the estimate names every link the operation will
            // reproduce or refuse rather than traverse.
            if entry.path().is_dir() {
                estimate.symlinked_dirs.push(entry.path().to_path_buf());
            }
        }
        if estimate.affected.len() >= ESTIMATE_ENTRY_CAP {
            estimate.truncated = true;
            break;
        }
        push(estimate, entry.path().to_path_buf());
    }
}

/// Preflight severity over an estimate. Advisory only: admission decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Low,
    Medium,
    High,
}

pub fn preflight(estimate: &EffectEstimate, op: &MutationOp) -> (Severity, String) {
    let mut reasons: Vec<String> = Vec::new();
    if estimate.truncated {
        reasons.push("estimate truncated past 10,000 entries".to_string());
    }
    if !estimate.outside_cwd.is_empty() {
        reasons.push(format!(
            "{} path(s) outside the session directory",
            estimate.outside_cwd.len()
        ));
    }
    if !estimate.overwrites.is_empty() {
        reasons.push(format!(
            "{} existing destination(s) would be overwritten",
            estimate.overwrites.len()
        ));
    }
    if matches!(
        op,
        MutationOp::Remove {
            recursive: true,
            ..
        }
    ) {
        reasons.push("recursive removal".to_string());
    }
    if matches!(op, MutationOp::Remove { force: true, .. }) {
        reasons.push("force suppresses missing-operand errors".to_string());
    }
    let severity = if !estimate.outside_cwd.is_empty()
        || estimate.truncated
        || matches!(
            op,
            MutationOp::Remove {
                recursive: true,
                ..
            }
        ) {
        Severity::High
    } else if !estimate.overwrites.is_empty() || matches!(op, MutationOp::Remove { .. }) {
        Severity::Medium
    } else {
        Severity::Low
    };
    let summary = if reasons.is_empty() {
        format!(
            "{} path(s), {} byte(s)",
            estimate.affected.len(),
            estimate.total_bytes
        )
    } else {
        format!(
            "{}: {} path(s), {} byte(s)",
            reasons.join("; "),
            estimate.affected.len(),
            estimate.total_bytes
        )
    };
    (severity, summary)
}

/// The authority gate every mutation must cross. The shell path always
/// resolves to [`Admission::RefusedClosed`] until Tethers ships an
/// admitted host-execution filesystem capability.
#[derive(Debug, Clone)]
pub enum Admission {
    /// No admitted capability: [`execute`] performs zero filesystem
    /// effects and reports the reason.
    RefusedClosed { reason: String },
    /// A verified bundle identity authorising exactly this operation.
    /// Constructed by the future `commit_bundle` hookup — or explicitly
    /// by physical-logic tests, which document the bypass.
    Admitted { bundle: String },
}

impl Admission {
    /// The shell default: refused until a host filesystem capability
    /// is admitted through Tethers authority.
    pub fn shell_default() -> Self {
        Admission::RefusedClosed {
            reason: "no admitted host filesystem capability: shell mutations need a Tethers-owner host-execution decision (see file.move/file.metadata: MCP-bound test prefixes, wrong executor)".to_string(),
        }
    }
}

/// Post-execution observation for one target: what the world looks like now.
#[derive(Debug, Clone)]
pub struct ObservedTarget {
    pub path: PathBuf,
    pub outcome: String,
    pub exists_after: bool,
    pub bytes_after: Option<u64>,
}

/// The evidence a mutation leaves: operation, authority, per-target
/// outcomes, post-observation, and the Fact paths the session must
/// invalidate or update.
#[derive(Debug, Clone)]
pub struct MutationReport {
    pub op: String,
    pub bundle: Option<String>,
    pub targets: Vec<ObservedTarget>,
    pub facts_to_invalidate: Vec<String>,
    pub truth: serde_json::Value,
}

fn observe(path: &Path, outcome: &str) -> ObservedTarget {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => ObservedTarget {
            path: path.to_path_buf(),
            outcome: outcome.to_string(),
            exists_after: true,
            bytes_after: Some(metadata.len()),
        },
        Err(_) => ObservedTarget {
            path: path.to_path_buf(),
            outcome: outcome.to_string(),
            exists_after: false,
            bytes_after: None,
        },
    }
}

/// Executes a resolved operation under admission. `RefusedClosed`
/// short-circuits BEFORE any filesystem write: zero effects, truthful
/// error. `Admitted` performs the operation, observes every target, and
/// returns the Fact-invalidation list (the session owns the registry).
pub fn execute(
    resolved: &ResolvedOp,
    admission: &Admission,
) -> Result<MutationReport, MutationError> {
    let bundle = match admission {
        Admission::RefusedClosed { reason } => return Err(MutationError::Refused(reason.clone())),
        Admission::Admitted { bundle } => Some(bundle.clone()),
    };
    let op_name = match &resolved.op {
        MutationOp::Copy { .. } => "cp",
        MutationOp::Move { .. } => "mv",
        MutationOp::Remove { .. } => "rm",
        MutationOp::MakeDir { .. } => "mkdir",
        MutationOp::RemoveDir { .. } => "rmdir",
        MutationOp::Touch { .. } => "touch",
        MutationOp::Link { .. } => "ln",
    };
    let mut targets: Vec<ObservedTarget> = Vec::new();
    let mut facts: Vec<String> = vec![resolved.cwd.to_string_lossy().into_owned()];
    let fail = |message: String| MutationError::Execute(message);
    match &resolved.op {
        MutationOp::Copy {
            sources,
            dest,
            recursive,
            no_clobber,
        } => {
            let dest_path = absolutize(&resolved.cwd, dest);
            let dest_is_dir = dest_path.is_dir() && !dest_path.is_symlink();
            if sources.len() > 1 && !dest_is_dir {
                return Err(fail(
                    "cp: multiple sources need an existing directory DEST".to_string(),
                ));
            }
            for source in sources {
                let src = absolutize(&resolved.cwd, source);
                let target = if dest_is_dir || sources.len() > 1 {
                    let name = src
                        .file_name()
                        .ok_or_else(|| fail(format!("cp: bad source name {}", src.display())))?;
                    dest_path.join(name)
                } else {
                    dest_path.clone()
                };
                copy_one(&src, &target, *recursive, *no_clobber).map_err(fail)?;
                facts.push(target.to_string_lossy().into_owned());
                targets.push(observe(&target, "copied"));
            }
        }
        MutationOp::Move {
            sources,
            dest,
            no_clobber,
        } => {
            let dest_path = absolutize(&resolved.cwd, dest);
            let dest_is_dir = dest_path.is_dir() && !dest_path.is_symlink();
            if sources.len() > 1 && !dest_is_dir {
                return Err(fail(
                    "mv: multiple sources need an existing directory DEST".to_string(),
                ));
            }
            for source in sources {
                let src = absolutize(&resolved.cwd, source);
                let target = if dest_is_dir || sources.len() > 1 {
                    let name = src
                        .file_name()
                        .ok_or_else(|| fail(format!("mv: bad source name {}", src.display())))?;
                    dest_path.join(name)
                } else {
                    dest_path.clone()
                };
                if *no_clobber && kind_of(&target).0 != "absent" {
                    return Err(fail(format!("mv: {} exists (-n)", target.display())));
                }
                std::fs::rename(&src, &target).map_err(|error| {
                    if error.kind() == std::io::ErrorKind::CrossesDevices {
                        fail(format!(
                            "mv: cross-device move unsupported in P2: {} -> {}",
                            src.display(),
                            target.display()
                        ))
                    } else {
                        fail(format!(
                            "mv: {} -> {}: {error}",
                            src.display(),
                            target.display()
                        ))
                    }
                })?;
                facts.push(src.to_string_lossy().into_owned());
                facts.push(target.to_string_lossy().into_owned());
                targets.push(observe(&target, "moved"));
            }
        }
        MutationOp::Remove {
            targets: operands,
            recursive,
            force,
        } => {
            for operand in operands {
                let target = absolutize(&resolved.cwd, operand);
                let (kind, _) = kind_of(&target);
                if kind == "absent" {
                    if *force {
                        targets.push(observe(&target, "absent-ok-force"));
                        continue;
                    }
                    return Err(fail(format!("rm: {}: no such file", target.display())));
                }
                if kind == "dir" && !*recursive {
                    return Err(fail(format!(
                        "rm: {} is a directory (need -r)",
                        target.display()
                    )));
                }
                if kind == "dir" {
                    std::fs::remove_dir_all(&target)
                        .map_err(|e| fail(format!("rm: {}: {e}", target.display())))?;
                } else {
                    // Symlinks and files: removes the link itself, never the target.
                    std::fs::remove_file(&target)
                        .map_err(|e| fail(format!("rm: {}: {e}", target.display())))?;
                }
                facts.push(target.to_string_lossy().into_owned());
                targets.push(observe(&target, "removed"));
            }
        }
        MutationOp::MakeDir { dirs, parents } => {
            for dir in dirs {
                let target = absolutize(&resolved.cwd, dir);
                if *parents {
                    std::fs::create_dir_all(&target)
                        .map_err(|e| fail(format!("mkdir: {}: {e}", target.display())))?;
                } else {
                    std::fs::create_dir(&target)
                        .map_err(|e| fail(format!("mkdir: {}: {e}", target.display())))?;
                }
                facts.push(target.to_string_lossy().into_owned());
                targets.push(observe(&target, "created"));
            }
        }
        MutationOp::RemoveDir { dirs } => {
            for dir in dirs {
                let target = absolutize(&resolved.cwd, dir);
                std::fs::remove_dir(&target)
                    .map_err(|e| fail(format!("rmdir: {}: {e}", target.display())))?;
                facts.push(target.to_string_lossy().into_owned());
                targets.push(observe(&target, "removed"));
            }
        }
        MutationOp::Touch { files, no_create } => {
            for file in files {
                let target = absolutize(&resolved.cwd, file);
                if kind_of(&target).0 == "absent" {
                    if *no_create {
                        targets.push(observe(&target, "absent-ok-nocreate"));
                        continue;
                    }
                    std::fs::write(&target, b"")
                        .map_err(|e| fail(format!("touch: {}: {e}", target.display())))?;
                } else if kind_of(&target).0 == "file" {
                    let now = filetime_now();
                    set_mtime(&target, now)
                        .map_err(|e| fail(format!("touch: {}: {e}", target.display())))?;
                }
                facts.push(target.to_string_lossy().into_owned());
                targets.push(observe(&target, "touched"));
            }
        }
        MutationOp::Link {
            sources,
            dest,
            symbolic,
        } => {
            let dest_path = absolutize(&resolved.cwd, dest);
            let dest_is_dir = dest_path.is_dir() && !dest_path.is_symlink();
            if sources.len() > 1 && !dest_is_dir {
                return Err(fail(
                    "ln: multiple sources need an existing directory DEST".to_string(),
                ));
            }
            for source in sources {
                let src = absolutize(&resolved.cwd, source);
                let target = if dest_is_dir || sources.len() > 1 {
                    let name = src
                        .file_name()
                        .ok_or_else(|| fail(format!("ln: bad source name {}", src.display())))?;
                    dest_path.join(name)
                } else {
                    dest_path.clone()
                };
                if *symbolic {
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(&src, &target)
                        .map_err(|e| fail(format!("ln: {}: {e}", target.display())))?;
                    #[cfg(windows)]
                    {
                        let (kind, _) = kind_of(&src);
                        let result = if kind == "dir" {
                            std::os::windows::fs::symlink_dir(&src, &target)
                        } else {
                            std::os::windows::fs::symlink_file(&src, &target)
                        };
                        result.map_err(|e| fail(format!("ln: {}: {e}", target.display())))?;
                    }
                } else {
                    std::fs::hard_link(&src, &target)
                        .map_err(|e| fail(format!("ln: {}: {e}", target.display())))?;
                }
                facts.push(target.to_string_lossy().into_owned());
                targets.push(observe(&target, "linked"));
            }
        }
    }
    facts.sort();
    facts.dedup();
    let truth = json!({
        "op": op_name,
        "bundle": bundle,
        "targets": targets.len(),
        "facts_to_invalidate": facts.len(),
    });
    Ok(MutationReport {
        op: op_name.to_string(),
        bundle,
        targets,
        facts_to_invalidate: facts,
        truth,
    })
}

fn copy_one(src: &Path, dest: &Path, recursive: bool, no_clobber: bool) -> Result<(), String> {
    let (kind, _) = kind_of(src);
    if kind == "absent" {
        return Err(format!("cp: {}: no such file", src.display()));
    }
    if no_clobber && kind_of(dest).0 != "absent" {
        return Err(format!("cp: {} exists (-n)", dest.display()));
    }
    if kind == "link" {
        let target = std::fs::read_link(src).map_err(|e| format!("cp: {}: {e}", src.display()))?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, dest)
            .map_err(|e| format!("cp: {}: {e}", dest.display()))?;
        #[cfg(windows)]
        {
            // Reproduce the link without following it; dir-ness is unknown
            // from the link alone, so try file first, then dir.
            if std::os::windows::fs::symlink_file(&target, dest).is_err() {
                std::os::windows::fs::symlink_dir(&target, dest)
                    .map_err(|e| format!("cp: {}: {e}", dest.display()))?;
            }
        }
        return Ok(());
    }
    if kind == "dir" {
        if !recursive {
            return Err(format!("cp: {} is a directory (need -r)", src.display()));
        }
        copy_dir_recursive(src, dest)?;
        return Ok(());
    }
    if let Some(parent) = dest.parent()
        && !parent.exists()
    {
        return Err(format!(
            "cp: destination directory {} missing",
            parent.display()
        ));
    }
    std::fs::copy(src, dest)
        .map_err(|e| format!("cp: {} -> {}: {e}", src.display(), dest.display()))?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("cp: {}: {e}", dest.display()))?;
    for entry in walkdir::WalkDir::new(src).sort_by_file_name().min_depth(1) {
        let entry = entry.map_err(|e| format!("cp: traversal under {}: {e}", src.display()))?;
        let relative = entry
            .path()
            .strip_prefix(src)
            .map_err(|e| format!("cp: {e}"))?;
        let target = dest.join(relative);
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            let link_target = std::fs::read_link(entry.path())
                .map_err(|e| format!("cp: {}: {e}", entry.path().display()))?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&link_target, &target)
                .map_err(|e| format!("cp: {}: {e}", target.display()))?;
            #[cfg(windows)]
            {
                if std::os::windows::fs::symlink_file(&link_target, &target).is_err() {
                    std::os::windows::fs::symlink_dir(&link_target, &target)
                        .map_err(|e| format!("cp: {}: {e}", target.display()))?;
                }
            }
        } else if file_type.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|e| format!("cp: {}: {e}", target.display()))?;
        } else {
            std::fs::copy(entry.path(), &target)
                .map_err(|e| format!("cp: {}: {e}", target.display()))?;
        }
    }
    Ok(())
}

fn filetime_now() -> std::time::SystemTime {
    std::time::SystemTime::now()
}

fn set_mtime(path: &Path, time: std::time::SystemTime) -> std::io::Result<()> {
    // Stable-only mtime update: length-preserving rewrite is racy, so use
    // the platform handle API via `std::fs::File::set_modified`.
    let file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.set_modified(time)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"aaaa").expect("write");
        std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
        std::fs::write(dir.path().join("sub").join("b.txt"), b"bb").expect("write");
        dir
    }

    /// Tests construct admission explicitly: physical-logic tests, not
    /// authority acceptance (that lives in omen-authority).
    fn test_admission() -> Admission {
        Admission::Admitted {
            bundle: "test-bundle".to_string(),
        }
    }

    #[test]
    fn parse_covers_p2_surface_and_rejects_unknown_flags() {
        assert!(matches!(
            parse(&s(&["cp", "a", "b"])),
            Ok(MutationOp::Copy {
                recursive: false,
                ..
            })
        ));
        assert!(matches!(
            parse(&s(&["cp", "-r", "a", "b"])),
            Ok(MutationOp::Copy {
                recursive: true,
                ..
            })
        ));
        assert!(matches!(
            parse(&s(&["rm", "-rf", "x"])),
            Ok(MutationOp::Remove {
                recursive: true,
                force: true,
                ..
            })
        ));
        assert!(matches!(
            parse(&s(&["mkdir", "-p", "x"])),
            Ok(MutationOp::MakeDir { parents: true, .. })
        ));
        assert!(matches!(
            parse(&s(&["ln", "-s", "a", "b"])),
            Ok(MutationOp::Link { symbolic: true, .. })
        ));
        assert!(parse(&s(&["cp", "--preserve", "a", "b"])).is_err());
        assert!(parse(&s(&["rm"])).is_err());
        assert!(parse(&s(&["find", "."])).is_err());
    }

    #[test]
    fn refused_closed_performs_zero_effects() {
        let dir = fixture();
        let before = snapshot(dir.path());
        for argv in [
            s(&["rm", "-rf", "."]),
            s(&["cp", "-r", "sub", "copy"]),
            s(&["mkdir", "new"]),
        ] {
            let op = parse(&argv).expect("parses");
            let resolved = resolve(op, dir.path());
            match execute(&resolved, &Admission::shell_default()) {
                Err(MutationError::Refused(_)) => {}
                other => panic!("expected refusal, got {other:?}"),
            }
        }
        assert_eq!(
            snapshot(dir.path()),
            before,
            "refused mutations changed the tree"
        );
    }

    fn snapshot(root: &Path) -> Vec<(PathBuf, u64)> {
        let mut entries: Vec<(PathBuf, u64)> = walkdir::WalkDir::new(root)
            .sort_by_file_name()
            .into_iter()
            .flatten()
            .map(|e| {
                (
                    e.path().to_path_buf(),
                    e.metadata().map(|m| m.len()).unwrap_or(0),
                )
            })
            .collect();
        entries.sort();
        entries
    }

    #[test]
    fn estimate_counts_overwrites_and_outside() {
        let dir = fixture();
        let op = parse(&s(&["cp", "-r", "sub", "sub2"])).expect("parses");
        let est = estimate(&resolve(op, dir.path()));
        assert!(est.affected.iter().any(|a| a.path.ends_with("sub")));
        assert!(est.overwrites.is_empty());
        assert_eq!(est.total_bytes, 2);
        // Overwrite detection.
        let op = parse(&s(&["cp", "a.txt", "sub"])).expect("parses");
        let est = estimate(&resolve(op, dir.path()));
        assert!(
            est.overwrites.is_empty(),
            "copy into dir without clash must not flag"
        );
        let op = parse(&s(&["cp", "a.txt", "a.txt"])).expect("parses");
        let est = estimate(&resolve(op, dir.path()));
        assert_eq!(est.overwrites.len(), 1);
        // Outside-cwd detection.
        let op = parse(&s(&["rm", "../outside"])).expect("parses");
        let est = estimate(&resolve(op, dir.path()));
        assert_eq!(est.outside_cwd.len(), 1);
        let (severity, _) = preflight(
            &est,
            &MutationOp::Remove {
                targets: vec![],
                recursive: false,
                force: false,
            },
        );
        assert_eq!(severity, Severity::High);
    }

    /// Symlinked dirs are recorded, never descended (unix-gated: link
    /// creation needs privilege on Windows; the documented rule holds on
    /// both).
    #[cfg(unix)]
    #[test]
    fn estimate_records_symlinked_dirs_without_following() {
        use std::os::unix::fs::symlink;
        let dir = fixture();
        symlink(dir.path().join("sub"), dir.path().join("linked-sub")).expect("symlink");
        let op = parse(&s(&["cp", "-r", "linked-sub", "copy"])).expect("parses");
        let est = estimate(&resolve(op, dir.path()));
        assert!(
            est.symlinked_dirs.iter().any(|p| p.ends_with("linked-sub")),
            "link recorded: {:?}",
            est.symlinked_dirs
        );
        // The link target's contents are NOT enumerated through the link.
        assert!(
            !est.affected
                .iter()
                .any(|a| a.path.to_string_lossy().contains("linked-sub/b.txt")),
            "must not descend through the link"
        );
    }

    #[test]
    fn admitted_executes_every_op_with_observation() {
        let dir = fixture();
        let run = |argv: &[&str]| {
            let op = parse(&s(argv)).expect("parses");
            execute(&resolve(op, dir.path()), &test_admission())
        };
        let report = run(&["mkdir", "-p", "new/nested"]).expect("executes");
        assert!(dir.path().join("new/nested").is_dir());
        assert!(!report.facts_to_invalidate.is_empty());
        let report = run(&["touch", "new/file.txt"]).expect("executes");
        assert_eq!(
            dir.path()
                .join("new/file.txt")
                .metadata()
                .expect("stat")
                .len(),
            0
        );
        assert_eq!(report.targets[0].bytes_after, Some(0));
        run(&["cp", "a.txt", "new/copied.txt"]).expect("cp");
        assert_eq!(
            std::fs::read(dir.path().join("new/copied.txt")).expect("read"),
            b"aaaa"
        );
        run(&["mv", "new/copied.txt", "new/moved.txt"]).expect("mv");
        assert!(!dir.path().join("new/copied.txt").exists());
        run(&["rm", "-r", "new"]).expect("rm -r");
        assert!(!dir.path().join("new").exists());
        run(&["rmdir", "sub"]).expect_err("non-empty rmdir must fail");
    }

    /// Symlink creation needs a Windows privilege most dev shells lack
    /// (os error 1314). Probe once; skip gracefully when unavailable so
    /// the suite stays green without pretending the ops were exercised.
    fn symlink_privilege(dir: &Path) -> bool {
        let probe_src = dir.join("probe-src.txt");
        let probe_dst = dir.join("probe-link");
        if std::fs::write(&probe_src, b"p").is_err() {
            return false;
        }
        #[cfg(unix)]
        let ok = std::os::unix::fs::symlink(&probe_src, &probe_dst).is_ok();
        #[cfg(windows)]
        let ok = std::os::windows::fs::symlink_file(&probe_src, &probe_dst).is_ok();
        let _ = std::fs::remove_file(&probe_dst);
        let _ = std::fs::remove_file(&probe_src);
        ok
    }

    #[test]
    fn symlink_ops_reproduce_links_without_following() {
        let dir = fixture();
        if !symlink_privilege(dir.path()) {
            eprintln!("SKIP symlink_ops: no symlink privilege in this environment");
            return;
        }
        let run = |argv: &[&str]| {
            let op = parse(&s(argv)).expect("parses");
            execute(&resolve(op, dir.path()), &test_admission())
        };
        run(&["ln", "-s", "a.txt", "link.txt"]).expect("ln -s");
        assert!(
            std::fs::symlink_metadata(dir.path().join("link.txt"))
                .expect("stat")
                .file_type()
                .is_symlink()
        );
        run(&["rm", "link.txt"]).expect("rm link");
        assert!(!dir.path().join("link.txt").exists());
        assert!(
            dir.path().join("a.txt").exists(),
            "rm on a link must not touch the target"
        );
        // Recursive copy reproduces links without following them.
        run(&["ln", "-s", "a.txt", "sub/sublink"]).expect("ln -s in sub");
        run(&["cp", "-r", "sub", "sub-copy"]).expect("cp -r");
        assert!(dir.path().join("sub-copy").is_dir());
        assert!(
            std::fs::symlink_metadata(dir.path().join("sub-copy/sublink"))
                .expect("stat")
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn guards_hold() {
        let dir = fixture();
        let run = |argv: &[&str]| {
            let op = parse(&s(argv)).expect("parses");
            execute(&resolve(op, dir.path()), &test_admission())
        };
        // No-clobber, missing source, dir without -r, missing rmdir target.
        assert!(run(&["cp", "-n", "a.txt", "a.txt"]).is_err());
        assert!(run(&["cp", "absent", "x"]).is_err());
        assert!(run(&["cp", "sub", "sub-copy"]).is_err());
        assert!(run(&["rm", "absent"]).is_err());
        assert!(run(&["rm", "-f", "absent"]).is_ok());
        run(&["cp", "-r", "sub", "sub-copy"]).expect("cp -r");
        assert!(dir.path().join("sub-copy").is_dir());
    }
}
