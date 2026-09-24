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
//! M1 adds one state to the Tab gate: PENDING ASYNC DISCOVERY. Tab is gated on
//! [`CompletionReadiness`], not a bare count —
//!
//! - `FinalZero` -> Tab declines cleanly (exact M0 semantics);
//! - `Ready(n)`  -> Tab opens the chooser;
//! - `Pending`   -> Tab opens the completion path *because discovery is not
//!   final yet* (zero inline + in-flight/triggered work is NOT a proven zero).
//!
//! When the chooser is not active, normal editing remains normal.
//! Editing outranks assistance: no ordinary editing key is overridden.

use crate::completion::{CompletionReadiness, OmenCompleter, OmenHinter, ReadinessCell};
use reedline::{
    ColumnarMenu, Completer, CompletionResult, DefaultValidator, EditMode, Emacs, KeyCode,
    KeyModifiers, MenuBuilder, PromptEditMode, Reedline, ReedlineEvent, ReedlineMenu,
    ReedlineRawEvent, default_emacs_keybindings,
};
use std::sync::{Arc, Mutex};

/// The name of the single completion menu used for every candidate class.
pub const COMPLETION_MENU_NAME: &str = "completion_menu";

/// Creates the shared readiness cell (starts at [`CompletionReadiness::FinalZero`]).
///
/// The cell is updated by [`OmenHinter::handle`] on each repaint and read by
/// [`OmenEditMode`] on every Tab, so the gate always reflects the live
/// discovery state — including pending async work.
pub fn new_readiness() -> ReadinessCell {
    crate::completion::new_readiness()
}

// ---------------------------------------------------------------------------
// EDIT MODE - zero-candidate Tab declines, pending Tab does not get swallowed
// ---------------------------------------------------------------------------

/// Wraps [`Emacs`] and intercepts Tab to gate menu activation on readiness.
///
/// * [`CompletionReadiness::FinalZero`] -> `ReedlineEvent::None` (clean
///   decline: no menu state, no visual change, Up/Down/Enter/Esc behave
///   normally immediately)
/// * [`CompletionReadiness::Ready`] / [`CompletionReadiness::Pending`] ->
///   `UntilFound([Menu, MenuNext])` (normal chooser path; a pending request
///   may still deliver candidates through the async settle)
///
/// Every non-Tab key is delegated unchanged to the inner [`Emacs`] so editing
/// always outranks assistance.
pub struct OmenEditMode {
    inner: Emacs,
    readiness: ReadinessCell,
}

impl OmenEditMode {
    pub fn new(inner: Emacs, readiness: ReadinessCell) -> Self {
        Self { inner, readiness }
    }
}

