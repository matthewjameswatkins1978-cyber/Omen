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

/// True when `entry` is a cargo target build dir (debug/release tree).
/// Matches the dir itself and any child (`deps`, `build/<hash>/out`, …).
pub fn is_stale_build_dir(entry: &str) -> bool {
    let n = norm(entry);
    n.contains("/target/debug") || n.contains("/target/release")
}

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
/// omen executable.
pub fn shadowing_omen_exe_dirs_in(base: &std::path::Path, entries: &[String]) -> Vec<String> {
    let exe = if cfg!(windows) { "omen.exe" } else { "omen" };
    let canonical = base.join("bin").to_string_lossy().to_string();
    let mut out = Vec::new();
    for e in shadowing_entries(entries, &canonical) {
        if std::path::Path::new(e).join(exe).is_file() && !out.contains(&e.to_string()) {
            out.push(e.to_string());
        }
    }
    out
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
