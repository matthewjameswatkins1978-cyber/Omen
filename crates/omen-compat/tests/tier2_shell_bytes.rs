//! Tier 2 shell-surface differential corpus: Omen builtins vs platform tools.
//!
//! Each case runs the same workflow through Omen's in-process builtins
//! (`run_if_builtin` / `run_pipeline`: pure `argv+cwd+env+stdin` bytes)
//! and through the platform's established tools, then compares normalized
//! outcomes. Where no sane oracle exists (binary NUL), the case pins the
//! byte-identity property instead — marked PROPERTY, never implied as
//! differential.
//!
//! Normalization (shared with `omen_compat::normalization` spirit):
//! CRLF becomes LF before comparison; exit codes compare exactly.
//! Classification on mismatch: Omen defect, deliberate difference,
//! platform difference, or oracle quirk — recorded at the assertion.

use omen_builtins::{BuiltinContext, PipelineStage};
use std::path::{Path, PathBuf};
use std::process::Command;

fn ctx_for(dir: &Path, stdin: &[u8]) -> BuiltinContext {
    BuiltinContext {
        cwd: dir.to_path_buf(),
        env: Vec::new(),
        stdin: stdin.to_vec(),
    }
}

fn s(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

/// Run one Omen builtin: (exit code, stdout bytes).
fn omen(argv: &[&str], stdin: &[u8], dir: &Path) -> (i32, Vec<u8>) {
    let out = omen_builtins::run_if_builtin(&s(argv), &ctx_for(dir, stdin))
        .unwrap_or_else(|| panic!("not a builtin: {argv:?}"))
        .unwrap_or_else(|e| panic!("builtin internal error: {e:?}"));
    (out.code, out.stdout)
}

/// Run an Omen pipeline: exit of the last stage + its stdout.
fn omen_pipe(stages: &[&[&str]], stdin: &[u8], dir: &Path) -> (i32, Vec<u8>) {
    let stages: Vec<PipelineStage> = stages
        .iter()
        .map(|a| PipelineStage {
            argv: s(a),
            env_overrides: Vec::new(),
        })
        .collect();
    let outputs =
        omen_builtins::run_pipeline(&stages, &ctx_for(dir, stdin)).expect("all-builtin pipeline");
    let last = outputs.last().expect("nonempty pipeline").as_ref();
    match last {
        Ok(out) => (out.code, out.stdout.clone()),
        Err(e) => panic!("pipeline internal error: {e:?}"),
    }
}

/// Run the platform oracle: (exit code, stdout bytes).
fn oracle(program: &str, args: &[&str], dir: &Path) -> (i32, Vec<u8>) {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("oracle would not spawn: {program} {e}"));
    (out.status.code().unwrap_or(-1), out.stdout)
}

/// CRLF becomes LF; lone CR untouched (byte truth otherwise preserved).
fn norm(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            out.push(b'\n');
            i += 2;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

fn fixture(dir: &Path) -> PathBuf {
    std::fs::write(dir.join("a.txt"), b"alpha\nbeta\ngamma\nalpha beta\n").unwrap();
    std::fs::write(dir.join("empty.txt"), b"").unwrap();
    dir.to_path_buf()
}

#[test]
fn cat_matches_oracle_bytes() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let (code, out) = omen(&["cat", "a.txt"], b"", dir.path());
    assert_eq!(code, 0);
    #[cfg(windows)]
    let (ocode, oout) = oracle("cmd", &["/c", "type", "a.txt"], dir.path());
    #[cfg(not(windows))]
    let (ocode, oout) = oracle("cat", &["a.txt"], dir.path());
    assert_eq!(ocode, 0, "oracle must succeed");
    assert_eq!(norm(&out), norm(&oout), "cat bytes match oracle");
}

#[test]
fn grep_literal_set_and_exits_match_oracle() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let (code, out) = omen(&["grep", "alpha", "a.txt"], b"", dir.path());
    assert_eq!(code, 0);
    #[cfg(windows)]
    let (ocode, oout) = oracle("findstr", &["/L", "/C:alpha", "a.txt"], dir.path());
    #[cfg(not(windows))]
    let (ocode, oout) = oracle("grep", &["-F", "alpha", "a.txt"], dir.path());
    assert_eq!(ocode, 0);
    assert_eq!(norm(&out), norm(&oout), "grep literal set matches");
    // No-match exit 1 parity.
    let (code, _) = omen(&["grep", "zzz-nope", "a.txt"], b"", dir.path());
    #[cfg(windows)]
    let (ocode, _) = oracle("findstr", &["/L", "/C:zzz-nope", "a.txt"], dir.path());
    #[cfg(not(windows))]
    let (ocode, _) = oracle("grep", &["-F", "zzz-nope", "a.txt"], dir.path());
    assert_eq!((code, ocode), (1, 1), "grep no-match exit parity");
}

