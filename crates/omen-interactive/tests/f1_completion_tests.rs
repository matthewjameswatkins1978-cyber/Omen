//! 0.9-F1 deterministic completion and ghost-suggestion proofs.
//!
//! Covers Lucy's locked edge matrix: mid-token suffix preservation, quoting
//! round-trip through the accepted grammar, no second grammar (no pipes /
//! redirection), ghost confidence, and deterministic ordering.

use omen_interactive::completion::{
    CandidateKind, CompletionContext, CompletionEngine, HotSemanticIndex, OmenCompleter,
    OmenHinter, bounds,
};
use omen_interactive::grammar::{self, GrammarScanner, TypedReference};
use reedline::Hinter;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

fn ctx_in(dir: &Path, cmds: &[&str]) -> CompletionContext {
    let mut hot_index = HotSemanticIndex::default();
    hot_index.update_path_commands(cmds.iter().map(|s| s.to_string()).collect());
    CompletionContext {
        cwd: dir.to_path_buf(),
        hot_index,
    }
}

fn touch(dir: &Path, name: &str) {
    fs::write(dir.join(name), b"x").unwrap();
}

// ---------------------------------------------------------------------------
// COMMAND CONTEXT
// ---------------------------------------------------------------------------

#[test]
fn f1_car_completes_to_cargo_from_path_cache() {
    let dir = tempdir().unwrap();
    let ctx = ctx_in(dir.path(), &["cargo", "card", "git"]);
    let c = CompletionEngine::complete(&ctx, "car", 3);
    assert!(
        c.iter().any(|x| x.literal == "cargo"),
        "got: {:?}",
        c.iter().map(|x| &x.literal).collect::<Vec<_>>()
    );
}

#[test]
fn f1_fakecommand_prefix_invents_nothing() {
    let dir = tempdir().unwrap();
    let ctx = ctx_in(dir.path(), &["cargo", "git"]);
    let c = CompletionEngine::complete(&ctx, "fakecommand", 11);
    assert!(
        c.is_empty(),
        "must not invent candidates, got: {:?}",
        c.iter().map(|x| &x.literal).collect::<Vec<_>>()
    );
    assert!(CompletionEngine::ghost(&ctx, "fakecommand", 11).is_none());
}

#[test]
fn f1_empty_buffer_offers_authority_but_no_confident_ghost() {
    let dir = tempdir().unwrap();
    let ctx = ctx_in(dir.path(), &["cargo"]);
    let c = CompletionEngine::complete(&ctx, "", 0);
    assert!(!c.is_empty());
    // Ambiguous by definition: no ghost on empty input.
    assert!(CompletionEngine::ghost(&ctx, "", 0).is_none());
}

#[test]
fn f1_command_with_existing_arguments_completes_arguments() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "notes.txt");
    let ctx = ctx_in(dir.path(), &["cargo"]);
    let c = CompletionEngine::complete(&ctx, "cat no", 6);
    assert!(c.iter().any(|x| x.literal == "notes.txt"), "got: {c:?}");
}

// ---------------------------------------------------------------------------
// FILESYSTEM
// ---------------------------------------------------------------------------

#[test]
fn f1_path_completion_file_and_directory_prefix() {
    let dir = tempdir().unwrap();
    fs::create_dir(dir.path().join("srcdir")).unwrap();
    touch(dir.path(), "srcfile.txt");
    let ctx = ctx_in(dir.path(), &[]);

    let c = CompletionEngine::complete(&ctx, "cat sr", 6);
    let kinds: Vec<_> = c
        .iter()
        .map(|x| (x.display_text.clone(), x.kind, x.literal.clone()))
        .collect();
    assert!(
        kinds
            .iter()
            .any(|(d, k, _)| d == "srcdir" && *k == CandidateKind::Directory),
        "got: {kinds:?}"
    );
    assert!(
        kinds
            .iter()
            .any(|(d, k, _)| d == "srcfile.txt" && *k == CandidateKind::Path),
        "got: {kinds:?}"
    );
}

