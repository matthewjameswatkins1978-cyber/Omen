//! ONE human interaction layer.
//!
//! All candidate classes (Omen actions, shell intrinsics, PATH commands,
//! filesystem paths) feed ONE typed candidate model which is presented through
//! ONE Reedline completion menu.  Tab is Omen's discovery key.
//!
//! M0 interaction grammar:
//!
//! ```text
//! Tab        discover / complete / open chooser
//! Up / Down  browse chooser ONLY when chooser active
//! Enter      accept selected candidate when chooser active
//! Esc        dismiss chooser, buffer unchanged
//! ```
//!
//! When the chooser is not active, normal editing remains normal.
//! Editing outranks assistance: no ordinary editing key is overridden.

use crate::completion::{OmenCompleter, OmenHinter};
use reedline::{
    ColumnarMenu, Completer, CompletionResult, DefaultValidator, EditMode, Emacs, KeyCode,
    KeyModifiers, MenuBuilder, PromptEditMode, Reedline, ReedlineEvent, ReedlineMenu,
    ReedlineRawEvent, default_emacs_keybindings,
};
use std::sync::{Arc, Mutex};

/// The name of the single completion menu used for every candidate class.
pub const COMPLETION_MENU_NAME: &str = "completion_menu";

/// Shared candidate count consulted by [`OmenEditMode`] on every Tab.
///
/// Updated by [`OmenHinter::handle`] on each repaint (which runs after every
/// keystroke) so the edit mode can decide whether to open the menu at all.
pub type CandidateCount = Arc<Mutex<usize>>;

/// Creates the shared candidate-count cell.
pub fn new_candidate_count() -> CandidateCount {
    Arc::new(Mutex::new(0))
}

// ---------------------------------------------------------------------------
// EDIT MODE - fixes zero-candidate Tab activating an invisible chooser
// ---------------------------------------------------------------------------

/// Wraps [`Emacs`] and intercepts Tab to gate menu activation on candidate count.
///
/// * zero candidates  -> `ReedlineEvent::None` (clean decline: no menu state,
///   no visual change, Up/Down/Enter/Esc behave normally immediately)
/// * one or more      -> `UntilFound([Menu, MenuNext])` (normal chooser path)
///
/// Every non-Tab key is delegated unchanged to the inner [`Emacs`] so editing
/// always outranks assistance.
pub struct OmenEditMode {
    inner: Emacs,
    candidate_count: CandidateCount,
}

impl OmenEditMode {
    pub fn new(inner: Emacs, candidate_count: CandidateCount) -> Self {
        Self {
            inner,
            candidate_count,
        }
    }
}

impl EditMode for OmenEditMode {
    fn parse_event(&mut self, event: ReedlineRawEvent) -> ReedlineEvent {
        let crossterm_event: crossterm::event::Event = event.into();
        // Intercept bare Tab to gate menu activation on candidate count.
        if let crossterm::event::Event::Key(ke) = &crossterm_event
            && ke.code == KeyCode::Tab
            && ke.modifiers == KeyModifiers::NONE
        {
            let count = *self
                .candidate_count
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            return tab_event_for_count(count);
        }
        // Everything else: standard Emacs behaviour.
        let event = ReedlineRawEvent::try_from(crossterm_event)
            .expect("Event -> ReedlineRawEvent round-trip is total");
        self.inner.parse_event(event)
    }

    fn edit_mode(&self) -> PromptEditMode {
        self.inner.edit_mode()
    }
}

// ---------------------------------------------------------------------------
// EDIT MODE FACTORY
// ---------------------------------------------------------------------------

/// Creates the ONE interaction edit mode with zero-candidate Tab gating.
pub fn create_edit_mode(candidate_count: CandidateCount) -> Box<dyn EditMode> {
    let mut keybindings = default_emacs_keybindings();
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu(COMPLETION_MENU_NAME.to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );
    Box::new(OmenEditMode::new(Emacs::new(keybindings), candidate_count))
}

