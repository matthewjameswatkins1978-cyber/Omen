//! Archive extraction security (H item 73).
//!
//! Update packages ship as zips (conveyor). Never trust archive paths:
//! reject absolute paths, traversal (`..`), unexpected executable
//! placement, duplicate critical files, and malformed manifest
//! relationships. Bytes are verified AFTER extraction as well as package
//! integrity (caller supplies expected digests).

use crate::error::LifecycleError;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Files the package manifest is allowed to place, relative to the stage
/// dir. Anything else is "unexpected placement" and fails closed.
pub const ALLOWED_PACKAGE_FILES: &[&str] =
    &["omen", "omen.exe", "omend", "omend.exe", "manifest.json"];

/// The conveyor ships a small acceptance fixture alongside the binaries.
/// `fixture/` entries are data, never executables, and stay allowlisted as
/// a subtree; binaries and the manifest must sit at top level.
pub const ALLOWED_FIXTURE_PREFIX: &str = "fixture/";

/// Validate one zip entry name. Returns the safe relative path or a
/// refusal naming the exact problem.
pub fn validate_entry_name(name: &str) -> Result<PathBuf, LifecycleError> {
    if name.is_empty() {
        return Err(LifecycleError::Stage(
            "empty archive entry name".to_string(),
        ));
    }
    // Zip names always use forward slashes; reject backslashes (Windows
    // drive/UNC smuggling) and absolute paths.
    if name.contains('\\')
        || name.starts_with('/')
        || name.starts_with("..")
        || Path::new(name).is_absolute()
    {
        return Err(LifecycleError::Stage(format!(
            "archive rejects absolute/escaped entry: {name}"
        )));
    }
    let path = PathBuf::from(name);
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(LifecycleError::Stage(format!(
            "archive rejects traversal entry: {name}"
        )));
    }
    // No drive-letter or UNC prefixes after normalization.
    let norm = name.replace('/', "");
    if norm.len() > 1 && norm.as_bytes().get(1) == Some(&b':') {
        return Err(LifecycleError::Stage(format!(
            "archive rejects drive-prefixed entry: {name}"
        )));
    }
    if path.components().count() > 4 {
        return Err(LifecycleError::Stage(format!(
            "archive rejects deeply nested entry: {name}"
        )));
    }
    Ok(path)
}

/// Validate the full entry list: placement allowlist + duplicate critical
/// files + manifest presence.
pub fn validate_entry_list(names: &[&str]) -> Result<(), LifecycleError> {
    let mut seen = BTreeSet::new();
    let mut has_manifest = false;
    let mut has_binary = false;
    for name in names {
        let rel = validate_entry_name(name)?;
        let flat = rel.to_string_lossy().replace('\\', "/");
        let file = flat.rsplit('/').next().unwrap_or("");
        let is_fixture_data = flat.starts_with(ALLOWED_FIXTURE_PREFIX);
        let is_top_level_binary = !flat.contains('/') && ALLOWED_PACKAGE_FILES.contains(&file);
        // Executables anywhere below top level are never legitimate
        // placement — including inside the fixture data subtree.
        if flat.contains('/')
            && (file == "omen" || file == "omen.exe" || file == "omend" || file == "omend.exe")
        {
            return Err(LifecycleError::Stage(format!(
                "archive rejects nested executable: {name}"
            )));
        }
        if !(is_top_level_binary || is_fixture_data) {
            return Err(LifecycleError::Stage(format!(
                "archive rejects unexpected file placement: {name}"
            )));
        }
        if !seen.insert(flat.clone()) {
            return Err(LifecycleError::Stage(format!(
                "archive rejects duplicate entry: {name}"
            )));
        }
        if file == "manifest.json" {
            has_manifest = true;
        }
        if file == "omen" || file == "omen.exe" {
            has_binary = true;
        }
    }
    if !has_manifest {
        return Err(LifecycleError::Manifest(
            "package manifest missing from archive".to_string(),
        ));
    }
    if !has_binary {
        return Err(LifecycleError::Manifest(
            "omen binary missing from archive".to_string(),
        ));
    }
    Ok(())
}

