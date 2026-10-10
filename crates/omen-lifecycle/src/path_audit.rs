//! Managed-PATH audit: pure, testable decisions over PATH entries.
//!
//! Incident: cargo `target/debug` (+ `deps`, build `out` dirs) sat on the
//! User PATH ahead of any managed install, so `omen` resolved to an
//! arbitrary debug build. The installer must detect shadowing entries,
//! remove only verified-stale Omen build dirs, and register the canonical
//! managed bin — never touching unrelated entries.
//!
//! A directory is *verified stale* when it is inside a cargo `target`
//! tree (`target/debug`, `target/release`, and their `deps` / `build`
//! children). Anything else Omen-related but unrecognized is reported,
//! never auto-removed (NEEDS_HUMAN stays human).

/// Normalized (lowercased, `/`-separated) form for comparison.
fn norm(entry: &str) -> String {
    entry.replace('\\', "/").to_lowercase()
}

/// True when `entry` is a cargo target build dir (debug/release tree),
/// matched on whole path components: `target/debugger` does NOT match,
/// `target/debug` and any child (`deps`, `build/<hash>/out`, …) do.
pub fn is_stale_build_dir(entry: &str) -> bool {
    norm(entry)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .windows(2)
        .any(|w| w[0] == "target" && (w[1] == "debug" || w[1] == "release"))
}

/// Executable names that resolve an Omen installation (both binaries).
const OMEN_EXES: &[&str] = &["omen.exe", "omen", "omend.exe", "omend"];

/// True when `entry` equals the canonical managed bin (case-insensitive).
pub fn is_canonical_bin(entry: &str, canonical_bin: &str) -> bool {
    norm(entry) == norm(canonical_bin)
}

/// Entries from `path_entries` (in order) that would shadow the managed
/// install: verified-stale build dirs that are NOT the canonical bin.
/// Returned in PATH order (first = the actual shadower).
pub fn shadowing_entries<'a>(path_entries: &'a [String], canonical_bin: &str) -> Vec<&'a str> {
    path_entries
        .iter()
        .map(String::as_str)
        .filter(|e| is_stale_build_dir(e) && !is_canonical_bin(e, canonical_bin))
        .collect()
}

/// PATH entries after registering `bin`: prepended when missing,
/// deduplicated (case-insensitive), order otherwise preserved.
pub fn entries_after_register(path_entries: &[String], bin: &str) -> Vec<String> {
    if path_entries.iter().any(|e| is_canonical_bin(e, bin)) {
        return path_entries.to_vec();
    }
    let mut out = Vec::with_capacity(path_entries.len() + 1);
    out.push(bin.to_string());
    out.extend(path_entries.iter().cloned());
    out
}

/// PATH entries after removing exactly `remove` (matched case-insensitively).
/// Entries not present are ignored; everything else is preserved verbatim.
pub fn entries_after_clean(path_entries: &[String], remove: &[String]) -> Vec<String> {
    path_entries
        .iter()
        .filter(|e| !remove.iter().any(|r| norm(e) == norm(r)))
        .cloned()
        .collect()
}

/// Join entries for the registry value.
pub fn join_entries(entries: &[String]) -> String {
    entries.join(";")
}

/// Parse `reg query HKCU\Environment /v Path` output into entries.
/// Pure for testability: takes captured stdout, returns PATH entries.
/// Returns None when the output has no readable Path value.
fn parse_reg_path_output(stdout: &str) -> Option<Vec<String>> {
    for line in stdout.lines() {
        let t = line.trim();
        // Typical row: `Path    REG_EXPAND_SZ    C:\a;C:\b`
        let mut parts = t.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        if !name.eq_ignore_ascii_case("path") {
            continue;
        }
        let kind = parts.next().unwrap_or("");
        if !kind.eq_ignore_ascii_case("REG_SZ") && !kind.eq_ignore_ascii_case("REG_EXPAND_SZ") {
            continue;
        }
        let value: String = parts.collect::<Vec<_>>().join(" ");
        if value.is_empty() {
            return None;
        }
        return Some(
            value
                .split(';')
                .map(str::trim)
                .filter(|e| !e.is_empty())
                .map(str::to_string)
                .collect(),
        );
    }
    None
}