/// Creates the ONE completion menu that every candidate class feeds.
pub fn create_completion_menu() -> Box<dyn reedline::Menu> {
    Box::new(ColumnarMenu::default().with_name(COMPLETION_MENU_NAME))
}

// ---------------------------------------------------------------------------
// COMPLETER ADAPTER
// ---------------------------------------------------------------------------

/// Shared bridge from the typed completion engine to Reedline.
pub struct CompleterAdapter(pub Arc<Mutex<OmenCompleter>>);

impl Completer for CompleterAdapter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        if let Ok(mut c) = self.0.lock() {
            c.complete(line, pos)
        } else {
            CompletionResult::fresh(Vec::new())
        }
    }
}

// ---------------------------------------------------------------------------
// LINE EDITOR FACTORY
// ---------------------------------------------------------------------------

/// Builds a Reedline wired to the ONE interaction model.
///
/// `quick_completions` is enabled so a single unambiguous candidate is
/// auto-accepted by Tab without opening a chooser.
pub fn create_line_editor(completer: Arc<Mutex<OmenCompleter>>) -> Reedline {
    let count = new_candidate_count();
    let hinter = Box::new(OmenHinter::new(completer.clone(), count.clone()));
    create_line_editor_inner(completer, hinter, count)
}

/// Same as [`create_line_editor`] but with a caller-supplied hinter.
///
/// The caller must use [`OmenHinter::new`] with the same [`CandidateCount`]
/// cell so the edit mode can consult it.  Use [`create_line_editor`] when
/// possible.
pub fn create_line_editor_with_hinter(
    completer: Arc<Mutex<OmenCompleter>>,
    hinter: Box<dyn reedline::Hinter>,
    candidate_count: CandidateCount,
) -> Reedline {
    create_line_editor_inner(completer, hinter, candidate_count)
}

fn create_line_editor_inner(
    completer: Arc<Mutex<OmenCompleter>>,
    hinter: Box<dyn reedline::Hinter>,
    candidate_count: CandidateCount,
) -> Reedline {
    Reedline::create()
        .with_validator(Box::new(DefaultValidator))
        .with_completer(Box::new(CompleterAdapter(completer)))
        .with_hinter(hinter)
        .with_menu(ReedlineMenu::EngineCompleter(create_completion_menu()))
        .with_edit_mode(create_edit_mode(candidate_count))
        .with_quick_completions(true)
        .with_partial_completions(false)
}

// ---------------------------------------------------------------------------
// ZERO-CANDIDATE TAB CONTRACT (unit-testable without a terminal)
// ---------------------------------------------------------------------------

/// Returns the [`ReedlineEvent`] that Tab should produce for `candidate_count`.
///
/// Extracted as a pure function so the zero-candidate decline is provable
/// without ConPTY.  [`OmenEditMode::parse_event`] delegates to this.
pub fn tab_event_for_count(candidate_count: usize) -> ReedlineEvent {
    if candidate_count == 0 {
        ReedlineEvent::None
    } else {
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu(COMPLETION_MENU_NAME.to_string()),
            ReedlineEvent::MenuNext,
        ])
    }
}

/// Computes the candidate count for `line`/`pos` given a completer context.
///
/// Called by [`OmenHinter`] on every repaint to keep the shared count
/// fresh for [`OmenEditMode`].
pub fn compute_candidate_count(
    completer: &Arc<Mutex<OmenCompleter>>,
    line: &str,
    pos: usize,
) -> usize {
    let Ok(mut comp) = completer.lock() else {
        return 0;
    };
    comp.complete_typed(line, pos).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_event_for_count_zero_is_none() {
        assert!(matches!(tab_event_for_count(0), ReedlineEvent::None));
    }

    #[test]
    fn tab_event_for_count_one_produces_menu_path() {
        assert!(matches!(
            tab_event_for_count(1),
            ReedlineEvent::UntilFound(_)
        ));
    }

    #[test]
    fn tab_event_for_count_many_produces_menu_path() {
        assert!(matches!(
            tab_event_for_count(5),
            ReedlineEvent::UntilFound(_)
        ));
    }
}
