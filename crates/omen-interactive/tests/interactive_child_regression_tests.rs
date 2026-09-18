use omen_core::InteractiveSessionId;
use omen_interactive::InteractiveSession;
use omen_interactive::child::{ChildClassification, ChildHandoff};
use omen_knowledge::{Database, ExecutionHistory};
use tempfile::tempdir;

#[test]
fn test_interactive_child_classification_matrix() {
    // 1. Dedicated interactive tools
    assert_eq!(
        ChildHandoff::classify(&["vim".into(), "src/main.rs".into()]),
        ChildClassification::InteractiveHandoff
    );
    assert_eq!(
        ChildHandoff::classify(&["nano".into(), "README.md".into()]),
        ChildClassification::InteractiveHandoff
    );
    assert_eq!(
        ChildHandoff::classify(&["less".into(), "output.log".into()]),
        ChildClassification::InteractiveHandoff
    );
    assert_eq!(
        ChildHandoff::classify(&["htop".into()]),
        ChildClassification::InteractiveHandoff
    );

    // 2. Interpreters: interactive without args, supervised with script args
    assert_eq!(
        ChildHandoff::classify(&["python".into()]),
        ChildClassification::InteractiveHandoff
    );
    assert_eq!(
        ChildHandoff::classify(&["python3".into(), "-i".into()]),
        ChildClassification::InteractiveHandoff
    );
    assert_eq!(
        ChildHandoff::classify(&["python".into(), "train.py".into()]),
        ChildClassification::StandardSupervised
    );
    assert_eq!(
        ChildHandoff::classify(&["python".into(), "-c".into(), "print(1)".into()]),
        ChildClassification::StandardSupervised
    );

    assert_eq!(
        ChildHandoff::classify(&["node".into()]),
        ChildClassification::InteractiveHandoff
    );
    assert_eq!(
        ChildHandoff::classify(&["node".into(), "index.js".into()]),
        ChildClassification::StandardSupervised
    );
    assert_eq!(
        ChildHandoff::classify(&["node".into(), "-e".into(), "console.log(1)".into()]),
        ChildClassification::StandardSupervised
    );

    // 3. Git invocation semantics
    // git commit without -m opens interactive editor
    assert_eq!(
        ChildHandoff::classify(&["git".into(), "commit".into()]),
        ChildClassification::InteractiveHandoff
    );
    // git commit with -m is supervised
    assert_eq!(
        ChildHandoff::classify(&[
            "git".into(),
            "commit".into(),
            "-m".into(),
            "feat: foo".into()
        ]),
        ChildClassification::StandardSupervised
    );
    // git rebase -i is interactive
    assert_eq!(
        ChildHandoff::classify(&["git".into(), "rebase".into(), "-i".into(), "HEAD~3".into()]),
        ChildClassification::InteractiveHandoff
    );
    // git add -p is interactive
    assert_eq!(
        ChildHandoff::classify(&["git".into(), "add".into(), "-p".into()]),
        ChildClassification::InteractiveHandoff
    );

    // 4. Truthful explicit override
    assert_eq!(
        ChildHandoff::classify(&["--interactive".into(), "custom-tool".into()]),
        ChildClassification::InteractiveHandoff
    );
    assert_eq!(
        ChildHandoff::classify(&["--handoff".into(), "custom-tool".into()]),
        ChildClassification::InteractiveHandoff
    );

    // 5. Default supervised
    assert_eq!(
        ChildHandoff::classify(&["cargo".into(), "test".into()]),
        ChildClassification::StandardSupervised
    );
    assert_eq!(
        ChildHandoff::classify(&["ripgrep".into(), "pattern".into()]),
        ChildClassification::StandardSupervised
    );
}

#[test]
fn test_interactive_children_record_execution_history() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("child_history.db");
    let mut db = Database::open(&db_path).unwrap();

    let session_id = InteractiveSessionId::new("sess-child-proof").unwrap();
    ExecutionHistory::register_session(&mut db, &session_id, "human", ".").unwrap();

    let mut session = InteractiveSession::new_with_session_id(
        session_id.clone(),
        dir.path().to_path_buf(),
        Some(db),
    )
    .unwrap();

    let (test_cmd, expected_sub) = if cfg!(windows) {
        ("--interactive cmd /c exit 0", "cmd /c exit 0")
    } else {
        ("--interactive sh -c 'exit 0'", "sh -c 'exit 0'")
    };

    let exit = session.dispatch_input(test_cmd).unwrap();
    assert_eq!(exit.code, Some(0));

    // Check that execution history in SQLite recorded this interactive execution
    let hist =
        ExecutionHistory::list_session_executions(session.db.as_ref().unwrap(), &session_id, 10)
            .unwrap();
    assert!(
        !hist.is_empty(),
        "Interactive execution must produce subordinate execution history"
    );
    assert!(
        hist[0].command.contains(expected_sub),
        "Execution history must preserve interactive command: {}",
        hist[0].command
    );
    assert_eq!(hist[0].exit_code, Some(0));
    assert!(hist[0].duration_ms.is_some());
}