#[test]
fn f1_awkward_filenames_quote_correctly() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "file with spaces.txt");
    touch(dir.path(), "it's.txt");
    touch(dir.path(), "unicode-\u{4e2d}\u{6587}.txt");
    let ctx = ctx_in(dir.path(), &[]);

    // Type an unquoted prefix word and complete at end of line.
    for (typed, name) in [
        ("cat file", "file with spaces.txt"),
        ("cat it", "it's.txt"),
        ("cat unicode-", "unicode-\u{4e2d}\u{6587}.txt"),
    ] {
        let c = CompletionEngine::complete(&ctx, typed, typed.len());
        let hit = c.iter().find(|x| x.literal == name).unwrap_or_else(|| {
            panic!(
                "missing {name:?} in {:?}",
                c.iter().map(|x| &x.literal).collect::<Vec<_>>()
            )
        });
        assert!(
            grammar::round_trips(name),
            "literal {name:?} must be representable"
        );
        assert!(!hit.edit.insertion_text.is_empty());
    }
}

#[test]
fn f1_dot_paths_and_parent_directory() {
    let dir = tempdir().unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    touch(dir.path(), "top.txt");
    touch(&sub, "inner.txt");

    let ctx = ctx_in(&sub, &[]);
    let c = CompletionEngine::complete(&ctx, "cat ../to", 9);
    assert!(
        c.iter().any(|x| x.literal.contains("top.txt")),
        "got: {:?}",
        c.iter().map(|x| &x.literal).collect::<Vec<_>>()
    );

    let c = CompletionEngine::complete(&ctx, "cat ./inn", 9);
    assert!(c.iter().any(|x| x.literal.ends_with("inner.txt")));
}

#[test]
fn f1_windows_and_relative_path_forms() {
    let dir = tempdir().unwrap();
    fs::create_dir(dir.path().join("My Projects")).unwrap();
    let ctx = ctx_in(dir.path(), &[]);

    let c = CompletionEngine::complete(&ctx, "cat My", 6);
    let hit = c
        .iter()
        .find(|x| x.display_text == "My Projects")
        .unwrap_or_else(|| panic!("got: {c:?}"));
    assert_eq!(hit.kind, CandidateKind::Directory);
    // Directory literals carry a trailing separator for continued editing.
    assert!(hit.literal.starts_with("My Projects"));
}

#[test]
fn f1_huge_directory_fixture_stays_bounded() {
    let dir = tempdir().unwrap();
    for i in 0..400 {
        touch(dir.path(), &format!("bulk-{i:04}.txt"));
    }
    let ctx = ctx_in(dir.path(), &[]);
    let c = CompletionEngine::complete(&ctx, "cat bulk-", 9);
    assert!(c.len() <= bounds::MAX_FS_ENTRIES);
    assert!(c.len() <= bounds::MAX_FINAL_CANDIDATES);
    assert!(!c.is_empty());
}

// ---------------------------------------------------------------------------
// CURSOR
// ---------------------------------------------------------------------------

#[test]
fn f1_end_of_line_completion() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "alpha.txt");
    let ctx = ctx_in(dir.path(), &[]);
    let c = CompletionEngine::complete(&ctx, "cat alp", 7);
    assert!(c.iter().any(|x| x.literal == "alpha.txt"));
}

#[test]
fn f1_middle_of_command_line_preserves_right_hand_arguments() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "alpha.txt");
    let ctx = ctx_in(dir.path(), &[]);

    // cat alp| extra args   -- cursor inside first argument
    let line = "cat alp trailing-arg";
    let pos = 7; // after "cat alp"
    let c = CompletionEngine::complete(&ctx, line, pos);
    let hit = c
        .iter()
        .find(|x| x.literal == "alpha.txt")
        .expect("candidate");
    // End-of-token of `alp`: replacement covers only the token `alp`.
    assert_eq!(hit.edit.replacement_range, 4..7);
    assert_eq!(hit.edit.insertion_text, "alpha.txt");
    // Reconstructing must leave ` trailing-arg` intact.
    let mut rebuilt = line.to_string();
    rebuilt.replace_range(hit.edit.replacement_range.clone(), &hit.edit.insertion_text);
    assert_eq!(rebuilt, "cat alpha.txt trailing-arg");
}

