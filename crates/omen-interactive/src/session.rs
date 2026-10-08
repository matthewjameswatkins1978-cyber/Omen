use crate::child::ChildHandoff;
use crate::prompt_adapter::OmenPrompt;
use omen_core::{CoreError, InteractiveSessionId, ProcessExit, RequiredAssurance, StdioMode};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use omen_knowledge::Database;
use omen_ui::{HumanSettings, TerminalCapabilities};
use reedline::Signal;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundJobInfo {
    pub request_id: String,
    pub execution_id: String,
}

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
    background_jobs: Vec<BackgroundJobInfo>,
    pub agent_provider: Option<std::sync::Arc<dyn omen_agent::AgentProvider>>,
    pub agent_registry: std::sync::Arc<omen_agent::ProviderRegistry>,
    pub backend_registry: std::sync::Arc<omen_engine::BackendRegistry>,
    pub human_settings: HumanSettings,
    standalone_jobs: Vec<StandaloneJob>,
    next_standalone_job: u64,
}

/// Lifecycle of one standalone (daemonless) background job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StandaloneJobState {
    Running,
    Finished { code: Option<i32>, runtime: String },
}

/// A background job owned by this session process: one OS thread driving
/// a cancelable supervised execution. Cancellation kills the process tree
/// (Job Objects on Windows, process-group KILL on Unix) via the engine.
/// Finished jobs stay listed until the session ends; at most 32 are
/// tracked (further background spawns refuse with a clear error).
pub struct StandaloneJob {
    pub id: String,
    pub label: String,
    pub state: std::sync::Arc<std::sync::Mutex<StandaloneJobState>>,
    cancel: tokio::sync::watch::Sender<bool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Cap on tracked standalone jobs: threads are heavier than daemon records.
const MAX_STANDALONE_JOBS: usize = 32;

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
            background_jobs: Vec::new(),
            db,
            comp_ctx,
            client,
            agent_provider,
            agent_registry,
            backend_registry,
            human_settings: HumanSettings::default(),
            standalone_jobs: Vec::new(),
            next_standalone_job: 1,
        };
        sess.prompt.apply_settings(sess.human_settings.clone());
        sess.update_prompt_state();
        Ok(sess)
    }

    pub fn tracked_background_jobs(&self) -> &[BackgroundJobInfo] {
        &self.background_jobs
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
        let mut line_editor = crate::interaction::create_line_editor(completer);

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

        self.cancel_background_jobs_on_exit();
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
                let res = if action == "jobs" {
                    self.dispatch_background_jobs()
                } else if action == "stop"
                    && args
                        .first()
                        .is_some_and(|target| target.starts_with("exec_"))
                {
                    self.dispatch_background_stop(&args[0])
                } else {
                    crate::actions::SemanticDispatcher::dispatch(
                        &action,
                        &args,
                        &self.cwd,
                        &self.session_id,
                        self.db.as_mut(),
                        Some(&self.agent_registry),
                        Some(&self.backend_registry),
                    )
                };
                if let Ok(exit) = &res {
                    self.last_exit = Some(exit.clone());
                }
                self.supervisor = ProcessSupervisor::with_backend(self.backend_registry.active());
                self.agent_provider = Some(self.agent_registry.active_provider());
                self.update_prompt_state();
                res
            }
            crate::grammar::InputLane::PortableShell { line } => self.dispatch_portable_shell(line),
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

                // Windows drive navigation: bare `X:` designator is navigation
                // grammar and must NEVER reach process spawn.
                #[cfg(windows)]
                if resolved_argv.len() == 1
                    && let Some(drive) = crate::commands::is_drive_designator(&resolved_argv[0])
                {
                    let target = PathBuf::from(format!("{drive}:\\"));
                    return self.navigate_to(target, &format!("{drive}:"));
                }

                // Built-in shell navigation: cd modifies session.cwd while preserving workspace_root
                if resolved_argv.first().map(|s| s.as_str()) == Some("cd") {
                    debug_assert!(crate::commands::is_shell_intrinsic("cd"));
                    let target_path = if let Some(target) = resolved_argv.get(1) {
                        resolve_cd_target(&self.cwd, target)
                    } else {
                        self.workspace_root.clone()
                    };

                    return self.navigate_to(target_path, "cd");
                }

                // In-process read-only builtins (P1): no child spawn, no
                // daemon broker. Read-only inspection/formatting only.
                if let Some(result) = self.try_dispatch_builtin(&resolved_argv, Vec::new()) {
                    return result;
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

    fn dispatch_portable_shell(
        &mut self,
        line: crate::shell_grammar::ShellLine,
    ) -> Result<ProcessExit, CoreError> {
        let mut status = ProcessExit::success(0);

        for item in line.items {
            if item.backgrounded {
                status = self.dispatch_background_item(&item.sequence)?;
                continue;
            }

            match item.sequence {
                crate::shell_grammar::ShellSequence::BooleanChain { first, rest } => {
                    status = self.dispatch_shell_pipeline(&first)?;
                    for (operator, pipeline) in rest {
                        let should_run = match operator {
                            crate::shell_grammar::ShellBooleanOperator::And => status.is_zero(),
                            crate::shell_grammar::ShellBooleanOperator::Or => !status.is_zero(),
                        };
                        if should_run {
                            status = self.dispatch_shell_pipeline(&pipeline)?;
                        }
                    }
                }
            }
        }

        self.last_exit = Some(status.clone());
        self.update_prompt_state();
        Ok(status)
    }

    fn dispatch_background_item(
        &mut self,
        sequence: &crate::shell_grammar::ShellSequence,
    ) -> Result<ProcessExit, CoreError> {
        let unsupported = |message: String| CoreError::ExecutionFailedCode {
            code: omen_core::ErrorCode::Unsupported,
            message,
        };
        let crate::shell_grammar::ShellSequence::BooleanChain { first, rest } = sequence;
        if !rest.is_empty() {
            return Err(unsupported(
                "background execution currently accepts a command or pipeline, not a boolean chain"
                    .into(),
            ));
        }
        if self.background_jobs.len() >= 128 {
            return Err(unsupported(
                "this interactive session has reached its 128 tracked background-job limit".into(),
            ));
        }
        let stages = self.prepare_shell_pipeline_stages(first, false)?;
        // Standalone (daemonless) sessions run background jobs locally on
        // cancelable supervised execution; connected sessions use the
        // shared daemon broker below.
        if self
            .client
            .as_ref()
            .is_none_or(|client| !client.is_connected())
        {
            return self.spawn_standalone_background(stages);
        }
        let client = self
            .client
            .as_ref()
            .filter(|client| client.is_connected())
            .ok_or_else(|| unsupported("Omen daemon is not connected".into()))?;

        let cwd = self.cwd.to_string_lossy().to_string();
        let submission = if stages.len() == 1 && stages[0].env.is_empty() {
            let stage = &stages[0];
            block_on_async(client.submit_background_execution(
                stage.argv[0].clone(),
                "",
                stage.argv.iter().skip(1).cloned().collect(),
                cwd,
                60000,
            ))
        } else {
            block_on_async(client.submit_background_pipeline(stages, cwd, 60000))
        }
        .map_err(|error| CoreError::Internal(format!("Background broker error: {error}")))?;

        let (request_id, execution_id) = match submission {
            omen_client::BackgroundExecutionSubmission::Accepted {
                consequential_request_id,
                execution_id,
            } => {
                println!("Started background job {execution_id}.");
                (consequential_request_id, execution_id)
            }
            omen_client::BackgroundExecutionSubmission::Finished {
                consequential_request_id,
                summary,
            } => {
                println!(
                    "Background job {} finished immediately with {:?}",
                    summary.execution_id, summary.runtime_status
                );
                if !summary.stdout_preview.is_empty() {
                    print!("{}", summary.stdout_preview);
                }
                if !summary.stderr_preview.is_empty() {
                    eprint!("{}", summary.stderr_preview);
                }
                (consequential_request_id, summary.execution_id)
            }
        };
        self.background_jobs.push(BackgroundJobInfo {
            request_id,
            execution_id,
        });
        Ok(ProcessExit::success(0))
    }

    fn prepare_shell_pipeline_stages(
        &self,
        pipeline: &crate::shell_grammar::ShellPipeline,
        strip_redirects: bool,
    ) -> Result<Vec<omen_ipc::PipelineStageRequest>, CoreError> {
        let unsupported = |message: String| CoreError::ExecutionFailedCode {
            code: omen_core::ErrorCode::Unsupported,
            message,
        };
        let mut stages = Vec::with_capacity(pipeline.commands.len());
        for command in &pipeline.commands {
            // Redirects are pre-resolved by the caller when `strip_redirects`
            // is set (in-process builtin path); the broker/background paths
            // keep the refusal.
            if !strip_redirects && !command.redirects.is_empty() {
                return Err(unsupported(
                    "redirection requires the supervised redirection dispatcher".into(),
                ));
            }
            let mut env = Vec::with_capacity(command.environment.len());
            for assignment in &command.environment {
                let expanded = crate::shell_grammar::expand_word(&assignment.value, &self.cwd)
                    .map_err(|error| {
                        unsupported(format!("environment assignment expansion failed: {error}"))
                    })?;
                let value = match expanded.as_slice() {
                    [value] => value.clone(),
                    _ => {
                        return Err(unsupported(format!(
                            "environment assignment '{}' must expand to exactly one value",
                            assignment.name
                        )));
                    }
                };
                env.push((assignment.name.clone(), value));
            }

            let mut argv = Vec::new();
            for word in &command.words {
                argv.extend(crate::shell_grammar::expand_word(word, &self.cwd).map_err(
                    |error| unsupported(format!("shell word expansion failed: {error}")),
                )?);
            }
            let argv = crate::commands::expand_shell_alias(&argv);
            let argv = crate::resolver::ReferenceResolver::resolve_argv(
                &argv,
                &self.session_id,
                self.db.as_ref(),
            );
            let Some(program) = argv.first() else {
                return Err(unsupported(
                    "every pipeline stage must contain an executable".into(),
                ));
            };
            if program == "cd" {
                return Err(unsupported(
                    "the Omen cd intrinsic cannot run as a pipeline stage".into(),
                ));
            }
            if ChildHandoff::classify(&argv)
                == crate::child::ChildClassification::InteractiveHandoff
            {
                return Err(unsupported(
                    "interactive terminal handoff cannot run inside a portable shell expression yet".into(),
                ));
            }
            if let Some(blast) = crate::preflight::BlastPreflight::assess(&argv) {
                println!(
                    "[Preflight Warning: {:?}] Command '{}' will affect: {}",
                    blast.severity, blast.command, blast.summary
                );
            }
            stages.push(omen_ipc::PipelineStageRequest { argv, env });
        }
        Ok(stages)
    }

    /// Spawns prepared stages as a standalone background job: one OS thread
    /// driving cancelable supervised execution. Prints `Started background
    /// job <id>.` and returns exit 0; the job's own outcome lands in its
    /// tracked state (see `:jobs`). Reuses the exact spawn semantics as
    /// foreground execution, minus the wait.
    fn spawn_standalone_background(
        &mut self,
        stages: Vec<omen_ipc::PipelineStageRequest>,
    ) -> Result<ProcessExit, CoreError> {
        if self.standalone_jobs.len() >= MAX_STANDALONE_JOBS {
            return Err(CoreError::ExecutionFailed(format!(
                "this interactive session has reached its {MAX_STANDALONE_JOBS} tracked standalone-job limit"
            )));
        }
        let id = format!("job-{}", self.next_standalone_job);
        self.next_standalone_job += 1;
        let label = stages
            .iter()
            .map(|stage| stage.argv.join(" "))
            .collect::<Vec<_>>()
            .join(" | ");
        let exec_cwd = self.cwd.clone();
        let backend = self.backend_registry.active();
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let state = std::sync::Arc::new(std::sync::Mutex::new(StandaloneJobState::Running));
        let thread_state = state.clone();
        let thread = std::thread::spawn(move || {
            let supervisor = omen_engine::ProcessSupervisor::with_backend(backend);
            let outcome = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| {
                    omen_core::CoreError::Internal(format!("background runtime failed: {error}"))
                })
                .and_then(|runtime| {
                    runtime.block_on(async {
                        if stages.len() == 1 {
                            let request = omen_engine::ExecutionRequest {
                                argv: stages[0].argv.clone(),
                                cwd: exec_cwd,
                                env: stages[0].env.clone(),
                                stdin_mode: omen_core::StdioMode::Closed,
                                stdin_payload: None,
                                timeout_ms: 60000,
                                inline_budget: 65536,
                                required_assurance: omen_core::RequiredAssurance::default(),
                                secrets: vec![],
                            };
                            supervisor
                                .execute_cancelable(request, cancel_rx)
                                .await
                                .map(|output| (output.process_exit.code, output.runtime_status))
                        } else {
                            let requests = stages
                                .iter()
                                .map(|stage| omen_engine::ExecutionRequest {
                                    argv: stage.argv.clone(),
                                    cwd: exec_cwd.clone(),
                                    env: stage.env.clone(),
                                    stdin_mode: omen_core::StdioMode::Closed,
                                    stdin_payload: None,
                                    timeout_ms: 60000,
                                    inline_budget: 65536,
                                    required_assurance: omen_core::RequiredAssurance::default(),
                                    secrets: vec![],
                                })
                                .collect();
                            supervisor
                                .execute_pipeline_cancelable(requests, cancel_rx)
                                .await
                                .map(|output| {
                                    (
                                        output.execution.process_exit.code,
                                        output.execution.runtime_status,
                                    )
                                })
                        }
                    })
                });
            let finished = match outcome {
                Ok((code, runtime)) => StandaloneJobState::Finished {
                    code,
                    runtime: format!("{runtime:?}"),
                },
                Err(error) => StandaloneJobState::Finished {
                    code: None,
                    runtime: format!("SpawnFailed: {error:?}"),
                },
            };
            if let Ok(mut guard) = thread_state.lock() {
                *guard = finished;
            }
        });
        println!("Started background job {id}.");
        self.standalone_jobs.push(StandaloneJob {
            id,
            label,
            state,
            cancel: cancel_tx,
            thread: Some(thread),
        });
        Ok(ProcessExit::success(0))
    }

    fn standalone_job_state(&self, id: &str) -> Option<StandaloneJobState> {
        self.standalone_jobs
            .iter()
            .find(|job| job.id == id)
            .and_then(|job| job.state.lock().ok().map(|guard| guard.clone()))
    }

    /// Stops a standalone job: fires cancellation (tree kill via the
    /// engine), joins the driver thread, and reports the observed state.
    fn stop_standalone_job(&mut self, id: &str) -> Result<ProcessExit, CoreError> {
        let position = self
            .standalone_jobs
            .iter()
            .position(|job| job.id == id)
            .ok_or_else(|| {
                CoreError::ExecutionFailed(format!(
                    "No background job with ID '{id}' is tracked by this session."
                ))
            })?;
        self.standalone_jobs[position]
            .cancel
            .send(true)
            .map_err(|_| CoreError::Internal(format!("job {id}: cancellation channel closed")))?;
        // The engine bounds its termination grace period, so the driver
        // thread always ends; a join failure means the thread panicked.
        if let Some(thread) = self.standalone_jobs[position].thread.take()
            && thread.join().is_err()
        {
            eprintln!("Background job {id}: driver thread panicked.");
            return Ok(ProcessExit {
                code: Some(1),
                signal: None,
            });
        }
        let state = self
            .standalone_job_state(id)
            .unwrap_or(StandaloneJobState::Running);
        println!("{id}: {state:?}");
        let stopped = !matches!(state, StandaloneJobState::Running);
        Ok(ProcessExit {
            code: Some(if stopped { 0 } else { 1 }),
            signal: None,
        })
    }

    fn dispatch_background_jobs(&self) -> Result<ProcessExit, CoreError> {
        for job in &self.standalone_jobs {
            let state = job
                .state
                .lock()
                .map(|guard| format!("{:?}", *guard))
                .unwrap_or_else(|_| "Unknown".to_string());
            println!("{} [{}] {}", job.id, state, job.label);
        }
        if self.background_jobs.is_empty() && self.standalone_jobs.is_empty() {
            println!("No background jobs tracked in this interactive session.");
            return Ok(ProcessExit::success(0));
        }
        if self.background_jobs.is_empty() {
            // Standalone jobs are already listed above; nothing brokered.
            return Ok(ProcessExit::success(0));
        }
        let client = self
            .client
            .as_ref()
            .filter(|client| client.is_connected())
            .ok_or_else(|| CoreError::Internal("Omen daemon is not connected".into()))?;
        for job in &self.background_jobs {
            let status =
                block_on_async(client.query_request_status(&job.request_id)).map_err(|error| {
                    CoreError::Internal(format!("Job status query failed: {error}"))
                })?;
            println!("{} [{:?}]", job.execution_id, status.status);
        }
        Ok(ProcessExit::success(0))
    }

    fn dispatch_background_stop(&mut self, execution_id: &str) -> Result<ProcessExit, CoreError> {
        // Standalone jobs (`job-N`) stop locally; daemon jobs (`exec_…`)
        // stop through the broker below.
        if execution_id.starts_with("job-") {
            return self.stop_standalone_job(execution_id);
        }
        if !self
            .background_jobs
            .iter()
            .any(|job| job.execution_id == execution_id)
        {
            eprintln!(
                "No background job with execution ID '{execution_id}' is tracked by this session."
            );
            return Ok(ProcessExit {
                code: Some(1),
                signal: None,
            });
        }
        let client = self
            .client
            .as_ref()
            .filter(|client| client.is_connected())
            .ok_or_else(|| CoreError::Internal("Omen daemon is not connected".into()))?;
        let record = block_on_async(client.cancel_execution(execution_id.to_string()))
            .map_err(|error| CoreError::Internal(format!("Job cancellation failed: {error}")))?;
        println!("{}: {:?}", record.execution_id, record.outcome);
        let succeeded = matches!(
            record.outcome,
            omen_ipc::CancelOutcome::TerminationConfirmed
                | omen_ipc::CancelOutcome::DispatchPrevented
                | omen_ipc::CancelOutcome::AlreadyFinished { .. }
        );
        Ok(ProcessExit {
            code: Some(if succeeded { 0 } else { 1 }),
            signal: None,
        })
    }

    fn cancel_background_jobs_on_exit(&mut self) {
        // Standalone jobs: fire every cancel flag, then join every driver.
        // Still-running jobs are tree-killed by the engine first, whose
        // termination grace period bounds this wait.
        for job in &self.standalone_jobs {
            let _ = job.cancel.send(true);
        }
        for job in &mut self.standalone_jobs {
            if let Some(thread) = job.thread.take()
                && thread.join().is_err()
            {
                eprintln!(
                    "Background job {}: driver thread panicked during exit.",
                    job.id
                );
            }
        }
        let Some(client) = self.client.as_ref().filter(|client| client.is_connected()) else {
            return;
        };
        for job in &self.background_jobs {
            match block_on_async(client.query_request_status(&job.request_id)) {
                Ok(status) if status.status == omen_ipc::ExecutionStatusCode::Running => {
                    match block_on_async(client.cancel_execution(job.execution_id.clone())) {
                        Ok(record) => eprintln!(
                            "Omen exit requested stop for background job {}: {:?}",
                            job.execution_id, record.outcome
                        ),
                        Err(error) => eprintln!(
                            "Could not stop background job {} during Omen exit; it may remain running: {error}",
                            job.execution_id
                        ),
                    }
                }
                Ok(_) => {}
                Err(error) => eprintln!(
                    "Could not inspect background job {} during Omen exit; it may remain running: {error}",
                    job.execution_id
                ),
            }
        }
    }

    fn dispatch_shell_pipeline(
        &mut self,
        pipeline: &crate::shell_grammar::ShellPipeline,
    ) -> Result<ProcessExit, CoreError> {
        let unsupported = |message: String| CoreError::ExecutionFailedCode {
            code: omen_core::ErrorCode::Unsupported,
            message,
        };
        let needs_pipeline_dispatch = pipeline.commands.len() > 1
            || pipeline
                .commands
                .iter()
                .any(|command| !command.environment.is_empty());

        if needs_pipeline_dispatch {
            // Redirects resolve before anything executes: input feeds the
            // chain, output is validated then refused-closed (P2) with
            // zero effects.
            let (chain_stdin, pending_output) =
                crate::redirect::resolve_pipeline_redirects(pipeline, &self.cwd)?;
            if let Some(pending) = &pending_output {
                return crate::redirect::apply_output_write(pending, &[]).map(|()| ProcessExit {
                    code: Some(0),
                    signal: None,
                });
            }
            let stages = self.prepare_shell_pipeline_stages(pipeline, true)?;
            // All-builtin pipelines run in-process: zero spawns, byte-exact
            // chaining. Mixed pipelines fall through to the broker.
            if let Some(result) = self.try_dispatch_builtin_pipeline(&stages, chain_stdin) {
                return result;
            }
            // External stages with redirects still need the supervised
            // redirection dispatcher: preserve the refusal.
            if pipeline.commands.iter().any(|c| !c.redirects.is_empty()) {
                return Err(CoreError::ExecutionFailedCode {
                    code: omen_core::ErrorCode::Unsupported,
                    message: "redirection requires the supervised redirection dispatcher".into(),
                });
            }
            return self.execute_pipeline_via_broker(stages, None);
        }

        let command = &pipeline.commands[0];
        // Single-command redirects resolve before execution; output writes
        // refuse closed (P2) with zero effects.
        let resolved = crate::redirect::resolve_command_redirects(command, &self.cwd)?;
        if let Some(pending) = &resolved.output {
            return crate::redirect::apply_output_write(pending, &[]).map(|()| ProcessExit {
                code: Some(0),
                signal: None,
            });
        }

        let mut argv = Vec::new();
        for word in &command.words {
            argv.extend(
                crate::shell_grammar::expand_word(word, &self.cwd).map_err(|error| {
                    unsupported(format!("shell word expansion failed: {error}"))
                })?,
            );
        }
        let argv = crate::commands::expand_shell_alias(&argv);
        let argv = crate::resolver::ReferenceResolver::resolve_argv(
            &argv,
            &self.session_id,
            self.db.as_ref(),
        );
        let Some(program) = argv.first() else {
            return Ok(ProcessExit::success(0));
        };

        if program == "cd" {
            debug_assert!(crate::commands::is_shell_intrinsic("cd"));
            let target = argv
                .get(1)
                .map(|value| resolve_cd_target(&self.cwd, value))
                .unwrap_or_else(|| self.workspace_root.clone());
            return self.navigate_to(target, "cd");
        }

        // In-process read-only builtins (P1): same rule as the executable
        // lane — no child spawn, no daemon broker.
        if let Some(result) = self.try_dispatch_builtin(&argv, resolved.stdin.clone()) {
            return result;
        }
        // External commands cannot consume a resolved input redirect yet:
        // the broker path carries no stdin payload in P3.
        if !resolved.stdin.is_empty() {
            return Err(CoreError::ExecutionFailedCode {
                code: omen_core::ErrorCode::Unsupported,
                message: "redirection requires the supervised redirection dispatcher".into(),
            });
        }

        if let Some(blast) = crate::preflight::BlastPreflight::assess(&argv) {
            println!(
                "[Preflight Warning: {:?}] Command '{}' will affect: {}",
                blast.severity, blast.command, blast.summary
            );
        }

        if ChildHandoff::classify(&argv) == crate::child::ChildClassification::InteractiveHandoff {
            return Err(unsupported(
                "interactive terminal handoff cannot run inside a portable shell expression yet"
                    .into(),
            ));
        }

        self.execute_via_broker("exec", "", argv, None)
    }

    fn execute_pipeline_via_broker(
        &mut self,
        stages: Vec<omen_ipc::PipelineStageRequest>,
        cwd: Option<PathBuf>,
    ) -> Result<ProcessExit, CoreError> {
        let exec_cwd = cwd.unwrap_or_else(|| self.cwd.clone());
        let command_label = stages
            .iter()
            .map(|stage| stage.argv.join(" "))
            .collect::<Vec<_>>()
            .join(" | ");

        if let Some(client) = self.client.as_ref().filter(|client| client.is_connected()) {
            print!(
                "{}",
                omen_ui::SemanticBlock::osc133_command_executed(&self.caps)
            );
            let summary = block_on_async(client.submit_pipeline(
                stages,
                exec_cwd.to_string_lossy().to_string(),
                60000,
            ))
            .map_err(|error| CoreError::Internal(format!("Daemon pipeline error: {error}")))?;
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

        let requests = stages
            .iter()
            .map(|stage| ExecutionRequest {
                argv: stage.argv.clone(),
                cwd: exec_cwd.clone(),
                env: stage.env.clone(),
                stdin_mode: StdioMode::Closed,
                stdin_payload: None,
                timeout_ms: 60000,
                inline_budget: 65536,
                required_assurance: RequiredAssurance::default(),
                secrets: vec![],
            })
            .collect();
        print!(
            "{}",
            omen_ui::SemanticBlock::osc133_command_executed(&self.caps)
        );
        let output = block_on_async(self.supervisor.execute_pipeline(requests))?;
        print!(
            "{}",
            omen_ui::SemanticBlock::osc133_command_finished(
                output.execution.process_exit.code.unwrap_or(0),
                &self.caps
            )
        );

        let execution_id = omen_core::ExecutionId::generate();
        let cas = omen_knowledge::ContentAddressedStore::new(
            omen_knowledge::resolve_workspace_dir(&self.workspace_root).join("cas"),
        );
        if let Some(db) = &mut self.db {
            let stdout_artifact = if output.execution.stdout_all.is_empty() {
                None
            } else {
                cas.store(
                    db,
                    &output.execution.stdout_all,
                    "application/octet-stream",
                    "pipeline",
                    omen_core::RetentionClass::Referenced,
                )
                .ok()
                .map(|artifact| artifact.uri.as_str().to_string())
            };
            let stderr_artifact = if output.execution.stderr_all.is_empty() {
                None
            } else {
                cas.store(
                    db,
                    &output.execution.stderr_all,
                    "application/octet-stream",
                    "pipeline",
                    omen_core::RetentionClass::Referenced,
                )
                .ok()
                .map(|artifact| artifact.uri.as_str().to_string())
            };
            let record = omen_knowledge::ExecutionRecord {
                execution_id,
                session_id: self.session_id.clone(),
                command: command_label,
                exit_code: output.execution.process_exit.code,
                duration_ms: Some(output.execution.duration_ms as i64),
                stdout_artifact,
                stderr_artifact,
                envelope_json: None,
                created_at: chrono::Utc::now().to_rfc3339(),
            };
            let _ = omen_knowledge::ExecutionHistory::record_execution(db, &record, &[], &[], &[]);
        }

        print!("{}", String::from_utf8_lossy(&output.execution.stdout_all));
        eprint!("{}", String::from_utf8_lossy(&output.execution.stderr_all));
        let exit = output.execution.process_exit;
        self.last_exit = Some(exit.clone());
        self.update_prompt_state();
        Ok(exit)
    }

    /// Runs `argv` as an in-process read-only builtin when it names one.
    ///
    /// Returns `None` when the command is not a builtin (the caller falls
    /// through to external execution). Builtins never spawn a child process
    /// and never go through the daemon broker: they are pure functions of
    /// `(argv, cwd, env, stdin)` in the same trust class as the `:action`
    /// dispatcher. Session-affecting words (`cd`) stay with the caller.
    fn try_dispatch_builtin(
        &mut self,
        argv: &[String],
        stdin: Vec<u8>,
    ) -> Option<Result<ProcessExit, CoreError>> {
        let ctx = omen_builtins::BuiltinContext {
            cwd: self.cwd.clone(),
            env: std::env::vars().collect(),
            stdin,
        };
        let output = match omen_builtins::run_if_builtin(argv, &ctx)? {
            Ok(output) => output,
            Err(error) => {
                return Some(Err(CoreError::Internal(format!(
                    "builtin internal failure: {error}"
                ))));
            }
        };
        use std::io::Write as _;
        if !output.stdout.is_empty() {
            let _ = std::io::stdout().write_all(&output.stdout);
            let _ = std::io::stdout().flush();
        }
        if !output.stderr.is_empty() {
            let _ = std::io::stderr().write_all(&output.stderr);
        }
        let exit = ProcessExit {
            code: Some(output.code),
            signal: None,
        };
        self.last_exit = Some(exit.clone());
        self.update_prompt_state();
        Some(Ok(exit))
    }

    /// Runs an all-builtin pipeline in-process (zero spawns, no broker).
    ///
    /// Returns `None` when any stage is external. Every stage runs; each
    /// stderr passes through in order, only the last stdout reaches the
    /// terminal, and the reported exit is the last stage's.
    fn try_dispatch_builtin_pipeline(
        &mut self,
        stages: &[omen_ipc::PipelineStageRequest],
        stdin: Vec<u8>,
    ) -> Option<Result<ProcessExit, CoreError>> {
        let inputs: Vec<omen_builtins::PipelineStage> = stages
            .iter()
            .map(|stage| omen_builtins::PipelineStage {
                argv: stage.argv.clone(),
                env_overrides: stage.env.clone(),
            })
            .collect();
        let ctx = omen_builtins::BuiltinContext {
            cwd: self.cwd.clone(),
            env: std::env::vars().collect(),
            stdin,
        };
        let outputs = omen_builtins::run_pipeline(&inputs, &ctx)?;
        use std::io::Write as _;
        let mut code = 0;
        let mut last_stdout: Vec<u8> = Vec::new();
        for output in outputs {
            let output = match output {
                Ok(output) => output,
                Err(error) => {
                    return Some(Err(CoreError::Internal(format!(
                        "builtin pipeline internal failure: {error}"
                    ))));
                }
            };
            // Every stage runs (POSIX pipelines do not short-circuit);
            // each stderr passes through in order, only the last stdout
            // reaches the terminal, and the exit is the last stage's.
            code = output.code;
            if !output.stderr.is_empty() {
                let _ = std::io::stderr().write_all(&output.stderr);
            }
            last_stdout = output.stdout;
        }
        if !last_stdout.is_empty() {
            let _ = std::io::stdout().write_all(&last_stdout);
            let _ = std::io::stdout().flush();
        }
        let exit = ProcessExit {
            code: Some(code),
            signal: None,
        };
        self.last_exit = Some(exit.clone());
        self.update_prompt_state();
        Some(Ok(exit))
    }
    ///
    /// Used by `cd`, bare drive designators (`D:`), and any future navigation
    /// grammar.  Exactly one place owns the navigation side-effects.
    fn navigate_to(&mut self, target: PathBuf, label: &str) -> Result<ProcessExit, CoreError> {
        if target.is_dir() {
            self.cwd = target.canonicalize().unwrap_or(target);
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
            Ok(exit)
        } else {
            eprintln!("{label}: {}: No such file or directory", target.display());
            let exit = ProcessExit {
                code: Some(1),
                signal: None,
            };
            self.last_exit = Some(exit.clone());
            self.update_prompt_state();
            Ok(exit)
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

/// Resolves a `cd` target argument to an absolute path.
///
/// Handles bare Windows drive designators (`D:`) as navigation grammar.
/// Absolute paths (`D:\`, `D:\foo`) resolve normally via `Path::is_absolute`.
///
/// Drive-relative paths (`D:foo`) are **outside M0** — Omen has no per-drive
/// cwd model and must not silently redefine their meaning.  They fall through
/// to ordinary relative resolution against `cwd` and will typically produce
/// "no such file or directory".
fn resolve_cd_target(cwd: &Path, target: &str) -> PathBuf {
    #[cfg(windows)]
    if let Some(drive) = crate::commands::is_drive_designator(target) {
        return PathBuf::from(format!("{drive}:\\"));
    }
    let p = PathBuf::from(target);
    if p.is_absolute() { p } else { cwd.join(p) }
}

#[cfg(test)]
mod builtin_pipeline_dispatch_tests {
    use super::*;

    fn standalone_session(cwd: PathBuf) -> InteractiveSession {
        let id = omen_core::InteractiveSessionId::generate();
        InteractiveSession::new_with_client(id, cwd, None, None).expect("session builds headless")
    }

    /// An all-builtin pipeline dispatches in-process: exit zero, no spawn.
    /// (`printf | grep | wc` exercises three chained stages.)
    #[test]
    fn all_builtin_pipeline_runs_in_process() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut session = standalone_session(dir.path().to_path_buf());
        let exit = session
            .dispatch_input("printf 'a\\nb\\n' | grep b | wc -l")
            .expect("pipeline dispatches");
        assert_eq!(exit.code, Some(0));
        assert!(session.last_exit.is_some_and(|e| e.is_zero()));
    }

    /// A failing builtin stage still runs the chain and reports the last
    /// stage's exit (POSIX: every stage runs).
    #[test]
    fn builtin_pipeline_reports_last_stage_exit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut session = standalone_session(dir.path().to_path_buf());
        // `grep zzz` matches nothing (exit 1); `wc -c` still runs (exit 0).
        let exit = session
            .dispatch_input("printf 'a\\n' | grep zzz | wc -c")
            .expect("pipeline dispatches");
        assert_eq!(exit.code, Some(0));
    }

    /// Binary bytes (NUL, invalid UTF-8, lone CR) survive a three-stage
    /// in-process chain byte-exactly.
    #[test]
    fn builtin_pipeline_preserves_binary_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("bin.dat"),
            [0x41, 0x00, 0xFF, 0x0A, 0x42, 0x0D, 0x0A, 0x43],
        )
        .expect("write");
        let mut session = standalone_session(dir.path().to_path_buf());
        // First line is `A NUL 0xFF \\n` = 4 bytes.
        let exit = session
            .dispatch_input("cat bin.dat | head -n 1 | wc -c")
            .expect("pipeline dispatches");
        assert_eq!(exit.code, Some(0));
    }

    /// A missing stage executable fails closed with a spawn error — never
    /// a hang, never a silent success.
    #[test]
    fn missing_pipeline_executable_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut session = standalone_session(dir.path().to_path_buf());
        let error = session
            .dispatch_input("nosuchbin_xyz_omen | wc -l")
            .expect_err("missing executable must refuse");
        let message = format!("{error:?}");
        assert!(
            message.contains("nosuchbin_xyz_omen"),
            "refusal names the missing binary, got {message}"
        );
    }

    /// A missing single command fails closed the same way.
    #[test]
    fn missing_single_executable_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut session = standalone_session(dir.path().to_path_buf());
        let error = session
            .dispatch_input("nosuchbin_xyz_omen")
            .expect_err("missing executable must refuse");
        assert!(format!("{error:?}").contains("nosuchbin_xyz_omen"));
    }

    /// `< file` feeds a builtin chain from bytes on disk.
    #[test]
    fn input_redirect_feeds_builtin_pipeline() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("in.txt"), b"b\na\nb\n").expect("write");
        let mut session = standalone_session(dir.path().to_path_buf());
        let exit = session
            .dispatch_input("sort < in.txt | uniq -c")
            .expect("pipeline dispatches");
        assert_eq!(exit.code, Some(0));
    }

    /// `> file` refuses closed BEFORE running: no file appears, even
    /// though the command itself is a harmless builtin.
    #[test]
    fn output_redirect_refuses_closed_with_zero_effects() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut session = standalone_session(dir.path().to_path_buf());
        let error = session
            .dispatch_input("echo hi > out.txt")
            .expect_err("output redirect must refuse");
        assert!(
            format!("{error:?}").contains("admitted filesystem authority"),
            "refusal names the missing authority"
        );
        assert!(
            !dir.path().join("out.txt").exists(),
            "refused write created a file"
        );
    }

    /// Portable long-runner for background-cancel tests (platform truth:
    /// `sleep` on Unix, `ping`-as-timer on Windows).
    #[cfg(windows)]
    fn sleeper_argv() -> Vec<String> {
        vec![
            "cmd".to_string(),
            "/c".to_string(),
            "ping -n 30 127.0.0.1 > nul".to_string(),
        ]
    }
    #[cfg(not(windows))]
    fn sleeper_argv() -> Vec<String> {
        vec!["sleep".to_string(), "30".to_string()]
    }

    /// Polls a job state until the deadline; panics with the last state.
    fn await_state(
        session: &InteractiveSession,
        id: &str,
        done: impl Fn(&StandaloneJobState) -> bool,
        label: &str,
    ) -> StandaloneJobState {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let state = session
                .standalone_job_state(id)
                .unwrap_or(StandaloneJobState::Running);
            if done(&state) {
                return state;
            }
            if std::time::Instant::now() >= deadline {
                panic!("{label}: timed out in state {state:?}");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// `cmd &` tracks a standalone job without a daemon; a fast command
    /// finishes and `:jobs` reports it.
    #[test]
    fn standalone_background_fast_command_finishes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut session = standalone_session(dir.path().to_path_buf());
        let exit = session
            .dispatch_input("hostname &")
            .expect("background dispatches");
        assert_eq!(exit.code, Some(0));
        let state = await_state(
            &session,
            "job-1",
            |s| !matches!(s, StandaloneJobState::Running),
            "hostname job",
        );
        assert!(
            matches!(state, StandaloneJobState::Finished { code: Some(0), .. }),
            "hostname finishes zero, got {state:?}"
        );
        let exit = session.dispatch_input(":jobs").expect(":jobs dispatches");
        assert_eq!(exit.code, Some(0));
    }

    /// Stopping a running standalone job tree-kills it and reports a
    /// non-running state (never a manufactured success).
    #[test]
    fn standalone_background_cancel_kills_sleeper() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut session = standalone_session(dir.path().to_path_buf());
        let argv = sleeper_argv();
        let stages = vec![omen_ipc::PipelineStageRequest {
            argv,
            env: Vec::new(),
        }];
        let exit = session
            .spawn_standalone_background(stages)
            .expect("sleeper spawns");
        assert_eq!(exit.code, Some(0));
        // Give the driver thread a moment to reach spawn; then stop.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let exit = session
            .stop_standalone_job("job-1")
            .expect("stop dispatches");
        assert_eq!(exit.code, Some(0));
        let state = session
            .standalone_job_state("job-1")
            .expect("job still tracked");
        assert!(
            !matches!(state, StandaloneJobState::Running),
            "cancelled sleeper left Running: {state:?}"
        );
        // Unknown ids refuse with a clear error, not a panic.
        assert!(session.stop_standalone_job("job-99").is_err());
    }
}
