use crate::child::ChildHandoff;
use crate::prompt_adapter::OmenPrompt;
use omen_core::{CoreError, InteractiveSessionId, ProcessExit, RequiredAssurance, StdioMode};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use omen_knowledge::Database;
use omen_ui::{HumanSettings, TerminalCapabilities};
use reedline::{DefaultValidator, MenuBuilder, Reedline, Signal};
use std::path::PathBuf;

pub struct InteractiveSession {
    pub session_id: InteractiveSessionId,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub supervisor: ProcessSupervisor,
    pub caps: TerminalCapabilities,
    pub prompt: OmenPrompt,
    pub last_exit: Option<ProcessExit>,
    pub db: Option<Database>,
    pub comp_ctx: std::sync::Arc<std::sync::Mutex<crate::completion::CompletionContext>>,
    pub client: Option<omen_client::OmenClient>,
    pub agent_provider: Option<std::sync::Arc<dyn omen_agent::AgentProvider>>,
    pub agent_registry: std::sync::Arc<omen_agent::ProviderRegistry>,
    pub backend_registry: std::sync::Arc<omen_engine::BackendRegistry>,
    pub human_settings: HumanSettings,
}

pub fn block_on_async<F>(future: F) -> F::Output
where
    F: std::future::Future + Send,
    F::Output: Send,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        match handle.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| handle.block_on(future))
            }
            _ => std::thread::scope(|s| {
                s.spawn(|| {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("Failed to build local tokio runtime")
                        .block_on(future)
                })
                .join()
                .expect("Tokio runtime thread panicked")
            }),
        }
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build local tokio runtime")
            .block_on(future)
    }
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
        let client = block_on_async(async {
            let sid = session_id.to_string();
            let c_path = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
            let path_str = c_path.to_string_lossy().to_string();
            if let Ok(c) = omen_client::OmenClient::connect_default(Some(sid)).await {
                let _ = c.attach_workspace(&path_str).await;
                Some(c)
            } else {
                None
            }
        });

        Self::new_with_client(session_id, cwd, db, client)
    }

    pub fn new_with_client(
        session_id: InteractiveSessionId,
        cwd: PathBuf,
        db: Option<Database>,
        client: Option<omen_client::OmenClient>,
    ) -> Result<Self, CoreError> {
        Self::new_with_workspace_and_client(session_id, cwd.clone(), cwd, db, client)
    }

    pub fn new_with_workspace_and_client(
        session_id: InteractiveSessionId,
        workspace_root: PathBuf,
        cwd: PathBuf,
        db: Option<Database>,
        client: Option<omen_client::OmenClient>,
    ) -> Result<Self, CoreError> {
        let backend_registry = std::sync::Arc::new(omen_engine::BackendRegistry::new());
        let supervisor = ProcessSupervisor::with_backend(backend_registry.active());
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

        let agent_registry = std::sync::Arc::new(omen_agent::ProviderRegistry::new());
        let agent_provider: Option<std::sync::Arc<dyn omen_agent::AgentProvider>> =
            Some(agent_registry.active_provider());

        let mut sess = Self {
            session_id,
            workspace_root,
            cwd,
            supervisor,
            caps,
            prompt,
            last_exit: None,
            db,
            comp_ctx,
            client,
            agent_provider,
            agent_registry,
            backend_registry,
            human_settings: HumanSettings::default(),
        };
        sess.prompt.apply_settings(sess.human_settings.clone());
        sess.update_prompt_state();
        Ok(sess)
    }

    pub fn with_agent_provider(
        mut self,
        agent_provider: Option<std::sync::Arc<dyn omen_agent::AgentProvider>>,
    ) -> Self {
        self.agent_provider = agent_provider;
        self
    }

    pub fn register_agent_provider(
        &self,
        descriptor: omen_agent::ProviderDescriptor,
        provider: std::sync::Arc<dyn omen_agent::AgentProvider>,
    ) {
        self.agent_registry.register(descriptor, provider);
    }

    pub fn use_agent_provider(&mut self, id: &str) -> Result<(), omen_agent::AgentError> {
        self.agent_registry.set_active_provider(id)?;
        self.agent_provider = Some(self.agent_registry.active_provider());
        Ok(())
    }

    /// Runs the interactive REPL loop.
    pub fn run_loop(&mut self) -> Result<(), CoreError> {
        self.ensure_first_run()?;
        if self.caps.is_interactive {
            let startup = omen_ui::appearance::startup_message(&self.human_settings, &self.caps);
            if !startup.is_empty() {
                print!("{startup}");
            }
        }
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
                        let roles =
                            omen_ui::ColorRoles::for_theme(&self.caps, self.human_settings.theme);
                        eprintln!(
                            "{}",
                            omen_ui::DiagnosticRenderer::render_human_error(&e, &roles)
                        );
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

    fn ensure_first_run(&mut self) -> Result<(), CoreError> {
        let Some(path) = HumanSettings::default_path() else {
            return Ok(());
        };
        let configured = HumanSettings::load(&path)
            .map_err(|error| CoreError::Internal(format!("human settings load failed: {error}")))?;
        if let Some(settings) = configured {
            self.human_settings = settings;
            self.prompt.apply_settings(self.human_settings.clone());
            return Ok(());
        }
        if !self.caps.is_interactive {
            return Ok(());
        }

        let theme =
            omen_ui::appearance::run_appearance_chooser(self.human_settings.theme, &self.caps)
                .map_err(|error| {
                    CoreError::Internal(format!("appearance chooser failed: {error}"))
                })?;
        self.human_settings.theme = theme;
        self.human_settings
            .save(&path)
            .map_err(|error| CoreError::Internal(format!("human settings save failed: {error}")))?;
        self.prompt.apply_settings(self.human_settings.clone());
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
                let out = crate::ai_lane::AiLaneDispatcher::dispatch_with_workspace(
                    &query,
                    &self.session_id,
                    &self.workspace_root,
                    &self.cwd,
                    self.db.as_ref(),
                    self.agent_provider.as_deref(),
                    Some(&self.comp_ctx),
                )?;
                println!("{}", out.response_text);
                for cmd in &out.suggested_commands {
                    println!("  {cmd}");
                }
                for action in &out.proposed_actions {
                    match action {
                        omen_agent::ProposedAction::ChangeDirectory { path } => {
                            let target = std::path::PathBuf::from(path);
                            let resolved = if target.is_absolute() {
                                target
                            } else {
                                self.cwd.join(target)
                            };
                            if resolved.is_dir() {
                                self.cwd = resolved.canonicalize().unwrap_or(resolved);
                                if let Ok(mut ctx) = self.comp_ctx.lock() {
                                    ctx.cwd = self.cwd.clone();
                                    ctx.hot_index.refresh(&self.cwd, self.db.as_ref());
                                }
                                println!("Changed directory to {}", self.cwd.display());
                            }
                        }
                        omen_agent::ProposedAction::ExecuteTool {
                            tool,
                            operation,
                            args,
                            cwd,
                        } => {
                            if Self::is_permitted_agent_action(action) {
                                let display = if operation.is_empty() {
                                    format!("{tool} {}", args.join(" "))
                                } else {
                                    format!("{tool} {operation} {}", args.join(" "))
                                };
                                println!("Executing permitted action: {display}");
                                let exec_cwd = cwd.as_ref().map(PathBuf::from);
                                let exit = self.execute_via_broker(
                                    tool,
                                    operation,
                                    args.clone(),
                                    exec_cwd,
                                )?;
                                return Ok(exit);
                            } else {
                                let in_cwd = cwd
                                    .as_ref()
                                    .map(|c| format!(" (in {c})"))
                                    .unwrap_or_default();
                                println!(
                                    "Proposed tool{in_cwd}: {tool} {operation} {}",
                                    args.join(" ")
                                );
                            }
                        }
                        omen_agent::ProposedAction::ExecuteCommand { argv, cwd } => {
                            if Self::is_permitted_agent_action(action) {
                                println!("Executing permitted command: {}", argv.join(" "));
                                let exec_cwd = cwd.as_ref().map(PathBuf::from);
                                let exit =
                                    self.execute_via_broker("exec", "", argv.clone(), exec_cwd)?;
                                return Ok(exit);
                            } else {
                                let in_cwd = cwd
                                    .as_ref()
                                    .map(|c| format!(" (in {c})"))
                                    .unwrap_or_default();
                                println!("Proposed command{in_cwd}: {}", argv.join(" "));
                            }
                        }
                        omen_agent::ProposedAction::SemanticAction { action, args } => {
                            println!("Proposed action: :{action} {}", args.join(" "));
                        }
                    }
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
                    Some(&self.agent_registry),
                    Some(&self.backend_registry),
                );
                if let Ok(exit) = &res {
                    self.last_exit = Some(exit.clone());
                }
                self.supervisor = ProcessSupervisor::with_backend(self.backend_registry.active());
                self.agent_provider = Some(self.agent_registry.active_provider());
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

                // Built-in shell navigation: cd modifies session.cwd while preserving workspace_root
                if resolved_argv.first().map(|s| s.as_str()) == Some("cd") {
                    let target_path = if let Some(target) = resolved_argv.get(1) {
                        let p = std::path::PathBuf::from(target);
                        if p.is_absolute() { p } else { self.cwd.join(p) }
                    } else {
                        self.workspace_root.clone()
                    };

                    if target_path.is_dir() {
                        self.cwd = target_path.canonicalize().unwrap_or(target_path);
                        if let Ok(mut ctx) = self.comp_ctx.lock() {
                            ctx.cwd = self.cwd.clone();
                            ctx.hot_index.refresh(&self.cwd, self.db.as_ref());
                        }
                        let exit = ProcessExit {
                            code: Some(0),
                            signal: None,
                        };
                        self.last_exit = Some(exit.clone());
                        self.update_prompt_state();
                        return Ok(exit);
                    } else {
                        eprintln!("cd: {}: No such file or directory", target_path.display());
                        let exit = ProcessExit {
                            code: Some(1),
                            signal: None,
                        };
                        self.last_exit = Some(exit.clone());
                        self.update_prompt_state();
                        return Ok(exit);
                    }
                }

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

                    // Record interactive execution ONCE: via daemon client if connected, else local db
                    if let Some(ref c) = self.client
                        && c.is_connected()
                    {
                        let client_c = c.clone();
                        let cmd = resolved_argv.join(" ");
                        let exit_code = exit.code;
                        if let Ok(handle) = tokio::runtime::Handle::try_current() {
                            handle.spawn(async move {
                                let _ = client_c
                                    .record_history(cmd, exit_code, duration_ms as u64, None, None)
                                    .await;
                            });
                        }
                    } else if let Some(db_ref) = &mut self.db {
                        let now = chrono::Utc::now().to_rfc3339();
                        let exec_id = omen_core::ExecutionId::generate();
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

                // 2. If daemon client is connected, route execution through shared daemon broker
                if let Some(ref c) = self.client
                    && c.is_connected()
                {
                    let tool = "exec";
                    let operation = "";
                    let client_clone = c.clone();
                    let argv_clone = resolved_argv.clone();
                    let cwd_str = self.cwd.to_string_lossy().to_string();

                    print!(
                        "{}",
                        omen_ui::SemanticBlock::osc133_command_executed(&self.caps)
                    );

                    let summary_res = block_on_async(
                        client_clone.submit_execution(tool, operation, argv_clone, cwd_str, 60000),
                    )
                    .map_err(|e| CoreError::Internal(format!("Daemon execution error: {e}")));

                    match summary_res {
                        Ok(summary) => {
                            print!(
                                "{}",
                                omen_ui::SemanticBlock::osc133_command_finished(
                                    summary.exit_code.unwrap_or(0),
                                    &self.caps
                                )
                            );

                            if !summary.stdout_preview.is_empty() {
                                print!("{}", summary.stdout_preview);
                            }
                            if !summary.stderr_preview.is_empty() {
                                eprint!("{}", summary.stderr_preview);
                            }

                            let exit = ProcessExit {
                                code: summary.exit_code,
                                signal: None,
                            };
                            self.last_exit = Some(exit.clone());
                            self.update_prompt_state();
                            return Ok(exit);
                        }
                        Err(e) => {
                            eprintln!("Execution error via shared daemon broker: {e}");
                            // Fall through to standalone execution if daemon fails
                        }
                    }
                }

                // 3. Standalone execution via ProcessSupervisor
                let req = ExecutionRequest {
                    argv: resolved_argv.clone(),
                    cwd: self.cwd.clone(),
                    env: vec![],
                    stdin_mode: StdioMode::Closed,
                    stdin_payload: None,
                    timeout_ms: 60000,
                    inline_budget: 65536,
                    required_assurance: RequiredAssurance::default(),
                    secrets: vec![],
                };

                print!(
                    "{}",
                    omen_ui::SemanticBlock::osc133_command_executed(&self.caps)
                );

                let output = block_on_async(self.supervisor.execute(req))?;

                print!(
                    "{}",
                    omen_ui::SemanticBlock::osc133_command_finished(
                        output.process_exit.code.unwrap_or(0),
                        &self.caps
                    )
                );

                let exec_id = omen_core::ExecutionId::generate();

                // Compute CAS artifacts and record execution in subordinate physical history
                let cas_dir =
                    omen_knowledge::resolve_workspace_dir(&self.workspace_root).join("cas");
                let cas = omen_knowledge::ContentAddressedStore::new(cas_dir);
                if let Some(db_ref) = &mut self.db {
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
                        stdout_artifact: out_art,
                        stderr_artifact: err_art,
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

        let dirty_count = if self
            .client
            .as_ref()
            .map(|c| c.is_connected())
            .unwrap_or(false)
        {
            if let Ok(ctx) = self.comp_ctx.lock() {
                ctx.hot_index
                    .active_facts
                    .iter()
                    .filter(|f| f.validity == omen_core::ValidityState::Dirty)
                    .count()
            } else {
                0
            }
        } else if let Some(db) = &self.db {
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

    /// Evaluates whether an agent action is permitted without secondary approval ceremony.
    /// Follows default-closed doctrine: natural-language intent and agent output NEVER grant authority.
    /// Narrowly permits verified read-only or non-destructive verification operations only.
    pub fn is_permitted_agent_action(action: &omen_agent::ProposedAction) -> bool {
        match action {
            omen_agent::ProposedAction::ChangeDirectory { .. } => true,
            omen_agent::ProposedAction::ExecuteTool {
                tool,
                operation,
                args,
                ..
            } => match tool.as_str() {
                "cargo" => {
                    let op = if operation.is_empty() {
                        args.first().map(|s| s.as_str()).unwrap_or("")
                    } else {
                        operation.as_str()
                    };
                    matches!(op, "check" | "test")
                }
                "git" => {
                    let op = if operation.is_empty() {
                        args.first().map(|s| s.as_str()).unwrap_or("")
                    } else {
                        operation.as_str()
                    };
                    if matches!(op, "status" | "diff" | "log") {
                        !Self::has_destructive_git_args(args)
                    } else {
                        false
                    }
                }
                "fs" => {
                    // Only read-only operations permitted. File mutations/deletions are blocked.
                    matches!(operation.as_str(), "read" | "stat" | "list")
                }
                "exec" => {
                    if let Some(cmd) = args.first() {
                        Self::is_permitted_executable(cmd, &args[1..])
                    } else {
                        false
                    }
                }
                _ => false, // Default closed
            },
            omen_agent::ProposedAction::ExecuteCommand { argv, .. } => {
                if let Some(cmd) = argv.first() {
                    Self::is_permitted_executable(cmd, &argv[1..])
                } else {
                    false
                }
            }
            omen_agent::ProposedAction::SemanticAction { action, .. } => {
                // Only informational/read-only semantic actions permitted
                matches!(
                    action.as_str(),
                    "status" | "why" | "show" | "inspect" | "history"
                )
            }
        }
    }

    fn is_permitted_executable(cmd: &str, args: &[String]) -> bool {
        let bin_name = std::path::Path::new(cmd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(cmd)
            .to_lowercase();
        let clean_bin = bin_name.strip_suffix(".exe").unwrap_or(&bin_name);

        match clean_bin {
            "cargo" => {
                let sub = args
                    .iter()
                    .find(|a| !a.starts_with('-'))
                    .map(|s| s.as_str());
                matches!(sub, Some("check" | "test"))
            }
            "git" => {
                let sub = args
                    .iter()
                    .find(|a| !a.starts_with('-'))
                    .map(|s| s.as_str());
                if let Some(subcmd) = sub {
                    if matches!(subcmd, "status" | "diff" | "log") {
                        !Self::has_destructive_git_args(args)
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            _ => false, // Default closed: arbitrary exec (rm, sh, bash, powershell, python, curl, etc.) is refused
        }
    }

    fn has_destructive_git_args(args: &[String]) -> bool {
        for a in args {
            let lower = a.to_lowercase();
            if lower == "reset"
                || lower == "clean"
                || lower == "push"
                || lower == "branch"
                || lower == "checkout"
                || lower == "rebase"
                || lower == "rm"
                || lower == "--hard"
                || lower == "-fd"
                || lower == "-df"
                || lower == "--force"
                || lower == "-f"
                || lower == "-d"
                || lower == "--delete"
            {
                return true;
            }
        }
        false
    }

    /// Executes an operation through the shared daemon broker when connected, or local supervisor if standalone.
    pub fn execute_via_broker(
        &mut self,
        tool: &str,
        operation: &str,
        argv: Vec<String>,
        cwd: Option<PathBuf>,
    ) -> Result<ProcessExit, CoreError> {
        let exec_cwd = cwd.unwrap_or_else(|| self.cwd.clone());

        if let Some(ref c) = self.client
            && c.is_connected()
        {
            let client_clone = c.clone();
            let cwd_str = exec_cwd.to_string_lossy().to_string();

            print!(
                "{}",
                omen_ui::SemanticBlock::osc133_command_executed(&self.caps)
            );

            let summary_res = block_on_async(
                client_clone.submit_execution(tool, operation, argv, cwd_str, 60000),
            )
            .map_err(|e| CoreError::Internal(format!("Daemon execution error: {e}")))?;

            print!(
                "{}",
                omen_ui::SemanticBlock::osc133_command_finished(
                    summary_res.exit_code.unwrap_or(0),
                    &self.caps
                )
            );

            if !summary_res.stdout_preview.is_empty() {
                print!("{}", summary_res.stdout_preview);
            }
            if !summary_res.stderr_preview.is_empty() {
                eprint!("{}", summary_res.stderr_preview);
            }

            let exit = ProcessExit {
                code: summary_res.exit_code,
                signal: None,
            };
            self.last_exit = Some(exit.clone());
            self.update_prompt_state();
            return Ok(exit);
        }

        // Standalone execution: run via supervisor and record into self.db with CAS
        let mut full_argv = Vec::new();
        if !tool.is_empty() && tool != "exec" {
            full_argv.push(tool.to_string());
        }
        if !operation.is_empty() {
            full_argv.push(operation.to_string());
        }
        full_argv.extend(argv);
        if full_argv.is_empty() {
            return Ok(ProcessExit {
                code: Some(0),
                signal: None,
            });
        }

        let req = ExecutionRequest {
            argv: full_argv.clone(),
            cwd: exec_cwd,
            env: vec![],
            stdin_mode: StdioMode::Closed,
            stdin_payload: None,
            timeout_ms: 60000,
            inline_budget: 65536,
            required_assurance: RequiredAssurance::default(),
            secrets: vec![],
        };

        print!(
            "{}",
            omen_ui::SemanticBlock::osc133_command_executed(&self.caps)
        );

        let output = block_on_async(self.supervisor.execute(req))?;

        print!(
            "{}",
            omen_ui::SemanticBlock::osc133_command_finished(
                output.process_exit.code.unwrap_or(0),
                &self.caps
            )
        );

        let exec_id = omen_core::ExecutionId::generate();

        let cas_dir = omen_knowledge::resolve_workspace_dir(&self.workspace_root).join("cas");
        let cas = omen_knowledge::ContentAddressedStore::new(cas_dir);
        if let Some(db_ref) = &mut self.db {
            let out_art = if !output.stdout_all.is_empty() {
                cas.store(
                    db_ref,
                    &output.stdout_all,
                    "text/plain",
                    "agent",
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
                    "agent",
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
                command: full_argv.join(" "),
                exit_code: output.process_exit.code,
                duration_ms: Some(output.duration_ms as i64),
                stdout_artifact: out_art,
                stderr_artifact: err_art,
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

        print!("{}", String::from_utf8_lossy(&output.stdout_all));
        eprint!("{}", String::from_utf8_lossy(&output.stderr_all));

        self.last_exit = Some(output.process_exit.clone());
        self.update_prompt_state();
        Ok(output.process_exit)
    }
}