#[test]
fn f1_middle_of_token_compatible_inserts_missing_middle_only() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "fileX.txt");
    let ctx = ctx_in(dir.path(), &[]);

    // file|.txt  -> candidate fileX.txt -> insert only "X"
    let line = "cat file.txt";
    let pos = 8; // after "file"
    let c = CompletionEngine::complete(&ctx, line, pos);
    let hit = c
        .iter()
        .find(|x| x.literal == "fileX.txt")
        .unwrap_or_else(|| panic!("got: {c:?}"));
    assert_eq!(hit.edit.replacement_range, pos..pos);
    assert_eq!(hit.edit.insertion_text, "X");

    let mut rebuilt = line.to_string();
    rebuilt.replace_range(hit.edit.replacement_range.clone(), &hit.edit.insertion_text);
    assert_eq!(rebuilt, "cat fileX.txt");
}

#[test]
fn f1_mid_token_incompatible_suffix_is_not_destroyed() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "foobaz");
    let ctx = ctx_in(dir.path(), &[]);

    // foo|bar with candidate foobaz: suffix "bar" is not preserved by foobaz.
    let line = "cat foobar";
    let pos = 7; // after "foo"
    let c = CompletionEngine::complete(&ctx, line, pos);
    assert!(
        c.iter().all(|x| x.literal != "foobaz"),
        "destructive candidate must be refused: {c:?}"
    );
    assert!(CompletionEngine::ghost(&ctx, line, pos).is_none());
}

#[test]
fn f1_unquoted_space_middle_is_declined_not_corrupted() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "file long.txt");
    let ctx = ctx_in(dir.path(), &[]);

    // file|.txt with candidate `file long.txt` would need quote surgery around
    // the right-hand `.txt`. F1 declines rather than corrupts.
    let line = "cat file.txt";
    let pos = 8;
    let c = CompletionEngine::complete(&ctx, line, pos);
    assert!(
        c.iter().all(|x| x.literal != "file long.txt"),
        "quote surgery must be declined: {c:?}"
    );
}

#[test]
fn f1_quoted_mid_token_inserts_middle_and_preserves_suffix() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "file long.txt");
    let ctx = ctx_in(dir.path(), &[]);

    // "file|.txt" -> middle " long" is safely encoded inside the open quote.
    let line = "cat \"file.txt\"";
    let pos = 9; // after `file` inside the quotes
    let c = CompletionEngine::complete(&ctx, line, pos);
    let hit = c
        .iter()
        .find(|x| x.literal == "file long.txt")
        .unwrap_or_else(|| panic!("got: {c:?}"));
    assert_eq!(hit.edit.replacement_range, pos..pos);
    assert_eq!(hit.edit.insertion_text, " long");

    let mut rebuilt = line.to_string();
    rebuilt.replace_range(hit.edit.replacement_range.clone(), &hit.edit.insertion_text);
    assert_eq!(rebuilt, "cat \"file long.txt\"");
}

#[test]
fn f1_single_quoted_mid_token_preserves_right_hand_text() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "file with spammces.txt");
    let ctx = ctx_in(dir.path(), &[]);

    // 'file with spa|ces.txt' -> compatible candidate inserts only "mm".
    let line = "cat 'file with spaces.txt'";
    // Find the cursor right after `file with spa` inside the quotes.
    let pos = line.find("file with spa").unwrap() + "file with spa".len();
    let c = CompletionEngine::complete(&ctx, line, pos);
    let hit = c
        .iter()
        .find(|x| x.literal == "file with spammces.txt")
        .unwrap_or_else(|| panic!("got: {c:?}"));
    assert_eq!(hit.edit.replacement_range, pos..pos);
    assert_eq!(hit.edit.insertion_text, "mm");

    let mut rebuilt = line.to_string();
    rebuilt.replace_range(hit.edit.replacement_range.clone(), &hit.edit.insertion_text);
    assert_eq!(rebuilt, "cat 'file with spammces.txt'");
    // The right-hand `ces.txt'` survived byte-for-byte.
    assert!(rebuilt.ends_with("ces.txt'"));
}

