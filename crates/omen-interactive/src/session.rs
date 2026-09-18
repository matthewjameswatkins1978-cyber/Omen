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
        Self::new_with_session_id(session_id, cwd, db)
    }

    pub fn new_with_session_id(
        session_id: InteractiveSessionId,
        cwd: PathBuf,
        db: Option<Database>,
    ) -> Result<Self, CoreError> {
        let supervisor = ProcessSupervisor::new();
        let caps = TerminalCapabilities::detect();
        let prompt = OmenPrompt::new(cwd.clone(), None, 0, false, caps.clone());

        let mut sess = Self {
            session_id,
            cwd,
            supervisor,
            caps,
            prompt,
            last_exit: None,
            db,
        };
        sess.update_prompt_state();
        Ok(sess)
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
            self.update_prompt_state();
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
        // Intercept multiline paste buffer before execution
        if let Some(review) = crate::preflight::PasteGuard::inspect(input) {
            println!(
                "[Paste Guard] {} lines detected in paste buffer. Review before execution:",
                review.line_count
            );
            for (i, line) in review.lines.iter().enumerate() {
                println!("  [{}] {}", i + 1, line);
            }
            return Ok(ProcessExit {
                code: Some(0),
                signal: None,
            });
        }

        let lane = crate::grammar::GrammarScanner::scan(input)?;

        match lane {
            crate::grammar::InputLane::AiReasoning { query } => {
                let out = crate::ai_lane::AiLaneDispatcher::dispatch(
                    &query,
                    &self.session_id,
                    self.db.as_ref(),
                )?;
                println!("{}", out.response_text);
                for cmd in &out.suggested_commands {
                    println!("  {cmd}");
                }
                let exit = ProcessExit {
                    code: Some(0),
                    signal: None,
                };
                self.last_exit = Some(exit.clone());
                self.update_prompt_state();
                Ok(exit)
            }
            crate::grammar::InputLane::SemanticAction { action, args } => {
                let res = crate::actions::SemanticDispatcher::dispatch(
                    &action,
                    &args,
                    &self.cwd,
                    &self.session_id,
                    self.db.as_mut(),
                );
                if let Ok(exit) = &res {
                    self.last_exit = Some(exit.clone());
                }
                self.update_prompt_state();
                res
            }
            crate::grammar::InputLane::Executable { argv } => {
                if argv.is_empty() {
                    return Ok(ProcessExit {
                        code: Some(0),
                        signal: None,
                    });
                }

                // Resolve any typed references in argv (@last, @last.artifact, etc.)
                let resolved_argv = crate::resolver::ReferenceResolver::resolve_argv(
                    &argv,
                    &self.session_id,
                    self.db.as_ref(),
                );

                // Blast-Radius Preflight assessment
                if let Some(blast) = crate::preflight::BlastPreflight::assess(&resolved_argv) {
                    println!(
                        "[Preflight Warning: {:?}] Command '{}' will affect: {}",
                        blast.severity, blast.command, blast.summary
                    );
                }

                let cmd_name = &resolved_argv[0];

                // 1. Interactive child handoff if command is interactive
                if ChildHandoff::is_interactive_command(cmd_name) {
                    let exit = ChildHandoff::spawn_interactive(&resolved_argv, &self.cwd)?;
                    self.last_exit = Some(exit.clone());
                    self.update_prompt_state();
                    return Ok(exit);
                }

                // 2. Ordinary executable invocation via ProcessSupervisor
                let req = ExecutionRequest {
                    argv: resolved_argv.clone(),
                    cwd: self.cwd.clone(),
                    env: vec![],
                    stdin_mode: StdioMode::Closed,
                    stdin_payload: None,
                    timeout_ms: 60000,
                    inline_budget: 65536,
                    required_assurance: RequiredAssurance::default(),
                };

                print!(
                    "{}",
                    omen_ui::SemanticBlock::osc133_command_executed(&self.caps)
                );

                let output = tokio::runtime::Handle::try_current()
                    .map_err(|_| CoreError::Internal("No tokio runtime found".into()))
                    .and_then(|handle| {
                        tokio::task::block_in_place(|| {
                            handle.block_on(self.supervisor.execute(req))
                        })
                    })?;

                print!(
                    "{}",
                    omen_ui::SemanticBlock::osc133_command_finished(
                        output.process_exit.code.unwrap_or(0),
                        &self.caps
                    )
                );

                let exec_id = omen_core::ExecutionId::new(format!(
                    "exec-{}",
                    &hex::encode(sha2::Sha256::digest(
                        format!(
                            "{}-{:?}",
                            resolved_argv.join(" "),
                            std::time::SystemTime::now()
                        )
                        .as_bytes()
                    ))[..12]
                ))?;

                // Record execution in subordinate physical history
                if let Some(db_ref) = &mut self.db {
                    let now = chrono::Utc::now().to_rfc3339();
                    let cas_dir = omen_knowledge::resolve_workspace_dir(&self.cwd).join("cas");
                    let cas = omen_knowledge::ContentAddressedStore::new(cas_dir);
                    let stdout_art = if !output.stdout_all.is_empty() {
                        cas.store(
                            db_ref,
                            &output.stdout_all,
                            "text/plain",
                            "human",
                            omen_core::RetentionClass::Referenced,
                        )
                        .ok()
                        .map(|m| m.uri.as_str().to_string())
                    } else {
                        None
                    };
                    let stderr_art = if !output.stderr_all.is_empty() {
                        cas.store(
                            db_ref,
                            &output.stderr_all,
                            "text/plain",
                            "human",
                            omen_core::RetentionClass::Referenced,
                        )
                        .ok()
                        .map(|m| m.uri.as_str().to_string())
                    } else {
                        None
                    };

                    let exec_rec = omen_knowledge::ExecutionRecord {
                        execution_id: exec_id,
                        session_id: self.session_id.clone(),
                        command: resolved_argv.join(" "),
                        exit_code: output.process_exit.code,
                        duration_ms: Some(output.duration_ms as i64),
                        stdout_artifact: stdout_art,
                        stderr_artifact: stderr_art,
                        envelope_json: None,
                        created_at: now,
                    };
                    let _ = omen_knowledge::ExecutionHistory::record_execution(
                        db_ref,
                        &exec_rec,
                        &[],
                        &[],
                        &[],
                    );
                }

                // Print stdout / stderr to user
                print!("{}", String::from_utf8_lossy(&output.stdout_all));
                eprint!("{}", String::from_utf8_lossy(&output.stderr_all));

                self.last_exit = Some(output.process_exit.clone());
                self.update_prompt_state();

                Ok(output.process_exit)
            }
        }
    }

    pub fn update_prompt_state(&mut self) {
        print!(
            "{}",
            omen_ui::SemanticBlock::osc7_cwd(&self.cwd, &self.caps)
        );
        let has_failure = self
            .last_exit
            .as_ref()
            .map(|e| !e.is_zero())
            .unwrap_or(false);

        let dirty_count = if let Some(db) = &self.db {
            omen_knowledge::FactRegistry::list_active_facts(db, 200)
                .map(|facts| {
                    facts
                        .iter()
                        .filter(|f| f.validity == omen_core::ValidityState::Dirty)
                        .count()
                })
                .unwrap_or(0)
        } else {
            0
        };

        self.prompt
            .update_state(self.cwd.clone(), None, dirty_count, has_failure);
    }
}
