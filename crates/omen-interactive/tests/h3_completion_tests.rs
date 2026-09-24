use omen_core::{Assurance, ResourceUri, ValidityState};
use omen_interactive::commands;
use omen_interactive::completion::{
    CompletionContext, HotSemanticIndex, OmenCompleter, OmenHinter,
};
use omen_knowledge::{Database, FactRegistry, PublishFactRequest};
use reedline::Hinter;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

fn ctx_with_commands(cmds: &[&str]) -> Arc<Mutex<CompletionContext>> {
    let mut hot_index = HotSemanticIndex::default();
    hot_index.update_path_commands(cmds.iter().map(|s| s.to_string()).collect());
    Arc::new(Mutex::new(CompletionContext {
        cwd: tempdir().unwrap().path().to_path_buf(),
        hot_index,
    }))
}

#[test]
fn test_completion_semantic_actions() {
    let ctx = Arc::new(Mutex::new(CompletionContext::default()));
    let mut completer = OmenCompleter::new(ctx);

    let suggestions = completer.complete_items(":doc", 4);
    assert!(!suggestions.is_empty());
    assert_eq!(suggestions[0].value, ":doctor");

    let suggestions = completer.complete_items(":stat", 5);
    assert!(!suggestions.is_empty());
    assert_eq!(suggestions[0].value, ":status");
}

#[test]
fn test_completion_typed_references() {
    let ctx = Arc::new(Mutex::new(CompletionContext::default()));
    let mut completer = OmenCompleter::new(ctx);

    let suggestions = completer.complete_items("@fa", 3);
    assert!(!suggestions.is_empty());
    assert!(suggestions.iter().any(|s| s.value == "@failed"));

    let suggestions = completer.complete_items("@last.", 6);
    assert!(!suggestions.is_empty());
    assert!(suggestions.iter().any(|s| s.value == "@last.failed"));
    assert!(suggestions.iter().any(|s| s.value == "@last.artifact"));
}

#[test]
fn test_completion_known_tools_and_subcommands() {
    let ctx = ctx_with_commands(&["cargo", "git", "threadmoth", "ripgrep"]);
    let mut completer = OmenCompleter::new(ctx);

    // Initial word -> PATH command cache
    let suggestions = completer.complete_items("car", 3);
    assert!(!suggestions.is_empty());
    assert_eq!(suggestions[0].value, "cargo");

    let suggestions = completer.complete_items("thread", 6);
    assert!(!suggestions.is_empty());
    assert_eq!(suggestions[0].value, "threadmoth");

    // Subcommands
    let suggestions = completer.complete_items("cargo te", 8);
    assert!(!suggestions.is_empty());
    assert_eq!(suggestions[0].value, "test");

    let suggestions = completer.complete_items("git st", 6);
    assert!(!suggestions.is_empty());
    assert_eq!(suggestions[0].value, "status");

    let suggestions = completer.complete_items("threadmoth mut", 14);
    assert!(!suggestions.is_empty());
    assert_eq!(suggestions[0].value, "mutate");
}

#[test]
fn test_fact_aware_ranking_elevates_dirty_facts() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let mut db = Database::open(&db_path).unwrap();

    let res_a = ResourceUri::parse("fact://git/status").unwrap();
    let res_b = ResourceUri::parse("fact://git/branch").unwrap();

    FactRegistry::set_generation(&mut db, "git:head", 1).unwrap();

    FactRegistry::publish_fact(
        &mut db,
        PublishFactRequest {
            resource: &res_a,
            value: "clean",
            assurance: Assurance::Observed,
            producer: "git",
            witness: None,
            dependencies: &[],
            artifacts: &[],
        },
    )
    .unwrap();

    FactRegistry::publish_fact(
        &mut db,
        PublishFactRequest {
            resource: &res_b,
            value: "main",
            assurance: Assurance::Observed,
            producer: "git",
            witness: None,
            dependencies: &[("git:head".into(), 1)],
            artifacts: &[],
        },
    )
    .unwrap();

    FactRegistry::increment_generation(&mut db, "git:head").unwrap();

    let fact_b = FactRegistry::get_fact(&db, &res_b, false).unwrap();
    assert_eq!(fact_b.validity, ValidityState::Dirty);

    let mut hot_index = HotSemanticIndex::default();
    hot_index.refresh(dir.path(), Some(&db));
    let ctx = Arc::new(Mutex::new(CompletionContext {
        cwd: dir.path().to_path_buf(),
        hot_index,
    }));
    let mut completer = OmenCompleter::new(ctx);

    // Canonical fact references are `@fact.<name>` per TypedReference authority.
    let suggestions = completer.complete_items("@fact.git", 9);
    assert!(!suggestions.is_empty());

    let dirty_opt = suggestions.iter().find(|s| s.value.contains("git/branch"));
    assert!(dirty_opt.is_some(), "got: {suggestions:?}");
    assert!(
        dirty_opt
            .unwrap()
            .description
            .as_ref()
            .unwrap()
            .contains("Dirty")
    );
}