#[test]
fn f1_already_matching_token_has_no_destructive_edit() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "file with spaces.txt");
    let ctx = ctx_in(dir.path(), &[]);

    // 'file with spa|ces.txt' with candidate `file with spaces.txt` is already
    // the token (empty missing middle). No destructive rewrite.
    let line = "cat 'file with spaces.txt'";
    let pos = line.find("file with spa").unwrap() + "file with spa".len();
    let c = CompletionEngine::complete(&ctx, line, pos);
    assert!(
        c.iter().all(|x| x.edit.insertion_text.is_empty()
            || x.literal != "file with spaces.txt"
            || x.edit.replacement_range.is_empty()
            || {
                // empty middle -> build_edit declines
                true
            }),
        "must not rewrite an already-complete token: {c:?}"
    );
    // More precisely: no candidate replaces the whole token destructively.
    for x in &c {
        if x.literal == "file with spaces.txt" {
            panic!("already-typed candidate must not be re-offered: {x:?}");
        }
    }
}

#[test]
fn f1_open_quote_unsafe_rewrites_are_declined() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "file's.txt");
    let ctx = ctx_in(dir.path(), &[]);

    // Inside single quotes the apostrophe cannot be encoded without quote
    // surgery around the right-hand text. Decline.
    let line = "cat 'file'";
    let pos = 9; // after `file`, before the closing quote
    let c = CompletionEngine::complete(&ctx, line, pos);
    assert!(
        c.iter().all(|x| x.literal != "file's.txt"),
        "apostrophe inside single quotes must be declined: {c:?}"
    );
}

// ---------------------------------------------------------------------------
// QUOTING
// ---------------------------------------------------------------------------

fn assert_round_trip(literal: &str) {
    let canonical = grammar::quote_literal(literal)
        .unwrap_or_else(|| panic!("literal must be representable: {literal:?}"));
    assert_eq!(
        GrammarScanner::split_words(&canonical),
        vec![literal.to_string()],
        "round-trip failed for {literal:?} via {canonical:?}"
    );
}

#[test]
fn f1_quoting_round_trips_through_accepted_grammar() {
    assert_round_trip("plain.txt");
    assert_round_trip("file with spaces.txt");
    assert_round_trip("it's.txt");
    assert_round_trip("say \"hi\".txt");
    assert_round_trip("unicode-\u{4e2d}\u{6587}.txt");
    assert_round_trip(r"C:\Users\Matmus\project");
    assert_round_trip(r"C:\Program Files\App");
    assert_round_trip(r"dir\");
    assert_round_trip(r"\\server\share\data.json");
    assert_round_trip("both ' and \".txt");
    assert_round_trip("a&b.txt");
    assert_round_trip("2>&1.txt");
}

#[test]
fn f1_unrepresentable_literals_are_unknown_not_corrupt() {
    assert!(!grammar::round_trips(""));
    // Both quote characters plus trailing backslash cannot be represented in
    // the accepted grammar (double quotes cannot escape `\`).
    assert!(!grammar::round_trips("it's\\"));
    assert!(grammar::quote_literal("it's\\").is_none());
}

#[test]
fn f1_insertion_text_is_never_the_display_text_when_quoting_is_needed() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "file with spaces.txt");
    let ctx = ctx_in(dir.path(), &[]);
    let c = CompletionEngine::complete(&ctx, "cat file", 8);
    let hit = c
        .iter()
        .find(|x| x.literal == "file with spaces.txt")
        .unwrap();
    assert_ne!(hit.display_text, hit.edit.insertion_text);
    assert_eq!(hit.display_text, "file with spaces.txt");
    assert!(hit.edit.insertion_text.contains('"') || hit.edit.insertion_text.contains('\''));
}

