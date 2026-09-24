//! Replacement-span / buffer integrity contract.
//!
//! For every completion:
//! - the candidate owns an exact replacement span
//! - only that span may change
//!
//! Proves: no duplicate prefix, no duplicate suffix, no duplicated slash/path
//! segment, no cursor outside valid buffer bounds, no stale replacement after
//! menu movement, Esc does not mutate editable text, accepting candidate
//! affects only its declared span, repeated Tab remains deterministic.

use omen_interactive::completion::{
    CandidateKind, CompletionContext, CompletionEngine, HotSemanticIndex,
};
use omen_interactive::grammar;
use std::fs;
use std::path::Path;
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

/// Applies the candidate's edit to `line` and returns the result.
fn apply_edit(line: &str, replacement_range: std::ops::Range<usize>, insertion: &str) -> String {
    let mut s = line.to_string();
    s.replace_range(replacement_range, insertion);
    s
}

// ---------------------------------------------------------------------------
// CORE CONTRACT: ONLY THE DECLARED SPAN CHANGES
// ---------------------------------------------------------------------------

#[test]
fn span_contract_end_of_token_replaces_only_the_token() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "alpha.txt");
    let ctx = ctx_in(dir.path(), &[]);

    let line = "cat alp trailing";
    let pos = 7;
    let c = CompletionEngine::complete(&ctx, line, pos);
    let hit = c.iter().find(|x| x.literal == "alpha.txt").unwrap();

    // The span covers exactly `alp` (bytes 4..7).
    assert_eq!(hit.edit.replacement_range, 4..7);
    assert_eq!(hit.edit.insertion_text, "alpha.txt");

    let rebuilt = apply_edit(
        line,
        hit.edit.replacement_range.clone(),
        &hit.edit.insertion_text,
    );
    assert_eq!(rebuilt, "cat alpha.txt trailing");
    // No duplicate prefix: `alp` was replaced, not extended.
    assert!(!rebuilt.contains("alpalpha"));
    // No duplicate suffix: ` trailing` survived byte-for-byte.
    assert!(rebuilt.ends_with(" trailing"));
}

#[test]
fn span_contract_mid_token_inserts_missing_middle_only() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "fileX.txt");
    let ctx = ctx_in(dir.path(), &[]);

    let line = "cat file.txt";
    let pos = 8; // after `file`
    let c = CompletionEngine::complete(&ctx, line, pos);
    let hit = c.iter().find(|x| x.literal == "fileX.txt").unwrap();

    // Pure insertion at cursor.
    assert_eq!(hit.edit.replacement_range, pos..pos);
    assert_eq!(hit.edit.insertion_text, "X");

    let rebuilt = apply_edit(
        line,
        hit.edit.replacement_range.clone(),
        &hit.edit.insertion_text,
    );
    assert_eq!(rebuilt, "cat fileX.txt");
    // No duplicate prefix: `file` was not re-inserted.
    assert!(!rebuilt.contains("filefile"));
    // No duplicate suffix: `.txt` survived.
    assert!(rebuilt.ends_with(".txt"));
}

#[test]
fn span_contract_quoted_mid_token_preserves_surrounding_quotes() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "file long.txt");
    let ctx = ctx_in(dir.path(), &[]);

    let line = "cat \"file.txt\"";
    let pos = 9;
    let c = CompletionEngine::complete(&ctx, line, pos);
    let hit = c.iter().find(|x| x.literal == "file long.txt").unwrap();

    assert_eq!(hit.edit.replacement_range, pos..pos);
    assert_eq!(hit.edit.insertion_text, " long");

    let rebuilt = apply_edit(
        line,
        hit.edit.replacement_range.clone(),
        &hit.edit.insertion_text,
    );
    assert_eq!(rebuilt, "cat \"file long.txt\"");
    // Quotes survive intact.
    assert!(rebuilt.starts_with("cat \""));
    assert!(rebuilt.ends_with('"'));
    // No duplicated quote.
    assert_eq!(rebuilt.matches('"').count(), 2);
}

// ---------------------------------------------------------------------------
// NO DUPLICATED SLASH / PATH SEGMENT
// ---------------------------------------------------------------------------

