use omen_interactive::{GrammarScanner, InputLane, InteractiveSession, TypedReference};

#[test]
fn test_grammar_lane_scanning() {
    // 1. Ordinary executable lane
    let exec = GrammarScanner::scan("cargo test --all").unwrap();
    assert_eq!(
        exec,
        InputLane::Executable {
            argv: vec!["cargo".into(), "test".into(), "--all".into()]
        }
    );

    // 2. Executable with quotes
    let exec_quotes =
        GrammarScanner::scan(r#"git commit -m "initial commit" 'another arg'"#).unwrap();
    assert_eq!(
        exec_quotes,
        InputLane::Executable {
            argv: vec![
                "git".into(),
                "commit".into(),
                "-m".into(),
                "initial commit".into(),
                "another arg".into(),
            ]
        }
    );

    // 3. Semantic action lane
    let action = GrammarScanner::scan(":test auth --release").unwrap();
    assert_eq!(
        action,
        InputLane::SemanticAction {
            action: "test".into(),
            args: vec!["auth".into(), "--release".into()]
        }
    );

    // 4. Semantic action without args
    let status_action = GrammarScanner::scan(":status").unwrap();
    assert_eq!(
        status_action,
        InputLane::SemanticAction {
            action: "status".into(),
            args: vec![]
        }
    );

    // 5. AI reasoning lane
    let ai = GrammarScanner::scan("? why does the auth test fail on token refresh?").unwrap();
    assert_eq!(
        ai,
        InputLane::AiReasoning {
            query: "why does the auth test fail on token refresh?".into()
        }
    );
}

#[test]
fn test_portable_shell_lane_preserves_plain_argv_and_parses_compounds() {
    assert!(matches!(
        GrammarScanner::scan("printf 'a|b'").unwrap(),
        InputLane::Executable { .. }
    ));

    assert!(matches!(
        GrammarScanner::scan("echo a | cat").unwrap(),
        InputLane::PortableShell { .. }
    ));
    assert_eq!(
        GrammarScanner::scan("echo a |& cat").unwrap_err().code(),
        omen_core::ErrorCode::Unsupported
    );

    let InputLane::PortableShell { line } = GrammarScanner::scan("a && b; c").unwrap() else {
        panic!("compound shell expression must use the portable shell lane");
    };
    assert_eq!(line.items.len(), 2);
    let omen_interactive::shell_grammar::ShellSequence::BooleanChain { first, rest } =
        &line.items[0].sequence;
    assert_eq!(
        first.commands[0].words[0].parts,
        [omen_interactive::shell_grammar::ShellWordPart::Text(
            "a".into()
        )]
    );
    assert_eq!(rest.len(), 1);
    assert_eq!(
        rest[0].0,
        omen_interactive::shell_grammar::ShellBooleanOperator::And
    );
    let omen_interactive::shell_grammar::ShellSequence::BooleanChain { first, rest } =
        &line.items[1].sequence;
    assert_eq!(
        first.commands[0].words[0].parts,
        [omen_interactive::shell_grammar::ShellWordPart::Text(
            "c".into()
        )]
    );
    assert!(rest.is_empty());
}

#[test]
fn portable_shell_boolean_chains_update_session_directory() {
    let workspace = tempfile::tempdir().unwrap();
    let nested = workspace.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    let workspace = workspace.path().canonicalize().unwrap();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut session = InteractiveSession::new_with_client(
        omen_core::InteractiveSessionId::generate(),
        workspace.clone(),
        None,
        None,
    )
    .unwrap();

    let semicolon = session.dispatch_input("cd nested; cd ..").unwrap();
    assert!(semicolon.is_zero());
    assert_eq!(session.cwd, workspace);

    let skipped = session.dispatch_input("cd missing && cd nested").unwrap();
    assert!(!skipped.is_zero());
    assert_eq!(session.cwd, workspace);

    let fallback = session.dispatch_input("cd missing || cd nested").unwrap();
    assert!(fallback.is_zero());
    assert_eq!(session.cwd, nested.canonicalize().unwrap());
}

#[test]
fn test_typed_reference_parsing() {
    assert_eq!(TypedReference::parse("@last"), Some(TypedReference::Last));
    assert_eq!(
        TypedReference::parse("@last.failed"),
        Some(TypedReference::LastFailed)
    );
    assert_eq!(
        TypedReference::parse("@last.artifact"),
        Some(TypedReference::LastArtifact)
    );
    assert_eq!(
        TypedReference::parse("@last.changed"),
        Some(TypedReference::LastChanged)
    );
    assert_eq!(
        TypedReference::parse("@last.output"),
        Some(TypedReference::LastOutput)
    );
    assert_eq!(
        TypedReference::parse("@failed"),
        Some(TypedReference::Failed)
    );
    assert_eq!(
        TypedReference::parse("@errors"),
        Some(TypedReference::Errors)
    );
    assert_eq!(
        TypedReference::parse("@fact.test"),
        Some(TypedReference::Fact("test".into()))
    );
    assert_eq!(
        TypedReference::parse("@service.dev"),
        Some(TypedReference::Service("dev".into()))
    );
    assert_eq!(
        TypedReference::parse("@custom_thing"),
        Some(TypedReference::Other("custom_thing".into()))
    );
    assert_eq!(TypedReference::parse("not_a_reference"), None);
}
