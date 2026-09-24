//! Human path presentation.
//!
//! Internal canonical path truth (including Windows verbatim / extended-length
//! `\\?\` prefixes) is preserved everywhere inside Omen.  This module strips
//! those prefixes for **human display only**.  Machine truth and human
//! rendering are separate layers.

use std::path::Path;

/// Strips the Windows verbatim / extended-length prefix for human display.
///
/// | Internal | Human |
/// |---|---|
/// | `\\?\D:\` | `D:\` |
/// | `\\?\C:\Program Files` | `C:\Program Files` |
/// | `\\?\UNC\server\share` | `\\server\share` |
/// | `//?/D:/` | `D:/` |
/// | `/home/user/project` | `/home/user/project` (unchanged) |
///
/// Only the presentation string changes.  The caller's `Path` / `PathBuf`
/// retains full canonical identity.
pub fn humanize_path(path: &Path) -> String {
    humanize_path_str(&path.to_string_lossy())
}

/// Same as [`humanize_path`] but operates on an already-extracted string.
pub fn humanize_path_str(s: &str) -> String {
    // Windows verbatim UNC:  \\?\UNC\server\share  ->  \\server\share
    if let Some(rest) = s.strip_prefix("\\\\?\\UNC\\") {
        return format!("\\\\{rest}");
    }
    // Windows verbatim:  \\?\D:\...  ->  D:\...
    if let Some(rest) = s.strip_prefix("\\\\?\\") {
        return rest.to_string();
    }
    // Forward-slash verbatim UNC:  //?/UNC/server/share  ->  //server/share
    if let Some(rest) = s.strip_prefix("//?/UNC/") {
        return format!("//{rest}");
    }
    // Forward-slash verbatim:  //?/D:/...  ->  D:/...
    if let Some(rest) = s.strip_prefix("//?/") {
        return rest.to_string();
    }
    s.to_string()
}

/// Renders a humanised path as a `file://` URI suitable for OSC 7 / OSC 8.
///
/// Strips the verbatim prefix and normalises separators to `/`.
pub fn path_to_file_uri(path: &Path) -> String {
    let human = humanize_path(path);
    let normalized = human.replace('\\', "/");
    if normalized.starts_with('/') {
        format!("file://localhost{normalized}")
    } else {
        format!("file://localhost/{normalized}")
    }
}