#[test]
fn sort_ascii_order_matches_oracle() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("u.txt"), b"pear\napple\nFig\napple\n").unwrap();
    // Lowercase-only comparison: cmd sort is case-insensitive (platform
    // difference, documented); byte order is compared on folded input.
    std::fs::write(dir.path().join("l.txt"), b"pear\napple\nfig\napple\n").unwrap();
    let (code, out) = omen(&["sort", "l.txt"], b"", dir.path());
    assert_eq!(code, 0);
    #[cfg(windows)]
    let (ocode, oout) = oracle("cmd", &["/c", "sort", "l.txt"], dir.path());
    #[cfg(not(windows))]
    let (ocode, oout) = oracle("sort", &["l.txt"], dir.path());
    assert_eq!(ocode, 0);
    assert_eq!(norm(&out), norm(&oout), "sort order matches oracle");
}

#[test]
fn head_tail_counts_match_oracle() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let (code, out) = omen(&["head", "-n", "2", "a.txt"], b"", dir.path());
    assert_eq!(code, 0);
    #[cfg(windows)]
    let (ocode, oout) = oracle(
        "powershell",
        &["-NoProfile", "-Command", "Get-Content a.txt -TotalCount 2"],
        dir.path(),
    );
    #[cfg(not(windows))]
    let (ocode, oout) = oracle("head", &["-n", "2", "a.txt"], dir.path());
    assert_eq!(ocode, 0);
    assert_eq!(norm(&out), norm(&oout), "head -n matches oracle");
    let (code, out) = omen(&["tail", "-n", "2", "a.txt"], b"", dir.path());
    assert_eq!(code, 0);
    #[cfg(windows)]
    let (ocode, oout) = oracle(
        "powershell",
        &["-NoProfile", "-Command", "Get-Content a.txt -Tail 2"],
        dir.path(),
    );
    #[cfg(not(windows))]
    let (ocode, oout) = oracle("tail", &["-n", "2", "a.txt"], dir.path());
    assert_eq!(ocode, 0);
    assert_eq!(norm(&out), norm(&oout), "tail -n matches oracle");
}

#[test]
fn pipeline_end_to_end_matches_shell() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    // cat | grep | sort, all builtins in-process.
    let (code, out) = omen_pipe(
        &[&["cat", "a.txt"], &["grep", "alpha"], &["sort", "-u"]],
        b"",
        dir.path(),
    );
    assert_eq!(code, 0);
    #[cfg(windows)]
    let (ocode, oout) = oracle(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "Get-Content a.txt | Select-String alpha | ForEach-Object { $_.Line } | Sort-Object -Unique",
        ],
        dir.path(),
    );
    #[cfg(not(windows))]
    let (ocode, oout) = oracle(
        "sh",
        &["-c", "cat a.txt | grep -F alpha | sort -u"],
        dir.path(),
    );
    assert_eq!(ocode, 0, "oracle pipeline must succeed");
    assert_eq!(norm(&out), norm(&oout), "pipeline bytes match shell");
}

#[test]
fn binary_nul_round_trip_property() {
    // PROPERTY (no sane cmd oracle preserves NUL): NUL bytes inside
    // lines survive sort byte-exactly (NUL sorts as 0x00).
    let dir = tempfile::tempdir().unwrap();
    let input = b"b\x00\na\x00\nc\n";
    let (code, out) = omen_pipe(&[&["cat"], &["sort", "-u"]], input, dir.path());
    assert_eq!(code, 0);
    assert_eq!(out, b"a\x00\nb\x00\nc\n", "NUL bytes sort byte-exactly");
}

#[test]
fn spaces_and_unicode_names_match_oracle() {
    let dir = tempfile::tempdir().unwrap();
    let spaced = dir.path().join("my file.txt");
    std::fs::write(&spaced, b"spaced content\n").unwrap();
    let uni = dir.path().join("z-é-ß.txt");
    std::fs::write(&uni, "unicode content é\n".as_bytes()).unwrap();
    for name in ["my file.txt", "z-é-ß.txt"] {
        let (code, out) = omen(&["cat", name], b"", dir.path());
        assert_eq!(code, 0, "{name} dispatches");
        #[cfg(windows)]
        let (ocode, oout) = oracle("cmd", &["/c", "type", name], dir.path());
        #[cfg(not(windows))]
        let (ocode, oout) = oracle("cat", &[name], dir.path());
        assert_eq!(ocode, 0, "oracle reads {name}");
        assert_eq!(norm(&out), norm(&oout), "{name} bytes match");
    }
}