/// Read the registry User PATH (what fresh shells resolve). Windows-only;
/// returns None when reg.exe is unavailable or the value is unreadable.
/// Observational: never writes. Shared by doctor and repair so both see
/// the same registry truth (never the inheriting process copy).
#[cfg(windows)]
pub fn read_registry_user_path() -> Option<Vec<String>> {
    let out = std::process::Command::new("reg")
        .args(["query", r"HKCU\Environment", "/v", "Path"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_reg_path_output(&String::from_utf8_lossy(&out.stdout))
}

/// Non-Windows: no registry PATH exists; callers fall back to guidance.
#[cfg(not(windows))]
pub fn read_registry_user_path() -> Option<Vec<String>> {
    None
}

/// Read the registry Machine PATH (HKLM, read-only; what every fresh
/// shell inherits before the User value). Windows-only; None when
/// unreadable. Observational: never writes.
#[cfg(windows)]
pub fn read_registry_machine_path() -> Option<Vec<String>> {
    let out = std::process::Command::new("reg")
        .args([
            "query",
            r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            "/v",
            "Path",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_reg_path_output(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(windows))]
pub fn read_registry_machine_path() -> Option<Vec<String>> {
    None
}

/// Write the registry User PATH on Windows via reg.exe (no setx 1024-char
/// truncation). Entries are joined verbatim. Returns Ok on success.
#[cfg(windows)]
pub fn write_registry_user_path(entries: &[String]) -> Result<(), String> {
    let value = join_entries(entries);
    let status = std::process::Command::new("reg")
        .args([
            "add",
            r"HKCU\Environment",
            "/v",
            "Path",
            "/t",
            "REG_EXPAND_SZ",
            "/d",
            &value,
            "/f",
        ])
        .output()
        .map_err(|e| format!("registry PATH write failed: {e}"))?;
    if status.status.success() {
        Ok(())
    } else {
        Err(format!(
            "registry PATH write failed: {}",
            String::from_utf8_lossy(&status.stderr)
        ))
    }
}

#[cfg(test)]
mod reg_tests {
    use super::parse_reg_path_output;

    #[test]
    fn parses_expand_sz_row() {
        let out = "\nHKEY_CURRENT_USER\\Environment\n    Path    REG_EXPAND_SZ    C:\\a;C:\\b;;C:\\c \n\n";
        assert_eq!(
            parse_reg_path_output(out),
            Some(vec![
                "C:\\a".to_string(),
                "C:\\b".to_string(),
                "C:\\c".to_string()
            ])
        );
    }

    #[test]
    fn missing_or_empty_value_is_none() {
        assert_eq!(parse_reg_path_output("\n\n"), None);
        assert_eq!(
            parse_reg_path_output("HKEY_CURRENT_USER\\Environment\n"),
            None
        );
    }
}

/// Shadowing omen executable dirs for EXPLICIT entries (pure, testable).
/// `entries` are PATH entries in order; returns display paths that are
/// verified-stale build dirs (never the canonical bin) containing an
/// omen OR omend executable.
pub fn shadowing_omen_exe_dirs_in(base: &std::path::Path, entries: &[String]) -> Vec<String> {
    let canonical = base.join("bin").to_string_lossy().to_string();
    let mut out = Vec::new();
    for e in shadowing_entries(entries, &canonical) {
        let dir = std::path::Path::new(e);
        let has_exe = OMEN_EXES.iter().any(|x| dir.join(x).is_file());
        if has_exe && !out.contains(&e.to_string()) {
            out.push(e.to_string());
        }
    }
    out
}

/// Omen executables found in DIRS THAT ARE NEITHER stale build trees NOR
/// the canonical bin: unknown ownership. Reported for human review;
/// never planned for automatic removal.
pub fn unknown_omen_exe_dirs_in(base: &std::path::Path, entries: &[String]) -> Vec<String> {
    let canonical = base.join("bin").to_string_lossy().to_string();
    let mut out = Vec::new();
    for e in entries {
        if is_canonical_bin(e, &canonical) || is_stale_build_dir(e) {
            continue;
        }
        let dir = std::path::Path::new(e);
        let has_exe = OMEN_EXES.iter().any(|x| dir.join(x).is_file());
        if has_exe && !out.contains(e) {
            out.push(e.clone());
        }
    }
    out
}

/// Canonical bin entries beyond the first occurrence (case-insensitive):
/// duplicates that collapse safely.
pub fn duplicate_canonical_entries(entries: &[String], canonical_bin: &str) -> Vec<String> {
    let mut seen = false;
    let mut dupes = Vec::new();
    for e in entries {
        if is_canonical_bin(e, canonical_bin) {
            if seen && !dupes.iter().any(|d: &String| norm(d) == norm(e)) {
                dupes.push(e.clone());
            }
            seen = true;
        }
    }
    dupes
}

/// Collapse case-insensitive duplicate entries, keeping the first
/// occurrence of each. Order preserved.
pub fn entries_after_dedupe(entries: &[String]) -> Vec<String> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for e in entries {
        let n = norm(e);
        if !seen.contains(&n) {
            seen.push(n);
            out.push(e.clone());
        }
    }
    out
}

/// Parse `--version` output: true only for genuine Omen binaries
/// (`omen <version> … contract:<v> …`). Pure for testability.
pub fn omen_version_output_owned(text: &str) -> bool {
    let first = text.lines().next().unwrap_or("").trim();
    first.starts_with("omen ") && text.contains("contract:")
}

/// Verify a shadowing dir resolves an OMEN-owned executable: runs
/// `<dir>/omen(.exe) --version` bounded (10s, no shell) and checks the
/// identity banner. Only the omen binary proves ownership (omend has no
/// version banner contract); dirs whose omen binary is missing or
/// foreign return false. Never deletes files.
pub fn omen_owned_exe_dir(dir: &std::path::Path) -> bool {
    let exe = dir.join(if cfg!(windows) { "omen.exe" } else { "omen" });
    if !exe.is_file() {
        return false;
    }
    let mut child = match std::process::Command::new(&exe)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return false;
                }
                let mut text = String::new();
                if let Some(mut out) = child.stdout.take() {
                    use std::io::Read;
                    let _ = out.read_to_string(&mut text);
                }
                return omen_version_output_owned(&text);
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => {
                let _ = child.kill();
                return false;
            }
        }
    }
}
/// Shadowing omen executable dirs on the PROCESS path: verified-stale
/// build dirs (never the canonical managed bin) that contain an omen
/// executable. Thin wrapper over [`shadowing_omen_exe_dirs_in`] for the
/// observational doctor path.
pub fn shadowing_omen_exe_dirs(base: &std::path::Path) -> Vec<String> {
    let entries: Vec<String> = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .map(|d| d.to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    shadowing_omen_exe_dirs_in(base, &entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANON: &str = "C:/Users/u/AppData/Local/Omen/bin";

    #[test]
    fn stale_patterns_match_build_trees_only() {
        for stale in [
            "D:/repo/target/debug",
            "D:/repo/target/debug/deps",
            "D:/repo/target/release",
            "D:/repo/target/release/deps",
            "D:/repo/target/debug/build/ring-abc/out",
            "/home/u/repo/target/debug",
        ] {
            assert!(is_stale_build_dir(stale), "{stale}");
        }
        for fresh in [
            CANON,
            "C:/Program Files/nodejs",
            "D:/repo/src",
            "D:/repo/target", // the bare target dir is not itself an exe dir
            "D:/my-target/debugger/bin",
            "D:/target/debugger/bin", // substring trap: not a build tree
            "D:/repo/targeted/release-notes",
        ] {
            assert!(!is_stale_build_dir(fresh), "{fresh}");
        }
    }

    #[test]
    fn shadowing_finds_debug_ahead_of_canonical() {
        let path = vec![
            "C:/Windows/system32".to_string(),
            "D:/Projects/Omen/Omen Shell/target/debug".to_string(),
            "D:/Projects/Omen/Omen Shell/target/debug/deps".to_string(),
            CANON.to_string(),
        ];
        let shadow = shadowing_entries(&path, CANON);
        assert_eq!(
            shadow,
            vec![
                "D:/Projects/Omen/Omen Shell/target/debug",
                "D:/Projects/Omen/Omen Shell/target/debug/deps",
            ]
        );
    }

    #[test]
    fn register_prepends_once_and_dedupes_case() {
        let path = vec!["C:/Windows".to_string()];
        let once = entries_after_register(&path, CANON);
        assert_eq!(once[0], CANON);
        let twice = entries_after_register(&once, "c:/users/u/appdata/local/omen/bin");
        assert_eq!(twice.len(), 2, "case-insensitive dedupe");
    }

    #[test]
    fn version_banner_proves_omen_ownership() {
        assert!(omen_version_output_owned(
            "omen 0.9.0-preview.25 contract:0.8 commit:abc target:x86_64-pc-windows-msvc profile:release\n"
        ));
        for foreign in [
            "",
            "omen.exe: command not found\n",
            "my-omen-tool 1.0\n",
            "omen without a contract banner\n",
            "Omen 0.9.0 contract:0.8\n", // capital O: not the banner
        ] {
            assert!(!omen_version_output_owned(foreign), "{foreign:?}");
        }
    }

    #[test]
    fn duplicate_canonical_detection_and_dedupe() {
        let path = vec![
            CANON.to_string(),
            "C:/Windows".to_string(),
            "c:/users/u/appdata/local/omen/bin".to_string(),
        ];
        assert_eq!(duplicate_canonical_entries(&path, CANON).len(), 1);
        let deduped = entries_after_dedupe(&path);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0], CANON);
    }

    #[test]
    fn clean_removes_only_listed_entries() {
        let path = vec![
            "C:/Windows".to_string(),
            "D:/repo/target/debug".to_string(),
            "D:/keep".to_string(),
        ];
        let out = entries_after_clean(&path, &["d:/REPO/target/debug".to_string()]);
        assert_eq!(out, vec!["C:/Windows".to_string(), "D:/keep".to_string()]);
    }
}