/// Extract a validated zip archive into `dest`. Fails closed on any
/// violation; on success returns the extracted relative paths. Bounded:
/// refuses entries over 512MiB and total over 1GiB (zip-bomb guard).
pub fn extract_validated(archive: &Path, dest: &Path) -> Result<Vec<PathBuf>, LifecycleError> {
    const MAX_ENTRY: u64 = 512 * 1024 * 1024;
    const MAX_TOTAL: u64 = 1024 * 1024 * 1024;
    let file = std::fs::File::open(archive).map_err(|e| LifecycleError::Io(e.to_string()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| LifecycleError::Stage(e.to_string()))?;
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .collect();
    let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    validate_entry_list(&name_refs)?;

    std::fs::create_dir_all(dest).map_err(|e| LifecycleError::Io(e.to_string()))?;
    let mut total = 0u64;
    let mut extracted = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| LifecycleError::Stage(e.to_string()))?;
        let rel = validate_entry_name(entry.name())?;
        if entry.is_dir() {
            continue;
        }
        let size = entry.size();
        if size > MAX_ENTRY {
            return Err(LifecycleError::Stage(format!(
                "archive entry too large: {} ({size} bytes)",
                entry.name()
            )));
        }
        total += size;
        if total > MAX_TOTAL {
            return Err(LifecycleError::Stage("archive total too large".to_string()));
        }
        let out = dest.join(&rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| LifecycleError::Io(e.to_string()))?;
        }
        // Containment re-check after join (junction surprise guard).
        if !out.starts_with(dest) {
            return Err(LifecycleError::Stage(format!(
                "archive entry escapes destination: {}",
                entry.name()
            )));
        }
        let mut f = std::fs::File::create(&out).map_err(|e| LifecycleError::Io(e.to_string()))?;
        std::io::copy(&mut entry, &mut f).map_err(|e| LifecycleError::Io(e.to_string()))?;
        extracted.push(rel);
    }
    Ok(extracted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_absolute() {
        for bad in [
            "../evil.exe",
            "/abs/path",
            "a/../../evil",
            "C:/win/evil.exe",
            "a\\b.exe",
            "",
        ] {
            assert!(validate_entry_name(bad).is_err(), "{bad}");
        }
        assert!(validate_entry_name("omen.exe").is_ok());
        assert!(validate_entry_name("bin/omen").is_ok());
    }

    #[test]
    fn rejects_unexpected_and_duplicate() {
        assert!(validate_entry_list(&["omen.exe", "evil.sh", "manifest.json"]).is_err());
        assert!(validate_entry_list(&["omen.exe", "omen.exe", "manifest.json"]).is_err());
        assert!(validate_entry_list(&["omen.exe"]).is_err());
        assert!(validate_entry_list(&["manifest.json"]).is_err());
        assert!(validate_entry_list(&["omen.exe", "manifest.json"]).is_ok());
    }

    #[test]
    fn conveyor_layout_accepted_nested_exe_rejected() {
        // The real Preview package shape: top-level binaries + manifest +
        // acceptance fixture data.
        assert!(
            validate_entry_list(&[
                "omen.exe",
                "omend.exe",
                "manifest.json",
                "fixture/Cargo.toml",
                "fixture/Cargo.lock",
                "fixture/src/lib.rs",
            ])
            .is_ok()
        );
        assert!(validate_entry_list(&["omen.exe", "manifest.json", "fixture/omen.exe"]).is_err());
        assert!(validate_entry_list(&["omen.exe", "manifest.json", "sub/manifest.json"]).is_err());
    }

    #[test]
    fn extracts_good_zip_and_rejects_evil_zip() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.zip");
        {
            let f = std::fs::File::create(&good).unwrap();
            let mut w = zip::ZipWriter::new(f);
            for (name, data) in [
                ("omen.exe", b"fakebinary".as_slice()),
                ("manifest.json", b"{}".as_slice()),
            ] {
                w.start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                use std::io::Write;
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        let out = dir.path().join("out");
        let paths = extract_validated(&good, &out).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(out.join("omen.exe").is_file());

        let evil = dir.path().join("evil.zip");
        {
            let f = std::fs::File::create(&evil).unwrap();
            let mut w = zip::ZipWriter::new(f);
            w.start_file("../../evil.exe", zip::write::SimpleFileOptions::default())
                .unwrap();
            use std::io::Write;
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        assert!(extract_validated(&evil, &dir.path().join("out2")).is_err());
    }
}