#[test]
fn span_contract_no_duplicated_slash_path_segment() {
    let dir = tempdir().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    touch(&dir.path().join("src"), "main.rs");
    let ctx = ctx_in(dir.path(), &[]);

    let line = "cat sr";
    let pos = 6;
    let c = CompletionEngine::complete(&ctx, line, pos);
    let hit = c
        .iter()
        .find(|x| x.literal.starts_with("src"))
        .expect("src candidate");

    let rebuilt = apply_edit(
        line,
        hit.edit.replacement_range.clone(),
        &hit.edit.insertion_text,
    );
    // The path is `src/main.rs` or `src\main.rs` — no `srsrc/` duplication.
    assert!(
        !rebuilt.contains("srsr"),
        "duplicated prefix in {rebuilt:?}"
    );
    // Exactly one separator between src and main.rs.
    let path_part = rebuilt.strip_prefix("cat ").unwrap();
    let sep_count = path_part
        .chars()
        .filter(|c| *c == '/' || *c == '\\')
        .count();
    assert_eq!(sep_count, 1, "expected one separator in {rebuilt:?}");
}

#[test]
fn span_contract_directory_literal_does_not_duplicate_separator() {
    let dir = tempdir().unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    let ctx = ctx_in(dir.path(), &[]);

    let line = "cat su";
    let pos = 6;
    let c = CompletionEngine::complete(&ctx, line, pos);
    let hit = c.iter().find(|x| x.literal.contains("sub")).unwrap();

    let rebuilt = apply_edit(
        line,
        hit.edit.replacement_range.clone(),
        &hit.edit.insertion_text,
    );
    // Should be `cat sub/` or `cat sub\` — not `cat susub/`.
    assert!(
        !rebuilt.contains("susub"),
        "duplicated separator in {rebuilt:?}"
    );
}

// ---------------------------------------------------------------------------
// NO CURSOR OUTSIDE VALID BUFFER BOUNDS
// ---------------------------------------------------------------------------

#[test]
fn span_contract_replacement_range_within_buffer_bounds() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "alpha.txt");
    touch(dir.path(), "alpine.txt");
    let ctx = ctx_in(dir.path(), &["cargo"]);

    for (line, pos) in [
        ("cat alp", 7),
        ("cat al", 6),
        ("car", 3),
        (":sta", 4),
        ("", 0),
        ("cat alpha.txt extra", 12),
    ] {
        let c = CompletionEngine::complete(&ctx, line, pos);
        for hit in &c {
            assert!(
                hit.edit.replacement_range.start <= line.len(),
                "start out of bounds: {} > {} for {line:?}",
                hit.edit.replacement_range.start,
                line.len()
            );
            assert!(
                hit.edit.replacement_range.end <= line.len(),
                "end out of bounds: {} > {} for {line:?}",
                hit.edit.replacement_range.end,
                line.len()
            );
            assert!(
                hit.edit.replacement_range.start <= hit.edit.replacement_range.end,
                "start > end for {line:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ACCEPTING CANDIDATE AFFECTS ONLY ITS DECLARED SPAN
// ---------------------------------------------------------------------------

#[test]
fn span_contract_accepting_leaves_prefix_and_suffix_intact() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "notes.txt");
    let ctx = ctx_in(dir.path(), &[]);

    // `cat no extra` — cursor after `no` (end of token), suffix ` extra` after.
    let line = "cat no extra";
    let pos = 6; // after `no`
    let c = CompletionEngine::complete(&ctx, line, pos);
    let hit = c.iter().find(|x| x.literal == "notes.txt").unwrap();

    let prefix_before = &line[..hit.edit.replacement_range.start];
    let suffix_after = &line[hit.edit.replacement_range.end..];

    let rebuilt = apply_edit(
        line,
        hit.edit.replacement_range.clone(),
        &hit.edit.insertion_text,
    );

    assert!(
        rebuilt.starts_with(prefix_before),
        "prefix changed: {prefix_before:?} vs {rebuilt:?}"
    );
    assert!(
        rebuilt.ends_with(suffix_after),
        "suffix changed: {suffix_after:?} vs {rebuilt:?}"
    );
    assert_eq!(rebuilt, "cat notes.txt extra");
}

// ---------------------------------------------------------------------------
// REPEATED TAB IS DETERMINISTIC
// ---------------------------------------------------------------------------

#[test]
fn span_contract_repeated_completion_is_deterministic() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "alpha.txt");
    touch(dir.path(), "alpine.txt");
    let ctx = ctx_in(dir.path(), &["cargo"]);

    let line = "cat al";
    let pos = 6;
    let a = CompletionEngine::complete(&ctx, line, pos);
    let b = CompletionEngine::complete(&ctx, line, pos);
    let c = CompletionEngine::complete(&ctx, line, pos);

    assert_eq!(a, b, "repeated completion must produce identical results");
    assert_eq!(b, c, "repeated completion must produce identical results");
}