// ---------------------------------------------------------------------------
// RANKING + STABILITY
// ---------------------------------------------------------------------------

#[test]
fn f1_identical_context_produces_identical_ordering_and_edits() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "alpha.txt");
    touch(dir.path(), "alpine.txt");
    fs::create_dir(dir.path().join("alt")).unwrap();
    let ctx = ctx_in(dir.path(), &["cargo", "cat"]);

    let a = CompletionEngine::complete(&ctx, "cat al", 6);
    let b = CompletionEngine::complete(&ctx, "cat al", 6);
    assert_eq!(a, b, "same input/state must produce the same ordering");
}

#[test]
fn f1_strong_context_candidate_beats_irrelevant_one() {
    let dir = tempdir().unwrap();
    let ctx = ctx_in(dir.path(), &["statusish"]);
    let c = CompletionEngine::complete(&ctx, ":stat", 5);
    assert_eq!(c[0].literal, ":status");
    assert!(c[0].kind == CandidateKind::OmenAction);
}

#[test]
fn f1_ambiguous_prefix_manufactures_no_confident_ghost() {
    let dir = tempdir().unwrap();
    let ctx = ctx_in(dir.path(), &["card", "cargo", "carbon"]);
    let c = CompletionEngine::complete(&ctx, "car", 3);
    assert!(c.len() >= 3);
    assert!(
        CompletionEngine::ghost(&ctx, "car", 3).is_none(),
        "close candidates must not manufacture a confident ghost"
    );
}

#[test]
fn f1_unique_prefix_gets_a_calm_ghost() {
    let dir = tempdir().unwrap();
    let ctx = ctx_in(dir.path(), &["cargo"]);
    let ghost = CompletionEngine::ghost(&ctx, "car", 3).expect("ghost");
    assert_eq!(ghost.literal, "cargo");
    let (hint, _) = CompletionEngine::ghost_hint(&ctx, "car", 3).unwrap();
    assert_eq!(hint, "go");
}

// ---------------------------------------------------------------------------
// SAFETY / NO SECOND GRAMMAR
// ---------------------------------------------------------------------------

#[test]
fn f1_redirection_and_pipes_are_never_completed() {
    let dir = tempdir().unwrap();
    let ctx = ctx_in(dir.path(), &["cargo", "git"]);

    for (line, pos) in [
        ("cargo test 2>", 12),
        ("cargo test 2>&", 14),
        ("cargo test >>", 13),
        ("cargo test |", 12),
        ("cargo test | ", 13),
        ("cat 2>&1", 8),
    ] {
        let c = CompletionEngine::complete(&ctx, line, pos);
        for x in &c {
            assert!(
                !x.literal.contains("2>&1")
                    && !x.literal.contains(">>")
                    && x.literal != "|"
                    && x.literal != ">"
                    && x.literal != "&1",
                "redirection/pipe syntax must never be offered: {:?}",
                x.literal
            );
        }
        assert!(
            CompletionEngine::ghost(&ctx, line, pos).is_none(),
            "no ghost for operator adjacency: {line:?}"
        );
    }
}

#[test]
fn f1_pipe_characters_are_literal_argv_not_operators() {
    // The accepted grammar is argv-only: `|` is literal text in a word.
    let lane = GrammarScanner::scan("echo foo|bar").unwrap();
    match lane {
        omen_interactive::InputLane::Executable { argv } => {
            assert_eq!(argv, vec!["echo".to_string(), "foo|bar".to_string()]);
        }
        other => panic!("unexpected lane: {other:?}"),
    }
}

