//! Portable Windows drive-designator detection.
//!
//! This is pure string logic and is portable: it *detects* the `D:` pattern
//! without hard-coding a drive assumption into behaviour. A bare designator is
//! **navigation grammar**, never an executable name and never rewritten into a
//! full path (`D:foo` is *not* turned into `D:\foo`).

/// Returns `Some(drive_letter)` when `s` is a bare Windows drive designator
/// (`D:`, `d:`) — exactly one ASCII letter followed by `:`.
///
/// Does **not** match `D:\`, `D:foo`, `CD:`, or any longer token.
pub fn is_drive_designator(s: &str) -> Option<char> {
    let b = s.as_bytes();
    if b.len() == 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        Some(b[0].to_ascii_uppercase() as char)
    } else {
        None
    }
}

/// Returns `true` when `s` starts with a Windows drive prefix (`D:`, `D:\`,
/// `D:foo`, `D:\path`).
pub fn is_drive_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_designator_matches() {
        assert_eq!(is_drive_designator("D:"), Some('D'));
        assert_eq!(is_drive_designator("d:"), Some('D'));
    }

    #[test]
    fn longer_tokens_do_not_match() {
        assert_eq!(is_drive_designator("D:\\"), None);
        assert_eq!(is_drive_designator("D:foo"), None);
        assert_eq!(is_drive_designator("CD:"), None);
        assert_eq!(is_drive_designator("git"), None);
    }

    #[test]
    fn drive_path_covers_prefix_forms() {
        assert!(is_drive_path("D:"));
        assert!(is_drive_path("D:\\"));
        assert!(is_drive_path("D:foo"));
        assert!(!is_drive_path("git"));
    }
}
