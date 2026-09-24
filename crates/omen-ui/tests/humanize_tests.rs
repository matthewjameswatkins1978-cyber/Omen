//! Human path presentation tests.
//!
//! Proves: internal canonical path truth is preserved while human rendering
//! strips Windows verbatim/extended-length prefixes.  Machine truth and human
//! rendering are separate layers.

use omen_ui::humanize::{humanize_path, humanize_path_str, path_to_file_uri};
use std::path::Path;

// ---------------------------------------------------------------------------
// DRIVE ROOT
// ---------------------------------------------------------------------------

#[test]
fn humanize_strips_verbatim_drive_root() {
    assert_eq!(humanize_path_str(r"\\?\D:\"), r"D:\");
}

#[test]
fn humanize_strips_verbatim_drive_root_no_trailing_sep() {
    assert_eq!(humanize_path_str(r"\\?\D:"), "D:");
}

#[test]
fn humanize_strips_verbatim_c_drive_root() {
    assert_eq!(humanize_path_str(r"\\?\C:\"), r"C:\");
}

// ---------------------------------------------------------------------------
// NESTED DIRECTORIES
// ---------------------------------------------------------------------------

#[test]
fn humanize_strips_verbatim_nested_path() {
    assert_eq!(
        humanize_path_str(r"\\?\D:\Omen Shell\crates"),
        r"D:\Omen Shell\crates"
    );
}

#[test]
fn humanize_strips_verbatim_deep_path() {
    assert_eq!(
        humanize_path_str(r"\\?\C:\Users\Matmus\project\src\main.rs"),
        r"C:\Users\Matmus\project\src\main.rs"
    );
}

// ---------------------------------------------------------------------------
// SPACES
// ---------------------------------------------------------------------------

#[test]
fn humanize_strips_verbatim_spaces() {
    assert_eq!(
        humanize_path_str(r"\\?\C:\Program Files"),
        r"C:\Program Files"
    );
}

#[test]
fn humanize_strips_verbatim_spaces_in_nested() {
    assert_eq!(
        humanize_path_str(r"\\?\D:\My Projects\Omen Shell"),
        r"D:\My Projects\Omen Shell"
    );
}

// ---------------------------------------------------------------------------
// ORDINARY NON-EXTENDED PATHS (unchanged)
// ---------------------------------------------------------------------------

#[test]
fn humanize_preserves_ordinary_unix_path() {
    assert_eq!(
        humanize_path_str("/home/user/project"),
        "/home/user/project"
    );
}

#[test]
fn humanize_preserves_ordinary_windows_path() {
    assert_eq!(humanize_path_str(r"C:\Users\Matmus"), r"C:\Users\Matmus");
}

#[test]
fn humanize_preserves_relative_path() {
    assert_eq!(humanize_path_str("./src/main.rs"), "./src/main.rs");
}

// ---------------------------------------------------------------------------
// UNC PATHS
// ---------------------------------------------------------------------------

#[test]
fn humanize_strips_verbatim_unc() {
    assert_eq!(
        humanize_path_str(r"\\?\UNC\server\share"),
        r"\\server\share"
    );
}

#[test]
fn humanize_strips_verbatim_unc_with_path() {
    assert_eq!(
        humanize_path_str(r"\\?\UNC\server\share\folder\file.txt"),
        r"\\server\share\folder\file.txt"
    );
}

#[test]
fn humanize_preserves_ordinary_unc() {
    assert_eq!(
        humanize_path_str(r"\\server\share\folder"),
        r"\\server\share\folder"
    );
}

// ---------------------------------------------------------------------------
// FORWARD-SLASH VERBIM FORMS
// ---------------------------------------------------------------------------

#[test]
fn humanize_strips_forward_slash_verbatim() {
    assert_eq!(humanize_path_str("//?/D:/"), "D:/");
}

#[test]
fn humanize_strips_forward_slash_verbatim_unc() {
    assert_eq!(humanize_path_str("//?/UNC/server/share"), "//server/share");
}

// ---------------------------------------------------------------------------
// PATH API
// ---------------------------------------------------------------------------

#[test]
fn humanize_path_from_path_object() {
    let p = Path::new(r"\\?\D:\Omen Shell");
    assert_eq!(humanize_path(p), r"D:\Omen Shell");
}

// ---------------------------------------------------------------------------
// FILE URI
// ---------------------------------------------------------------------------

#[test]
fn path_to_file_uri_strips_verbatim() {
    let p = Path::new(r"\\?\D:\Omen Shell");
    assert_eq!(path_to_file_uri(p), "file://localhost/D:/Omen Shell");
}

#[test]
fn path_to_file_uri_unix() {
    let p = Path::new("/home/user/project");
    assert_eq!(path_to_file_uri(p), "file://localhost/home/user/project");
}

#[test]
fn path_to_file_uri_drive_root() {
    let p = Path::new(r"\\?\D:\");
    assert_eq!(path_to_file_uri(p), "file://localhost/D:/");
}

// ---------------------------------------------------------------------------
// INTERNAL TRUTH PRESERVED
// ---------------------------------------------------------------------------

#[test]
fn humanize_does_not_mutate_the_path_object() {
    // The Path itself is never modified — only the display string.
    let p = Path::new(r"\\?\D:\Omen Shell");
    let _display = humanize_path(p);
    // Path still has the verbatim prefix internally.
    assert!(p.to_string_lossy().starts_with(r"\\?\"));
}