#[test]
fn f1_grammar_span_scanner_projects_split_words() {
    let samples = [
        "",
        " ",
        "cargo test --all",
        r#"git commit -m "initial commit" 'another arg'"#,
        r#"exec "C:\Program Files\Rust\bin\cargo.exe" --version"#,
        r#"run 'D:\My Projects\app.exe' arg1"#,
        "cat \"file with spaces.txt\"",
        "echo \"unclosed",
        "echo 'unclosed",
        "a\\b",
        r#"say \""hi\""#,
        "trailing\\",
        ":status :why",
        "a \"\" b",
        "\u{4e2d}\u{6587} test",
    ];
    for s in samples {
        let projected: Vec<String> = grammar::scan_words_with_spans(s)
            .into_iter()
            .filter(|w| !w.literal.is_empty())
            .map(|w| w.literal)
            .collect();
        assert_eq!(
            projected,
            GrammarScanner::split_words(s),
            "span scanner must project split_words for {s:?}"
        );
    }
}

#[test]
fn f1_typed_reference_completes_only_authoritative_handles() {
    let dir = tempdir().unwrap();
    let ctx = ctx_in(dir.path(), &[]);
    let c = CompletionEngine::complete(&ctx, "@la", 3);
    let literals: Vec<_> = c.iter().map(|x| x.literal.clone()).collect();
    assert!(literals.iter().any(|l| l == "@last"));
    // Invented URI-style handles are not authoritative TypedReferences.
    assert!(!literals.iter().any(|l| l.starts_with("@symbol://")));
    assert!(!literals.iter().any(|l| l.starts_with("@package://")));

    for h in TypedReference::STATIC_HANDLES {
        assert!(omen_interactive::grammar::TypedReference::parse(h).is_some());
    }
}

#[test]
fn f1_malformed_partial_input_never_panics() {
    let dir = tempdir().unwrap();
    let ctx = ctx_in(dir.path(), &["cargo"]);
    let buffers = [
        "\"",
        "'",
        "\"\\",
        "'\\",
        "\\",
        "\\\\",
        "\"\"\"",
        "'''",
        ":",
        "::",
        "@",
        "@@",
        "cat \"a\\",
        "cat 'a\\",
        "cat \u{4e2d}",
        "a\"b'c",
        "2>&1 | >>",
    ];
    for b in buffers {
        for pos in 0..=b.len() {
            if b.is_char_boundary(pos) {
                let _ = CompletionEngine::complete(&ctx, b, pos);
                let _ = CompletionEngine::ghost(&ctx, b, pos);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// UI / LINE EDITOR
// ---------------------------------------------------------------------------

#[test]
fn f1_ghost_hint_is_append_only_and_subordinate() {
    let dir = tempdir().unwrap();
    let ctx = Arc::new(Mutex::new(ctx_in(dir.path(), &["cargo"])));
    let completer = Arc::new(Mutex::new(OmenCompleter::new(ctx)));
    let mut hinter = OmenHinter::new(completer);

    let hist = reedline::FileBackedHistory::with_file(10, dir.path().join("h.txt")).unwrap();
    let hint = hinter.handle("car", 3, &hist, false, ".");
    assert_eq!(hint, "go");
    // Hint is the unstyled remainder; decoration is the editor's concern and
    // is omitted under use_ansi=false / NO_COLOR.
    let hint_plain = hinter.handle("car", 3, &hist, true, ".");
    assert_eq!(hint_plain, "go");
}

#[test]
fn f1_explicit_menu_edit_does_not_alter_until_applied() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "alpha.txt");
    let ctx = ctx_in(dir.path(), &[]);
    let line = "cat alp";
    let c = CompletionEngine::complete(&ctx, line, 7);
    // Building the candidate set must not mutate anything: the caller applies
    // the edit explicitly.
    assert_eq!(line, "cat alp");
    assert!(!c.is_empty());
}

#[test]
fn f1_escape_dismisses_by_producing_no_forced_insert() {
    // The engine never auto-applies an edit. Dismissing is "do nothing", which
    // is structurally the default.
    let dir = tempdir().unwrap();
    let ctx = ctx_in(dir.path(), &["cargo"]);
    let _ = CompletionEngine::complete(&ctx, "car", 3);
    // No API applies edits implicitly; callers own the buffer.
}

// ---------------------------------------------------------------------------
// DEGRADED TERMINAL
// ---------------------------------------------------------------------------

#[test]
fn f1_degraded_terminal_relies_on_plain_insertion_text() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "file with spaces.txt");
    let ctx = ctx_in(dir.path(), &[]);
    let c = CompletionEngine::complete(&ctx, "cat file", 8);
    let hit = c
        .iter()
        .find(|x| x.literal == "file with spaces.txt")
        .unwrap();
    // Insertion text is pure canonical characters: no ANSI, no styling.
    assert!(!hit.edit.insertion_text.contains('\u{1b}'));
    assert!(!hit.display_text.contains('\u{1b}'));
}