#[test]
fn test_hinter_provides_inline_suggestion() {
    let ctx = Arc::new(Mutex::new(CompletionContext::default()));
    let completer = Arc::new(Mutex::new(OmenCompleter::new(ctx)));
    let mut hinter = OmenHinter::new(completer, omen_interactive::interaction::new_readiness());

    let hist_dir = tempdir().unwrap();
    let hist_file = hist_dir.path().join("history.txt");
    let history = reedline::FileBackedHistory::with_file(10, hist_file).unwrap();

    let hint = hinter.handle(":doc", 4, &history, false, ".");
    assert_eq!(hint, "tor");
}

#[test]
fn test_completion_structural_bounds_not_wall_clock() {
    // Structural performance properties instead of fragile CI latency asserts.
    let ctx = ctx_with_commands(&["cargo", "git"]);
    let mut completer = OmenCompleter::new(ctx);

    let suggestions = completer.complete_items("cargo te", 8);
    assert!(!suggestions.is_empty());
    assert!(
        suggestions.len() <= omen_interactive::completion::bounds::MAX_FINAL_CANDIDATES,
        "candidate set must be bounded"
    );

    // Empty and malformed input must degrade to no candidates, never panic.
    for (line, pos) in [
        ("", 0),
        ("\"unclosed", 9),
        ("'also unclosed", 14),
        ("cat \"", 5),
        ("\\", 1),
        ("\"\\", 2),
        (":", 1),
        ("@", 1),
    ] {
        let _ = completer.complete_items(line, pos);
    }
}

#[test]
fn test_omen_actions_authority_is_shared_with_dispatcher() {
    use omen_core::InteractiveSessionId;
    use omen_interactive::SemanticDispatcher;

    // Frozen regression guard: the shared authority must stay aligned with the
    // dispatcher's match arms.
    let expected: Vec<&str> = vec![
        "actions",
        "agent",
        "backend",
        "capabilities",
        "def",
        "describe",
        "doctor",
        "history",
        "how",
        "inspect",
        "orient",
        "packages",
        "plan",
        "refs",
        "rerun",
        "services",
        "show",
        "status",
        "stop",
        "structure",
        "symbol",
        "tasks",
        "tools",
        "why",
    ];
    assert_eq!(commands::OMEN_ACTIONS, expected.as_slice());

    // The unknown-action diagnostic is generated from the same authority.
    let msg = commands::unknown_action_message("nope");
    for action in commands::OMEN_ACTIONS {
        assert!(commands::is_omen_action(action));
        assert!(
            msg.contains(&format!(":{action}")),
            "missing {action} in {msg}"
        );
    }
    assert!(msg.contains("Unknown Omen semantic action ':nope'"));

    let dir = tempdir().unwrap();
    let session_id = InteractiveSessionId::generate();

    // Every declared action must dispatch (usage errors may exit non-zero;
    // the unknown-action arm is what must never fire for declared names).
    for action in commands::OMEN_ACTIONS {
        let exit =
            SemanticDispatcher::dispatch(action, &[], dir.path(), &session_id, None, None, None)
                .unwrap();
        assert!(
            exit.code == Some(0) || exit.code == Some(1),
            "action '{action}' must dispatch through a known arm"
        );
    }

    // An unlisted name must be rejected via the shared authority message.
    let exit = SemanticDispatcher::dispatch(
        "definitely_not_an_action",
        &[],
        dir.path(),
        &session_id,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(exit.code, Some(1));
}
