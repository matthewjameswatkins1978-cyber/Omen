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
    pub comp_ctx: std::sync::Arc<std::sync::Mutex<crate::completion::CompletionContext>>,
    pub client: Option<omen_client::OmenClient>,
}

impl InteractiveSession {
    pub fn new(cwd: PathBuf, db: Option<Database>) -> Result<Self, CoreError> {
        let session_id = InteractiveSessionId::generate();
        Self::new_with_session_id(session_id, cwd, db)
    }

    pub fn new_with_session_id(
        session_id: InteractiveSessionId,
        cwd: PathBuf,
        db: Option<Database>,
    ) -> Result<Self, CoreError> {
        let client = if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let sid = session_id.to_string();
            let c_path = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
            let path_str = c_path.to_string_lossy().to_string();
            match handle.runtime_flavor() {
                tokio::runtime::RuntimeFlavor::MultiThread => tokio::task::block_in_place(|| {
                    handle.block_on(async {
                        if let Ok(c) = omen_client::OmenClient::connect_default(Some(sid)).await {
                            let _ = c.attach_workspace(&path_str).await;
                            Some(c)
                        } else {
                            None
                        }
                    })
                }),
                _ => std::thread::scope(|s| {
                    s.spawn(|| {
                        tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .ok()
                            .and_then(|rt| {
                                rt.block_on(async {
                                    if let Ok(c) =
                                        omen_client::OmenClient::connect_default(Some(sid)).await
                                    {
                                        let _ = c.attach_workspace(&path_str).await;
                                        Some(c)
                                    } else {
                                        None
                                    }
                                })
                            })
                    })
                    .join()
                    .unwrap_or(None)
                }),
            }
        } else {
            None
        };

