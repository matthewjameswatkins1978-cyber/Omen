//! Minimal Reedline loop for genuine ConPTY terminal-gate testing.
//!
//! Spawns a Reedline line editor wired to the F1 completion engine so an
//! external ConPTY test can send keystrokes and observe real terminal output.

use omen_interactive::completion::{
    CompletionContext, HotSemanticIndex, OmenCompleter, OmenHinter,
};
use reedline::{DefaultValidator, MenuBuilder, Prompt, Reedline, Signal};
use std::borrow::Cow;
use std::sync::{Arc, Mutex};

struct GatePrompt;
impl Prompt for GatePrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        "pty-gate> ".into()
    }
    fn render_prompt_right(&self) -> Cow<'_, str> {
        "".into()
    }
    fn render_prompt_indicator(&self, _prompt_mode: reedline::PromptEditMode) -> Cow<'_, str> {
        "".into()
    }
    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        "::: ".into()
    }
    fn render_prompt_history_search_indicator(
        &self,
        _history_search: reedline::PromptHistorySearch,
    ) -> Cow<'_, str> {
        "search> ".into()
    }
}

fn main() {
    let mut hot = HotSemanticIndex::default();
    hot.update_path_commands(vec![
        "cargo".into(),
        "git".into(),
        "rg".into(),
        "cat".into(),
        "echo".into(),
    ]);

    let ctx = Arc::new(Mutex::new(CompletionContext {
        cwd: std::env::current_dir().unwrap_or_else(|_| ".".into()),
        hot_index: hot,
    }));

    let completer = Arc::new(Mutex::new(OmenCompleter::new(ctx)));
    let hinter = Box::new(OmenHinter::new(completer.clone()));

    struct CompleterAdapter(Arc<Mutex<OmenCompleter>>);
    impl reedline::Completer for CompleterAdapter {
        fn complete(&mut self, line: &str, pos: usize) -> reedline::CompletionResult {
            if let Ok(mut c) = self.0.lock() {
                c.complete(line, pos)
            } else {
                reedline::CompletionResult::fresh(Vec::new())
            }
        }
    }

    let completion_menu = Box::new(reedline::ColumnarMenu::default().with_name("completion_menu"));

    let mut line_editor = Reedline::create()
        .with_validator(Box::new(DefaultValidator))
        .with_completer(Box::new(CompleterAdapter(completer)))
        .with_hinter(hinter)
        .with_menu(reedline::ReedlineMenu::EngineCompleter(completion_menu));

    loop {
        match line_editor.read_line(&GatePrompt) {
            Ok(Signal::Success(buffer)) => {
                let trimmed = buffer.trim();
                if trimmed == "exit" || trimmed == "quit" {
                    eprintln!("PTY_GATE_EXIT");
                    break;
                }
                eprintln!("PTY_GATE_LINE:{trimmed}");
            }
            Ok(Signal::CtrlC) => {
                eprintln!("PTY_GATE_CTRLC");
            }
            Ok(Signal::CtrlD) => {
                eprintln!("PTY_GATE_CTRLD");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("PTY_GATE_ERROR:{e}");
                break;
            }
        }
    }
}
