use omen_interactive::{GrammarScanner, InputLane, TypedReference};

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
