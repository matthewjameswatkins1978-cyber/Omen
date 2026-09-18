use omen_interactive::InteractiveSession;
use tempfile::tempdir;

#[test]
fn test_two_sessions_in_same_cwd_have_different_ids() {
    let dir = tempdir().unwrap();
    let cwd = dir.path().to_path_buf();

    let sess1 = InteractiveSession::new(cwd.clone(), None).unwrap();
    let sess2 = InteractiveSession::new(cwd.clone(), None).unwrap();

    assert_ne!(
        sess1.session_id, sess2.session_id,
        "Two interactive sessions launched in the same cwd must have distinct unique session IDs"
    );
    assert!(
        sess1.session_id.as_str().starts_with("sess-"),
        "Session ID must have 'sess-' prefix"
    );
    assert!(
        sess2.session_id.as_str().starts_with("sess-"),
        "Session ID must have 'sess-' prefix"
    );
}