        Self::new_with_client(session_id, cwd, db, client)
    }

    pub fn new_with_client(
        session_id: InteractiveSessionId,
        cwd: PathBuf,
        db: Option<Database>,
        client: Option<omen_client::OmenClient>,
    ) -> Result<Self, CoreError> {
        let supervisor = ProcessSupervisor::new();
        let caps = TerminalCapabilities::detect();
        let mode_label = if client.is_some() {
            "[shared]"
        } else {
            "[standalone]"
        };
        let prompt = OmenPrompt::new(cwd.clone(), None, 0, false, caps.clone())
            .with_mode_indicator(mode_label);

        let mut hot_index = crate::completion::HotSemanticIndex::default();
        hot_index.refresh(&cwd, db.as_ref());

        let comp_ctx = std::sync::Arc::new(std::sync::Mutex::new(
            crate::completion::CompletionContext {
                cwd: cwd.clone(),
                hot_index,
            },
        ));

        if let Some(ref c) = client {
            let mut event_rx = c.subscribe_events();
            let comp_ctx_bg = comp_ctx.clone();
            let client_bg = c.clone();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    if let (Ok(snap), Ok(mut ctx)) =
                        (client_bg.get_snapshot().await, comp_ctx_bg.lock())
                    {
                        ctx.hot_index.apply_snapshot(&snap);
                    }
                    while let Ok(event) = event_rx.recv().await {
                        match event.payload {
                            omen_ipc::EventPayload::FactPublished {
                                resource_uri,
                                validity,
                                ..
                            } => {
                                let info = omen_ipc::FactInfo {
                                    resource_uri,
                                    fact_id: String::new(),
                                    value: String::new(),
                                    validity,
                                    assurance: String::new(),
                                };
                                if let Ok(mut ctx) = comp_ctx_bg.lock() {
                                    ctx.hot_index.update_fact(&info);
                                }
                            }
                            omen_ipc::EventPayload::FactInvalidated { resource_uri, .. } => {
                                if let Ok(mut ctx) = comp_ctx_bg.lock() {
                                    ctx.hot_index.mark_fact_dirty(&resource_uri);
                                }
                            }
                            omen_ipc::EventPayload::ResyncRequired { .. } => {
                                if let (Ok(snap), Ok(mut ctx)) =
                                    (client_bg.get_snapshot().await, comp_ctx_bg.lock())
                                {
                                    ctx.hot_index.apply_snapshot(&snap);
                                }
                            }
                            _ => {}
                        }
                    }
                });
            }
        }

        let mut sess = Self {
            session_id,
            cwd,
            supervisor,
            caps,
            prompt,
            last_exit: None,
            db,
            comp_ctx,
            client,
        };
        sess.update_prompt_state();
        Ok(sess)
    }

    /// Runs the interactive REPL loop.
    pub fn run_loop(&mut self) -> Result<(), CoreError> {
        let completer = std::sync::Arc::new(std::sync::Mutex::new(
            crate::completion::OmenCompleter::new(self.comp_ctx.clone()),
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

                // 1. Interactive child handoff if invocation requires terminal ownership
                if ChildHandoff::classify(&resolved_argv)
                    == crate::child::ChildClassification::InteractiveHandoff
                {
                    let start_t = std::time::Instant::now();
                    let exit = ChildHandoff::spawn_interactive(&resolved_argv, &self.cwd)?;
                    let duration_ms = start_t.elapsed().as_millis() as i64;
                    self.last_exit = Some(exit.clone());

                    // Record interactive execution in subordinate physical history
                    if let Some(db_ref) = &mut self.db {
                        let now = chrono::Utc::now().to_rfc3339();
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
                        let exec_rec = omen_knowledge::ExecutionRecord {
                            execution_id: exec_id,
                            session_id: self.session_id.clone(),
                            command: resolved_argv.join(" "),
                            exit_code: exit.code,
                            duration_ms: Some(duration_ms),
                            stdout_artifact: None,
                            stderr_artifact: None,
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

                // Compute CAS artifacts and record execution in subordinate physical history
                let cas_dir = omen_knowledge::resolve_workspace_dir(&self.cwd).join("cas");
                let cas = omen_knowledge::ContentAddressedStore::new(cas_dir);
                let (stdout_art, stderr_art) = if let Some(db_ref) = &mut self.db {
                    let out_art = if !output.stdout_all.is_empty() {
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
                    let err_art = if !output.stderr_all.is_empty() {
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

                    let now = chrono::Utc::now().to_rfc3339();
                    let exec_rec = omen_knowledge::ExecutionRecord {
                        execution_id: exec_id,
                        session_id: self.session_id.clone(),
                        command: resolved_argv.join(" "),
                        exit_code: output.process_exit.code,
                        duration_ms: Some(output.duration_ms as i64),
                        stdout_artifact: out_art.clone(),
                        stderr_artifact: err_art.clone(),
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
                    (out_art, err_art)
                } else {
                    (None, None)
                };

                // If daemon client is connected, record history asynchronously
                if let (Some(c), Ok(handle)) = (&self.client, tokio::runtime::Handle::try_current())
                {
                    let client_c = c.clone();
                    let cmd = resolved_argv.join(" ");
                    let exit_code = output.process_exit.code;
                    let duration = output.duration_ms;
                    let out_art = stdout_art.clone();
                    let err_art = stderr_art.clone();
                    handle.spawn(async move {
                        let _ = client_c
                            .record_history(cmd, exit_code, duration, out_art, err_art)
                            .await;
                    });
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
        } else if let Ok(ctx) = self.comp_ctx.lock() {
            ctx.hot_index
                .active_facts
                .iter()
                .filter(|f| f.validity == omen_core::ValidityState::Dirty)
                .count()
        } else {
            0
        };

        let mode_label = if self
            .client
            .as_ref()
            .map(|c| c.is_connected())
            .unwrap_or(false)
        {
            "[shared]"
        } else {
            "[standalone]"
        };
        self.prompt.set_mode_indicator(Some(mode_label.to_string()));

        self.prompt
            .update_state(self.cwd.clone(), None, dirty_count, has_failure);

        // Refresh bounded hot semantic index out-of-band for completion
        if let Ok(mut ctx) = self.comp_ctx.lock() {
            ctx.cwd = self.cwd.clone();
            if self.client.is_none() {
                ctx.hot_index.refresh(&self.cwd, self.db.as_ref());
            }
        }
    }
}
