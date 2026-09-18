use crate::child::ChildHandoff;
use crate::prompt_adapter::OmenPrompt;
use omen_core::{CoreError, InteractiveSessionId, ProcessExit, RequiredAssurance, StdioMode};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use omen_knowledge::Database;
use omen_ui::TerminalCapabilities;
use reedline::{DefaultValidator, MenuBuilder, Reedline, Signal};
use sha2::Digest;
use std::path::PathBuf;

pub struct InteractiveSession {
    pub session_id: InteractiveSessionId,
    pub cwd: PathBuf,
    pub supervisor: ProcessSupervisor,
    pub caps: TerminalCapabilities,
    pub prompt: OmenPrompt,
    pub last_exit: Option<ProcessExit>,
    pub db: Option<Database>,
}

impl InteractiveSession {
    pub fn new(cwd: PathBuf, db: Option<Database>) -> Result<Self, CoreError> {
        let session_id = InteractiveSessionId::new(format!(
            "sess-{}",
            &hex::encode(sha2::Sha256::digest(cwd.to_string_lossy().as_bytes()))[..12]
        ))?;
        let supervisor = ProcessSupervisor::new();
        let caps = TerminalCapabilities::detect();
        let prompt = OmenPrompt::new(cwd.clone(), None, 0, false, caps.clone());

        Ok(Self {
            session_id,
            cwd,
            supervisor,
            caps,
            prompt,
            last_exit: None,
            db,
        })
    }

    /// Runs the interactive REPL loop.
    pub fn run_loop(&mut self) -> Result<(), CoreError> {
        let comp_ctx = std::sync::Arc::new(std::sync::Mutex::new(
            crate::completion::CompletionContext {
                cwd: self.cwd.clone(),
                db: None,
                ..Default::default()
            },
        ));
        let completer = std::sync::Arc::new(std::sync::Mutex::new(
            crate::completion::OmenCompleter::new(comp_ctx),
        ));
        let hinter = Box::new(crate::completion::OmenHinter::new(completer.clone()));
        let completion_menu =
            Box::new(reedline::ColumnarMenu::default().with_name("completion_menu"));

        struct CompleterAdapter(std::sync::Arc<std::sync::Mutex<crate::completion::OmenCompleter>>);
        impl reedline::Completer for CompleterAdapter {
            fn complete(&mut self, line: &str, pos: usize) -> reedline::CompletionResult {
                if let Ok(mut c) = self.0.lock() {
                    c.complete(line, pos)
                } else {
                    reedline::CompletionResult::fresh(Vec::new())
                }
            }
        }

        let mut line_editor = Reedline::create()
            .with_validator(Box::new(DefaultValidator))
            .with_completer(Box::new(CompleterAdapter(completer)))
            .with_hinter(hinter)
            .with_menu(reedline::ReedlineMenu::EngineCompleter(completion_menu));

        loop {
            let sig = line_editor.read_line(&self.prompt);
            match sig {
                Ok(Signal::Success(buffer)) => {
                    let trimmed = buffer.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    if trimmed == "exit" || trimmed == "quit" {
                        break;
                    }

                    if let Err(e) = self.dispatch_input(trimmed) {
                        eprintln!("Error: {e}");
                    }
                }
                Ok(Signal::CtrlC) => {
                    println!("^C");
                }
                Ok(Signal::CtrlD) => {
                    println!("exit");
                    break;
                }
                Ok(_) => {}
                Err(err) => {
                    eprintln!("Interactive input error: {err}");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Dispatches entered input: ordinary executable, semantic action (:), or AI lane (?).
    pub fn dispatch_input(&mut self, input: &str) -> Result<ProcessExit, CoreError> {
        let lane = crate::grammar::GrammarScanner::scan(input)?;

        match lane {
            crate::grammar::InputLane::AiReasoning { query } => {
                println!(
                    "AI reasoning is not configured.\nDeterministic options:\n  :show @failed\n  :why @last\n  :open @failed\nQuery was: {query}"
                );
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            crate::grammar::InputLane::SemanticAction { action, args } => {
                crate::actions::SemanticDispatcher::dispatch(
                    &action,
                    &args,
                    &self.cwd,
                    self.db.as_mut(),
                )
            }
            crate::grammar::InputLane::Executable { argv } => {
                if argv.is_empty() {
                    return Ok(ProcessExit {
                        code: Some(0),
                        signal: None,
                    });
                }

                let cmd_name = &argv[0];

                // 1. Interactive child handoff if command is interactive
                if ChildHandoff::is_interactive_command(cmd_name) {
                    let exit = ChildHandoff::spawn_interactive(&argv, &self.cwd)?;
                    self.last_exit = Some(exit.clone());
                    self.update_prompt_state();
                    return Ok(exit);
                }

                // 2. Ordinary executable invocation via ProcessSupervisor
                let req = ExecutionRequest {
                    argv,
                    cwd: self.cwd.clone(),
                    env: vec![],
                    stdin_mode: StdioMode::Closed,
                    stdin_payload: None,
                    timeout_ms: 60000,
                    inline_budget: 65536,
                    required_assurance: RequiredAssurance::default(),
                };

                let output = tokio::runtime::Handle::try_current()
                    .map_err(|_| CoreError::Internal("No tokio runtime found".into()))
                    .and_then(|handle| {
                        tokio::task::block_in_place(|| {
                            handle.block_on(self.supervisor.execute(req))
                        })
                    })?;

                // Print stdout / stderr to user
                print!("{}", String::from_utf8_lossy(&output.stdout_all));
                eprint!("{}", String::from_utf8_lossy(&output.stderr_all));

                self.last_exit = Some(output.process_exit.clone());
                self.update_prompt_state();

                Ok(output.process_exit)
            }
        }
    }

    fn update_prompt_state(&mut self) {
        let has_failure = self
            .last_exit
            .as_ref()
            .map(|e| !e.is_zero())
            .unwrap_or(false);
        self.prompt
            .update_state(self.cwd.clone(), None, 0, has_failure);
    }
}
