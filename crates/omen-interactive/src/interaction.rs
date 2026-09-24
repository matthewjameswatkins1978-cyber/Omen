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
    default_emacs_keybindings, ColumnarMenu, Completer, CompletionResult, DefaultValidator, Emacs,
    KeyCode, KeyModifiers, MenuBuilder, Reedline, ReedlineEvent, ReedlineMenu,
};
use std::sync::{Arc, Mutex};

/// The name of the single completion menu used for every candidate class.
pub const COMPLETION_MENU_NAME: &str = "completion_menu";

/// Creates the ONE interaction edit mode.
///
/// Tab is bound to `UntilFound([Menu, MenuNext])` which, combined with
/// `quick_completions`, realises the universal Tab behaviour:
///
/// * one unambiguous candidate — auto-completes (reedline `decide_menu_completion`)
/// * multiple candidates — opens the navigable chooser
/// * zero candidates — the menu opens empty and Esc dismisses cleanly
///
/// Every other key keeps its reedline default so editing always outranks
/// assistance.
pub fn create_edit_mode() -> Box<dyn reedline::EditMode> {
    let mut keybindings = default_emacs_keybindings();
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu(COMPLETION_MENU_NAME.to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );
    Box::new(Emacs::new(keybindings))
}

/// Creates the ONE completion menu that every candidate class feeds.
pub fn create_completion_menu() -> Box<dyn reedline::Menu> {
    Box::new(ColumnarMenu::default().with_name(COMPLETION_MENU_NAME))
}

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

/// Builds a Reedline wired to the ONE interaction model.
///
/// `quick_completions` is enabled so a single unambiguous candidate is
/// auto-accepted by Tab without opening a chooser.
pub fn create_line_editor(completer: Arc<Mutex<OmenCompleter>>) -> Reedline {
    let hinter = Box::new(OmenHinter::new(completer.clone()));
    create_line_editor_with_hinter(completer, hinter)
}

/// Same as [`create_line_editor`] but with a caller-supplied hinter.
pub fn create_line_editor_with_hinter(
    completer: Arc<Mutex<OmenCompleter>>,
    hinter: Box<dyn reedline::Hinter>,
) -> Reedline {
    Reedline::create()
        .with_validator(Box::new(DefaultValidator))
        .with_completer(Box::new(CompleterAdapter(completer)))
        .with_hinter(hinter)
        .with_menu(ReedlineMenu::EngineCompleter(create_completion_menu()))
        .with_edit_mode(create_edit_mode())
        .with_quick_completions(true)
        .with_partial_completions(false)
}
