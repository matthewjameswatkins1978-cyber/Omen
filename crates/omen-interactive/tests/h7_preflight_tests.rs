use omen_interactive::InteractiveSession;
use omen_interactive::preflight::{BlastPreflight, BlastSeverity, PasteGuard};
use tempfile::tempdir;

#[test]
fn test_blast_preflight_destructive_detection() {
    // 1. rm -rf
    let argv_rm = vec!["rm".into(), "-rf".into(), "dir".into()];
    let blast_rm = BlastPreflight::assess(&argv_rm);
    assert!(blast_rm.is_some());
    let rep = blast_rm.unwrap();
    assert_eq!(rep.severity, BlastSeverity::High);
    assert!(rep.summary.contains("File deletion"));

    // 2. cargo clean
    let argv_clean = vec!["cargo".into(), "clean".into()];
    let blast_clean = BlastPreflight::assess(&argv_clean);
    assert!(blast_clean.is_some());
    let rep = blast_clean.unwrap();
    assert_eq!(rep.severity, BlastSeverity::Medium);
    assert!(rep.summary.contains("Purging Cargo"));

    // 3. git reset --hard
    let argv_git_reset = vec!["git".into(), "reset".into(), "--hard".into()];
    let blast_reset = BlastPreflight::assess(&argv_git_reset);
    assert!(blast_reset.is_some());
    let rep = blast_reset.unwrap();
    assert_eq!(rep.severity, BlastSeverity::High);
    assert!(rep.summary.contains("Hard reset"));

    // 4. threadmoth mutate
    let argv_tm = vec![
        "threadmoth".into(),
        "mutate".into(),
        "--request".into(),
        "req.json".into(),
    ];
    let blast_tm = BlastPreflight::assess(&argv_tm);
    assert!(blast_tm.is_some());
    let rep = blast_tm.unwrap();
    assert_eq!(rep.severity, BlastSeverity::Medium);
    assert!(rep.summary.contains("ThreadMoth"));

    // 5. Non-destructive commands must NOT trigger preflight warnings
    assert!(BlastPreflight::assess(&["cargo".into(), "test".into()]).is_none());
    assert!(BlastPreflight::assess(&["git".into(), "status".into()]).is_none());
    assert!(BlastPreflight::assess(&["ls".into(), "-la".into()]).is_none());
}

#[test]
fn test_paste_guard_multiline_interception() {
    // Single-line -> None
    let single = "cargo test";
    assert!(PasteGuard::inspect(single).is_none());

    // Multiline -> Intercepted
    let multiline = "git add .\ngit commit -m \"msg\"\ngit push";
    let review = PasteGuard::inspect(multiline);
    assert!(review.is_some());
    let rev = review.unwrap();
    assert_eq!(rev.line_count, 3);
    assert_eq!(rev.lines[0], "git add .");
    assert_eq!(rev.lines[1], "git commit -m \"msg\"");
    assert_eq!(rev.lines[2], "git push");
}

#[test]
fn test_session_paste_guard_dispatch() {
    let temp = tempdir().unwrap();
    let mut session = InteractiveSession::new(temp.path().to_path_buf(), None).unwrap();

    let multiline = "echo first\necho second\n";
    let exit = session.dispatch_input(multiline).unwrap();
    // Intercepted safely without error
    assert!(exit.is_zero());
}
