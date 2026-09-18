use omen_core::{Assurance, ResourceUri, ValidityState};
use omen_interactive::completion::{CompletionContext, OmenCompleter, OmenHinter};
use omen_knowledge::{Database, FactRegistry, PublishFactRequest};
use reedline::Hinter;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tempfile::tempdir;

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
    let ctx = Arc::new(Mutex::new(CompletionContext::default()));
    let mut completer = OmenCompleter::new(ctx);

    // Initial word -> tool
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

    // Publish two facts: fact A (will stay current) and fact B (will become dirty)
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

    // Increment git:head to make res_b DIRTY
    FactRegistry::increment_generation(&mut db, "git:head").unwrap();

    let fact_b = FactRegistry::get_fact(&db, &res_b, false).unwrap();
    assert_eq!(fact_b.validity, ValidityState::Dirty);

    let mut hot_index = omen_interactive::completion::HotSemanticIndex::default();
    hot_index.refresh(dir.path(), Some(&db));
    let ctx = Arc::new(Mutex::new(CompletionContext {
        cwd: dir.path().to_path_buf(),
        hot_index,
    }));
    let mut completer = OmenCompleter::new(ctx);

    // Query @git
    let suggestions = completer.complete_items("@git", 4);
    assert!(!suggestions.is_empty());

    // Because res_b is DIRTY, it should have a boost applied in candidates
    let dirty_opt = suggestions.iter().find(|s| s.value.contains("git/branch"));
    assert!(dirty_opt.is_some());
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
    let mut hinter = OmenHinter::new(completer);

    let hist_dir = tempdir().unwrap();
    let hist_file = hist_dir.path().join("history.txt");
    let history = reedline::FileBackedHistory::with_file(10, hist_file).unwrap();

    let hint = hinter.handle(":doc", 4, &history, false, ".");
    assert_eq!(hint, "tor");
}

#[test]
fn test_completion_performance_under_10ms() {
    let ctx = Arc::new(Mutex::new(CompletionContext::default()));
    let mut completer = OmenCompleter::new(ctx);

    let start = Instant::now();
    for _ in 0..100 {
        let _ = completer.complete_items("cargo te", 8);
    }
    let elapsed = start.elapsed();
    let per_op = elapsed / 100;
    println!("Avg completion time: {:?}", per_op);
    assert!(
        per_op.as_millis() < 10,
        "Completion took too long: {:?}",
        per_op
    );
}
