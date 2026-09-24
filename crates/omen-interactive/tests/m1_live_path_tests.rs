//! M1 live-path proofs: the real session's completer IS the M1 discovery
//! authority.
//!
//! These tests fail if the live path silently reverts to an M0-era candidate
//! source:
//!
//! - the feature proofs exercise **M1-only capabilities** (structured tool
//!   options, subcommand descriptions) that the M0 `CompletionEngine`
//!   `gather_sources` pipeline could never produce;
//! - the session-structure proof asserts `session.rs` still builds its editor
//!   on `OmenCompleter` + `interaction::create_line_editor`;
//! - the async seam proofs drive the real Reedline
//!   `Fresh`/`Stale`/`Pending` + `poll_completion` lifecycle end-to-end.

use omen_interactive::completion::{
    CompletionContext, CompletionReadiness, HotSemanticIndex, OmenCompleter,
};
use omen_interactive::interaction;
use reedline::{Completer, CompletionResult, CompletionStatus};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

/// Builds a completer exactly the way `session.rs` does.
fn live_completer(path_commands: &[&str]) -> OmenCompleter {
    let mut hot = HotSemanticIndex::default();
    hot.update_path_commands(path_commands.iter().map(|s| s.to_string()).collect());
    OmenCompleter::new(Arc::new(Mutex::new(CompletionContext {
        cwd: std::env::current_dir().unwrap_or_else(|_| ".".into()),
        hot_index: hot,
    })))
}

