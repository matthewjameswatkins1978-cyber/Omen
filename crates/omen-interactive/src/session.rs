use crate::child::ChildHandoff;
use crate::prompt_adapter::OmenPrompt;
use omen_core::{CoreError, InteractiveSessionId, ProcessExit, RequiredAssurance, StdioMode};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use omen_knowledge::Database;
use omen_ui::TerminalCapabilities;
use reedline::{DefaultValidator, Reedline, Signal};
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
        let mut line_editor = Reedline::create().with_validator(Box::new(DefaultValidator));

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

    /// Dispatches entered input: ordinary executable or internal commands.
    pub fn dispatch_input(&mut self, input: &str) -> Result<ProcessExit, CoreError> {
        let parts: Vec<String> = input.split_whitespace().map(|s| s.to_string()).collect();

        if parts.is_empty() {
            return Ok(ProcessExit {
                code: Some(0),
                signal: None,
            });
        }

        let cmd_name = &parts[0];

        // 1. Interactive child handoff if command is interactive
        if ChildHandoff::is_interactive_command(cmd_name) {
            let exit = ChildHandoff::spawn_interactive(&parts, &self.cwd)?;
            self.last_exit = Some(exit.clone());
            self.update_prompt_state();
            return Ok(exit);
        }

        // 2. Ordinary executable invocation via ProcessSupervisor
        let req = ExecutionRequest {
            argv: parts,
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
                tokio::task::block_in_place(|| handle.block_on(self.supervisor.execute(req)))
            })?;

        // Print stdout / stderr to user
        print!("{}", String::from_utf8_lossy(&output.stdout_all));
        eprint!("{}", String::from_utf8_lossy(&output.stderr_all));

        self.last_exit = Some(output.process_exit.clone());
        self.update_prompt_state();

        Ok(output.process_exit)
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