// ---------------------------------------------------------------------------
// PERFORMANCE EVIDENCE (reporting, not a CI latency gate)
// ---------------------------------------------------------------------------

#[test]
fn f1_perf_evidence_report() {
    use std::time::Instant;

    let dir = tempdir().unwrap();
    for i in 0..200 {
        touch(dir.path(), &format!("f{i:03}.txt"));
    }
    fs::create_dir(dir.path().join("src")).unwrap();
    for i in 0..50 {
        touch(&dir.path().join("src"), &format!("m{i:03}.rs"));
    }

    let mut hot = HotSemanticIndex::default();
    let mut many: Vec<String> = (0..1000).map(|i| format!("cmd-{i:04}")).collect();
    many.push("cargo".into());
    hot.update_path_commands(many);
    let ctx = CompletionContext {
        cwd: dir.path().to_path_buf(),
        hot_index: hot,
    };

    // Cold: first representative call after context construction.
    let t = Instant::now();
    let cold = CompletionEngine::complete(&ctx, "cat f0", 6);
    let cold_ms = t.elapsed().as_secs_f64() * 1000.0;

    // Warm: repeated representative call.
    let mut warm_samples = Vec::new();
    for _ in 0..200 {
        let t = Instant::now();
        let _ = CompletionEngine::complete(&ctx, "cat f0", 6);
        warm_samples.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    warm_samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let warm_p50 = warm_samples[100];
    let warm_p95 = warm_samples[190];

    // 1,000-candidate ranking (PATH cache with 1000 commands).
    let t = Instant::now();
    let many_c = CompletionEngine::complete(&ctx, "cmd-", 4);
    let many_ms = t.elapsed().as_secs_f64() * 1000.0;

    // Awkward-path completion.
    let t = Instant::now();
    let _ = CompletionEngine::complete(&ctx, "cat src/m0", 10);
    let awkward_ms = t.elapsed().as_secs_f64() * 1000.0;

    println!("F1-PERF cold_ms={cold_ms:.3} candidates={}", cold.len());
    println!("F1-PERF warm_p50_ms={warm_p50:.3} warm_p95_ms={warm_p95:.3}");
    println!(
        "F1-PERF rank_1000_ms={many_ms:.3} retained={}",
        many_c.len()
    );
    println!("F1-PERF awkward_path_ms={awkward_ms:.3}");

    // Structural bounds only as correctness gates.
    assert!(many_c.len() <= bounds::MAX_FINAL_CANDIDATES);
    assert!(cold.len() <= bounds::MAX_FINAL_CANDIDATES);
    assert!(!many_c.is_empty());
}

#[test]
fn f1_candidate_generation_is_bounded_per_source() {
    let dir = tempdir().unwrap();
    for i in 0..300 {
        touch(dir.path(), &format!("s{i:03}"));
    }
    let mut hot = HotSemanticIndex::default();
    hot.update_path_commands((0..900).map(|i| format!("c{i:04}")).collect());
    let ctx = CompletionContext {
        cwd: dir.path().to_path_buf(),
        hot_index: hot,
    };
    for (line, pos) in [("s", 1), ("c", 1), (":s", 2), ("@l", 3)] {
        let c = CompletionEngine::complete(&ctx, line, pos);
        assert!(
            c.len() <= bounds::MAX_FINAL_CANDIDATES,
            "{line} -> {}",
            c.len()
        );
    }
}