/// Bounded pump over the real Reedline seam: poll until work settles.
fn settle(completer: &mut OmenCompleter) -> CompletionStatus {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match completer.poll_completion() {
            CompletionStatus::Ready => return CompletionStatus::Ready,
            // Nothing in flight any more (settled-and-adopted, or declined).
            CompletionStatus::Idle => return CompletionStatus::Idle,
            CompletionStatus::Pending => {
                if std::time::Instant::now() > deadline {
                    return CompletionStatus::Pending;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// THE M1 AUTHORITY GUARD
// ---------------------------------------------------------------------------

#[test]
fn live_completer_uses_m1_discovery_not_m0_completion_engine() {
    // `git --<Tab>` structured options exist ONLY in M1 (tool-spec +
    // HelpHarvest providers). The M0 `CompletionEngine::gather_sources`
    // pipeline offered no option candidates at all — if the live completer
    // ever routes back to it, this proof fails.
    let mut c = live_completer(&["git"]);
    let result = c.complete("git --", 6);
    let suggestions = result.suggestions();
    assert!(
        !suggestions.is_empty(),
        "M1 must surface structured git options on the live path"
    );
    let verbose = suggestions
        .iter()
        .find(|s| s.value == "--verbose")
        .expect("M1 tool-spec option --verbose must reach the live completer");
    assert!(
        verbose
            .description
            .as_deref()
            .is_some_and(|d| d.contains("be more verbose")),
        "descriptions come from real M1 authority: {:?}",
        verbose.description
    );
    assert!(suggestions.iter().all(|s| s.value.starts_with("--")));
}

#[test]
fn session_source_builds_the_m1_backed_completer() {
    // The live REPL must construct OmenCompleter and the ONE interaction
    // model — not an independent candidate engine.
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/session.rs"))
        .expect("session.rs readable");
    assert!(
        src.contains("OmenCompleter::new"),
        "session.rs must build its editor on the M1-backed OmenCompleter"
    );
    assert!(
        src.contains("create_line_editor"),
        "session.rs must wire the ONE interaction model"
    );
}

// ---------------------------------------------------------------------------
// REAL REEDLINE ASYNC SEAM (Fresh / Stale / Pending + poll_completion)
// ---------------------------------------------------------------------------

#[test]
fn inline_request_returns_fresh_without_background_work() {
    let mut c = live_completer(&["cargo"]);
    let result = c.complete("car", 3);
    assert!(
        matches!(result, CompletionResult::Fresh { .. }),
        "command-name position has no async applicability => Fresh"
    );
    assert_eq!(result.suggestions().len(), 1);
    assert_eq!(result.suggestions()[0].value, "cargo");
    assert_eq!(c.poll_completion(), CompletionStatus::Idle);
}

#[test]
fn async_context_reports_provisional_then_settles_via_poll() {
    // `git st` is an ExecArg context: the TIER 1B filesystem provider is
    // dispatched out-of-band. The first answer is provisional (Stale) with
    // inline truth; poll_completion reports Pending then Ready; the settled
    // answer is Fresh.
    let mut c = live_completer(&["git"]);
    let first = c.complete("git st", 6);
    assert!(
        first.is_provisional(),
        "async context must answer Stale/Pending, never block: {first:?}"
    );
    assert!(
        first.suggestions().iter().any(|s| s.value == "status"),
        "inline truth is shown while computing: {:?}",
        first.suggestions()
    );
    // Background work is in flight (or already finished — a local readdir is
    // fast); it must never report Idle while the request is outstanding.
    let status = c.poll_completion();
    assert_ne!(
        status,
        CompletionStatus::Idle,
        "async request must be visible to poll_completion"
    );

    assert_eq!(
        settle(&mut c),
        CompletionStatus::Ready,
        "poll reports Ready"
    );

    let settled = c.complete("git st", 6);
    assert!(
        matches!(settled, CompletionResult::Fresh { .. }),
        "origin unchanged => adopted settled truth is Fresh"
    );
    assert!(
        settled.suggestions().iter().any(|s| s.value == "status"),
        "settled answer keeps the semantic truth"
    );
}

#[test]
fn filesystem_case_completes_through_the_live_async_seam() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("special.txt"), b"x").unwrap();
    let mut c = OmenCompleter::new(Arc::new(Mutex::new(CompletionContext {
        cwd: dir.path().to_path_buf(),
        hot_index: HotSemanticIndex::default(),
    })));

    // Request begins: provisional (TIER 1B filesystem dispatched), no editor
    // blocking.
    let first = c.complete("cat spe", 7);
    assert!(first.is_provisional(), "filesystem work runs out-of-band");

    // Provider completes later; poll reports Ready; candidate appears.
    assert_eq!(settle(&mut c), CompletionStatus::Ready);
    let settled = c.complete("cat spe", 7);
    assert!(
        matches!(settled, CompletionResult::Fresh { .. }),
        "settled filesystem result is Fresh"
    );
    let hit = settled
        .suggestions()
        .iter()
        .find(|s| s.value.contains("special.txt"))
        .unwrap_or_else(|| panic!("got: {:?}", settled.suggestions()));
    // Exact replacement span over the typed token only.
    assert_eq!(hit.span.start, 4);
    assert_eq!(hit.span.end, 7);
}

// ---------------------------------------------------------------------------
// HEADLINE FLOWS THROUGH THE LIVE PATH
// ---------------------------------------------------------------------------

#[test]
fn live_flow_b_cargo_subcommands_with_authoritative_descriptions() {
    let mut c = live_completer(&["cargo"]);
    let items = c.complete_items("cargo ", 6);
    let values: Vec<&str> = items.iter().map(|s| s.value.as_str()).collect();
    assert!(values.contains(&"build"), "got: {values:?}");
    assert!(values.contains(&"test"), "got: {values:?}");
    let test = items.iter().find(|s| s.value == "test").unwrap();
    assert!(
        test.description
            .as_deref()
            .is_some_and(|d| d.contains("run tests")),
        "description from the M1 tool spec: {:?}",
        test.description
    );
}

#[test]
fn live_flow_d_action_surface_comes_from_canonical_truth() {
    // Current canonical truth (H2 not merged). Re-prove `omen authority <Tab>`
    // after H2 reconciliation.
    let mut c = live_completer(&[]);
    let items = c.complete_items(":sta", 4);
    assert_eq!(items[0].value, ":status");
    let all = c.complete_items(":", 1);
    for s in &all {
        let name = s.value.trim_start_matches(':');
        assert!(
            omen_interactive::commands::OMEN_ACTIONS.contains(&name),
            "{} must come from the shared authority",
            s.value
        );
    }
}

// ---------------------------------------------------------------------------
// PENDING VS ZERO + GHOST/ TAB COHERENCE (live paths)
// ---------------------------------------------------------------------------

#[test]
fn live_readiness_distinguishes_final_zero_from_pending() {
    // True zero: command-name gibberish, no async applicability.
    let mut c = live_completer(&["cargo"]);
    assert_eq!(
        c.readiness("zzzznonexistent", 15),
        CompletionReadiness::FinalZero
    );

    // Pending: option position on an unknown tool — zero inline candidates,
    // but TIER 2 harvest applies. Tab must NOT be swallowed as a proven zero.
    let pending = c.readiness("unknown_tool --xyz", 18);
    assert!(
        matches!(pending, CompletionReadiness::Pending { inline: 0 }),
        "zero inline + triggered async work is Pending, got {pending:?}"
    );
    // Readiness never dispatched anything (repaint path is read-only).
    assert_eq!(c.poll_completion(), CompletionStatus::Idle);

    // Ready: inline truth exists.
    assert!(matches!(
        c.readiness("car", 3),
        CompletionReadiness::Ready(n) if n >= 1
    ));
}

#[test]
fn live_ghost_never_invents_candidates_absent_from_m1_truth() {
    let mut c = live_completer(&["cargo", "card", "carbon"]);
    // Ambiguous: no ghost, and the underlying candidate set still exists.
    assert_eq!(c.ghost_view_hint("car", 3), None);
    let candidates = c.complete_typed("car", 3);
    assert!(candidates.len() >= 3);

    // Unique: ghost equals the top M1 candidate — nothing invented.
    let mut c2 = live_completer(&["cargo"]);
    let ghost = c2.ghost_view("car", 3).expect("unique prefix ghosts");
    let candidates = c2.complete_typed("car", 3);
    assert_eq!(ghost.literal, "cargo");
    assert!(
        candidates.iter().any(|x| x.literal == ghost.literal),
        "ghost candidate must exist in M1 truth"
    );
    let (hint, _) = c2.ghost_view_hint("car", 3).unwrap();
    assert_eq!(hint, "go");
}

#[test]
fn live_tab_gate_and_ghost_agree_on_candidate_validity() {
    let mut c = live_completer(&["cargo"]);
    // A candidate exists => readiness is not FinalZero.
    let ready = c.readiness("car", 3);
    assert!(matches!(ready, CompletionReadiness::Ready(_)));
    // And the completer's completion path agrees.
    let result = c.complete("car", 3);
    assert!(!result.suggestions().is_empty());
    // Zero candidate => both agree on zero.
    assert_eq!(c.readiness("zzzznope", 8), CompletionReadiness::FinalZero);
    assert!(c.complete("zzzznope", 8).suggestions().is_empty());
}

#[test]
fn live_tab_event_mapping_matches_readiness() {
    // The pure gate function the real OmenEditMode delegates to.
    assert!(matches!(
        interaction::tab_event_for_readiness(CompletionReadiness::FinalZero),
        reedline::ReedlineEvent::None
    ));
    assert!(matches!(
        interaction::tab_event_for_readiness(CompletionReadiness::Pending { inline: 0 }),
        reedline::ReedlineEvent::UntilFound(_)
    ));
    assert!(matches!(
        interaction::tab_event_for_readiness(CompletionReadiness::Ready(2)),
        reedline::ReedlineEvent::UntilFound(_)
    ));
}

// ---------------------------------------------------------------------------
// EDITOR-THREAD ECONOMY (semantic properties, not wall-clock)
// ---------------------------------------------------------------------------

#[test]
fn repaint_paths_never_dispatch_background_work() {
    // The hinter/count/ghost paths observe only: repeated repaints leave the
    // scheduler idle even in async-triggering contexts.
    let mut c = live_completer(&["git"]);
    for _ in 0..5 {
        let _ = c.complete_typed("git st", 6);
        let _ = c.readiness("unknown_tool --xyz", 18);
        let _ = c.ghost_view("car", 3);
    }
    assert_eq!(
        c.poll_completion(),
        CompletionStatus::Idle,
        "observe paths must never queue background work"
    );
}