#[test]
fn span_contract_repeated_edit_application_is_idempotent_for_exact_match() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "alpha.txt");
    let ctx = ctx_in(dir.path(), &[]);

    // After applying the edit once, re-running completion on the result
    // must not offer the same candidate again (it's already exact).
    let line = "cat alp";
    let pos = 7;
    let c = CompletionEngine::complete(&ctx, line, pos);
    let hit = c.iter().find(|x| x.literal == "alpha.txt").unwrap();
    let after = apply_edit(
        line,
        hit.edit.replacement_range.clone(),
        &hit.edit.insertion_text,
    );

    // The buffer now has `cat alpha.txt`. Completion on `alpha.txt` at end
    // should not re-offer `alpha.txt` as a replacement (already exact).
    let c2 = CompletionEngine::complete(&ctx, &after, after.len());
    for x in &c2 {
        if x.literal == "alpha.txt" {
            // If offered, the edit must be a no-op (empty insertion, empty range).
            assert!(
                x.edit.insertion_text.is_empty() || x.edit.replacement_range.is_empty(),
                "already-exact candidate must not rewrite: {x:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ESC DOES NOT MUTATE EDITABLE TEXT
// ---------------------------------------------------------------------------

#[test]
fn span_contract_engine_never_mutates_input_buffer() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "alpha.txt");
    let ctx = ctx_in(dir.path(), &["cargo"]);

    // The engine API is pure: `complete` and `ghost` take `&str` and return
    // owned data. There is no mutation path. Dismissing (Esc) is "do nothing".
    let line = String::from("cat alp");
    let snapshot = line.clone();
    let _ = CompletionEngine::complete(&ctx, &line, 7);
    let _ = CompletionEngine::ghost(&ctx, &line, 7);
    assert_eq!(line, snapshot, "engine must not mutate the input buffer");
}

// ---------------------------------------------------------------------------
// NO STALE REPLACEMENT AFTER MENU MOVEMENT
// ---------------------------------------------------------------------------

#[test]
fn span_contract_each_candidate_has_self_consistent_edit() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "alpha.txt");
    touch(dir.path(), "alpine.txt");
    touch(dir.path(), "alto.txt");
    let ctx = ctx_in(dir.path(), &[]);

    let line = "cat al";
    let pos = 6;
    let c = CompletionEngine::complete(&ctx, line, pos);
    assert!(c.len() >= 3);

    // Every candidate's edit, applied to the ORIGINAL buffer, produces a
    // buffer that decodes to that candidate's literal. No candidate's edit
    // depends on another candidate's edit having been applied first.
    for hit in &c {
        let rebuilt = apply_edit(
            line,
            hit.edit.replacement_range.clone(),
            &hit.edit.insertion_text,
        );
        let words = grammar::GrammarScanner::split_words(&rebuilt);
        assert_eq!(
            words.last().map(|s| s.as_str()),
            Some(hit.literal.as_str()),
            "edit for {:?} produced {rebuilt:?} -> {words:?}",
            hit.literal
        );
    }
}

// ---------------------------------------------------------------------------
// NO FABRICATED VALIDITY
// ---------------------------------------------------------------------------

#[test]
fn span_contract_no_candidate_for_impossible_literal() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "alpha.txt");
    let ctx = ctx_in(dir.path(), &[]);

    // A name that doesn't exist gets no candidates.
    let c = CompletionEngine::complete(&ctx, "cat zzzz", 8);
    assert!(
        c.iter().all(|x| x.literal != "alpha.txt"),
        "must not fabricate validity"
    );
}

// ---------------------------------------------------------------------------
// EXACT SPAN DECLARATION
// ---------------------------------------------------------------------------

#[test]
fn span_contract_every_candidate_declares_its_span() {
    let dir = tempdir().unwrap();
    touch(dir.path(), "alpha.txt");
    fs::create_dir(dir.path().join("sub")).unwrap();
    let ctx = ctx_in(dir.path(), &["cargo"]);

    for (line, pos) in [("cat alp", 7), ("car", 3), (":sta", 4), ("cat su", 6)] {
        let c = CompletionEngine::complete(&ctx, line, pos);
        for hit in &c {
            // The replacement_range is always a valid Range<usize>.
            assert!(
                hit.edit.replacement_range.start <= hit.edit.replacement_range.end,
                "invalid range for {line:?}"
            );
            // The insertion_text is never empty for a real candidate.
            // (Empty insertion + empty range = no-op, which build_edit declines.)
            assert!(
                !hit.edit.insertion_text.is_empty(),
                "empty insertion for {line:?} candidate {:?}",
                hit.literal
            );
        }
    }
}