impl EditMode for OmenEditMode {
    fn parse_event(&mut self, event: ReedlineRawEvent) -> ReedlineEvent {
        let crossterm_event: crossterm::event::Event = event.into();
        // Intercept bare Tab to gate menu activation on readiness.
        if let crossterm::event::Event::Key(ke) = &crossterm_event
            && ke.code == KeyCode::Tab
            && ke.modifiers == KeyModifiers::NONE
        {
            let readiness = *self.readiness.lock().unwrap_or_else(|e| e.into_inner());
            return tab_event_for_readiness(readiness);
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

/// Creates the ONE interaction edit mode with readiness-gated Tab.
pub fn create_edit_mode(readiness: ReadinessCell) -> Box<dyn EditMode> {
    let mut keybindings = default_emacs_keybindings();
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu(COMPLETION_MENU_NAME.to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );
    Box::new(OmenEditMode::new(Emacs::new(keybindings), readiness))
}

/// Creates the ONE completion menu that every candidate class feeds.
pub fn create_completion_menu() -> Box<dyn reedline::Menu> {
    Box::new(ColumnarMenu::default().with_name(COMPLETION_MENU_NAME))
}

// ---------------------------------------------------------------------------
// COMPLETER ADAPTER
// ---------------------------------------------------------------------------

/// Shared bridge from the typed completion engine to Reedline.
///
/// Delegates to [`OmenCompleter`], which is backed by the M1 discovery
/// runtime: this issues a completion **request** (non-blocking, async seam).
pub struct CompleterAdapter(pub Arc<Mutex<OmenCompleter>>);

impl Completer for CompleterAdapter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        match self.0.lock() {
            Ok(mut c) => c.complete(line, pos),
            Err(e) => e.into_inner().complete(line, pos),
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
    let readiness = new_readiness();
    let hinter = Box::new(OmenHinter::new(completer.clone(), readiness.clone()));
    create_line_editor_inner(completer, hinter, readiness)
}

/// Same as [`create_line_editor`] but with a caller-supplied hinter.
///
/// The caller must use [`OmenHinter::new`] with the same [`ReadinessCell`]
/// cell so the edit mode can consult it.  Use [`create_line_editor`] when
/// possible.
pub fn create_line_editor_with_hinter(
    completer: Arc<Mutex<OmenCompleter>>,
    hinter: Box<dyn reedline::Hinter>,
    candidate_count: ReadinessCell,
) -> Reedline {
    create_line_editor_inner(completer, hinter, candidate_count)
}

fn create_line_editor_inner(
    completer: Arc<Mutex<OmenCompleter>>,
    hinter: Box<dyn reedline::Hinter>,
    candidate_count: ReadinessCell,
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
// ZERO-CANDIDATE / PENDING TAB CONTRACT (unit-testable without a terminal)
// ---------------------------------------------------------------------------

/// Returns the [`ReedlineEvent`] that Tab should produce for `readiness`.
///
/// Extracted as a pure function so the zero-candidate decline and the
/// pending-vs-zero distinction are provable without ConPTY.
/// [`OmenEditMode::parse_event`] delegates to this.
pub fn tab_event_for_readiness(readiness: CompletionReadiness) -> ReedlineEvent {
    match readiness {
        // Discovery proved zero: decline cleanly (M0 law).
        CompletionReadiness::FinalZero => ReedlineEvent::None,
        // Discovery delivered candidates now.
        CompletionReadiness::Ready(0) => ReedlineEvent::None,
        CompletionReadiness::Ready(_) => ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu(COMPLETION_MENU_NAME.to_string()),
            ReedlineEvent::MenuNext,
        ]),
        // Discovery still in flight: Tab must NOT be swallowed as a proven
        // zero. Open the completion path; async results settle into it.
        CompletionReadiness::Pending { .. } => ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu(COMPLETION_MENU_NAME.to_string()),
            ReedlineEvent::MenuNext,
        ]),
    }
}

/// Computes the readiness for `line`/`pos` from the live completer.
///
/// Called by [`OmenHinter`] on every repaint to keep the shared cell fresh
/// for [`OmenEditMode`]. Read-only: never dispatches background work.
pub fn compute_readiness(
    completer: &Arc<Mutex<OmenCompleter>>,
    line: &str,
    pos: usize,
) -> CompletionReadiness {
    match completer.lock() {
        Ok(mut comp) => comp.readiness(line, pos),
        Err(e) => e.into_inner().readiness(line, pos),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_event_final_zero_is_none() {
        assert!(matches!(
            tab_event_for_readiness(CompletionReadiness::FinalZero),
            ReedlineEvent::None
        ));
    }

    #[test]
    fn tab_event_ready_produces_menu_path() {
        assert!(matches!(
            tab_event_for_readiness(CompletionReadiness::Ready(1)),
            ReedlineEvent::UntilFound(_)
        ));
        assert!(matches!(
            tab_event_for_readiness(CompletionReadiness::Ready(5)),
            ReedlineEvent::UntilFound(_)
        ));
        // Defensive: a ready-zero behaves like a proven zero.
        assert!(matches!(
            tab_event_for_readiness(CompletionReadiness::Ready(0)),
            ReedlineEvent::None
        ));
    }

    #[test]
    fn tab_event_pending_is_never_swallowed_as_zero() {
        // The whole point of the pending state: zero inline candidates with
        // async work outstanding must still open the completion path.
        assert!(matches!(
            tab_event_for_readiness(CompletionReadiness::Pending { inline: 0 }),
            ReedlineEvent::UntilFound(_)
        ));
        assert!(matches!(
            tab_event_for_readiness(CompletionReadiness::Pending { inline: 3 }),
            ReedlineEvent::UntilFound(_)
        ));
    }

    #[test]
    fn readiness_cell_starts_at_final_zero() {
        let cell = new_readiness();
        assert_eq!(*cell.lock().unwrap(), CompletionReadiness::FinalZero);
    }
}
