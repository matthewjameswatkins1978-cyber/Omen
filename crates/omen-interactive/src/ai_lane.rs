use omen_agent::{
    AgentContext, AgentError, AgentProvider, AgentRequest, ExecutionSummary, ProposedAction,
};
use omen_core::{CoreError, InteractiveSessionId};
use omen_knowledge::{Database, ExecutionHistory};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AiLaneDispatchStats {
    pub provider_calls: usize,
    pub git_probes: usize,
    pub db_context_queries: usize,
    pub service_scans: usize,
    #[serde(default)]
    pub semantic_provider_calls: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiLaneOutput {
    pub configured: bool,
    pub query: String,
    pub response_text: String,
    pub suggested_commands: Vec<String>,
    pub proposed_actions: Vec<ProposedAction>,
    pub references: Vec<String>,
    #[serde(default)]
    pub stats: AiLaneDispatchStats,
}

pub struct AiLaneDispatcher;

impl AiLaneDispatcher {
    pub fn dispatch(
        query: &str,
        session_id: &InteractiveSessionId,
        db: Option<&Database>,
    ) -> Result<AiLaneOutput, CoreError> {
        Self::dispatch_with_session(query, session_id, Path::new("."), db, None, None)
    }

    pub fn dispatch_with_session(
        query: &str,
        session_id: &InteractiveSessionId,
        cwd: &Path,
        db: Option<&Database>,
        provider: Option<&dyn AgentProvider>,
        comp_ctx: Option<&Arc<Mutex<crate::completion::CompletionContext>>>,
    ) -> Result<AiLaneOutput, CoreError> {
        Self::dispatch_with_workspace(query, session_id, cwd, cwd, db, provider, comp_ctx)
    }

    pub fn dispatch_with_workspace(
        query: &str,
        session_id: &InteractiveSessionId,
        workspace_root: &Path,
        cwd: &Path,
        db: Option<&Database>,
        provider: Option<&dyn AgentProvider>,
        comp_ctx: Option<&Arc<Mutex<crate::completion::CompletionContext>>>,
    ) -> Result<AiLaneOutput, CoreError> {
        let mut stats = AiLaneDispatchStats::default();

        // 1. CHEAP CLASSIFICATION FIRST:
        // Categorize trivial deterministic queries with 0 model calls, 0 Git probes, 0 DB queries, 0 service scans.
        if let Some(kind) = omen_agent::DeterministicClassifier::classify(query) {
            match kind {
                omen_agent::DeterministicKind::CurrentDirectory => {
                    let folder_name = cwd
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| cwd.display().to_string());
                    let msg = format!("You are in folder `{folder_name}` (`{}`).", cwd.display());
                    return Ok(AiLaneOutput {
                        configured: true,
                        query: query.to_string(),
                        response_text: format!("Agent\n\n{msg}"),
                        suggested_commands: vec![":status".to_string()],
                        proposed_actions: Vec::new(),
                        references: Vec::new(),
                        stats,
                    });
                }
                omen_agent::DeterministicKind::FileFocus => {
                    let msg = format!("Current focus is directory `{}`.", cwd.display());
                    return Ok(AiLaneOutput {
                        configured: true,
                        query: query.to_string(),
                        response_text: format!("Agent\n\n{msg}"),
                        suggested_commands: vec![":status".to_string()],
                        proposed_actions: Vec::new(),
                        references: Vec::new(),
                        stats,
                    });
                }
                omen_agent::DeterministicKind::GitBranch => {
                    stats.git_probes += 1;
                    let git = omen_agent::AgentContext::detect_git_status(cwd);
                    let branch = git
                        .as_ref()
                        .and_then(|g| g.branch.as_deref())
                        .unwrap_or("no active branch or not in a git repository");
                    let msg = format!("Active git branch is `{branch}`.");
                    return Ok(AiLaneOutput {
                        configured: true,
                        query: query.to_string(),
                        response_text: format!("Agent\n\n{msg}"),
                        suggested_commands: vec![":status".to_string()],
                        proposed_actions: Vec::new(),
                        references: Vec::new(),
                        stats,
                    });
                }
                omen_agent::DeterministicKind::LastCommand => {
                    stats.db_context_queries += 1;
                    let last_exec = if let Some(db_ref) = db {
                        omen_knowledge::ExecutionHistory::get_last_execution(db_ref, session_id)
                            .ok()
                            .flatten()
                    } else {
                        None
                    };
                    let msg = if let Some(exec) = last_exec {
                        let code = exec
                            .exit_code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "none".into());
                        format!(
                            "The last command was `{}` (exit code {code}).",
                            exec.command
                        )
                    } else {
                        "No previous command recorded in this session.".to_string()
                    };
                    return Ok(AiLaneOutput {
                        configured: true,
                        query: query.to_string(),
                        response_text: format!("Agent\n\n{msg}"),
                        suggested_commands: vec![":status".to_string()],
                        proposed_actions: Vec::new(),
                        references: Vec::new(),
                        stats,
                    });
                }
                omen_agent::DeterministicKind::WorkspaceDirty => {
                    stats.db_context_queries += 1;
                    stats.git_probes += 1;
                    let mut dirty_items = Vec::new();
                    if let Some(db_ref) = db
                        && let Ok(active) =
                            omen_knowledge::FactRegistry::list_active_facts(db_ref, 50)
                    {
                        let dirty_count = active
                            .iter()
                            .filter(|f| f.validity == omen_core::ValidityState::Dirty)
                            .count();
                        if dirty_count > 0 {
                            dirty_items.push(format!("{dirty_count} dirty fact(s)"));
                        }
                    }
                    let git = omen_agent::AgentContext::detect_git_status(cwd);
                    if let Some(ref g) = git {
                        if !g.modified_files.is_empty() {
                            dirty_items
                                .push(format!("{} modified file(s)", g.modified_files.len()));
                        }
                        if !g.untracked_files.is_empty() {
                            dirty_items
                                .push(format!("{} untracked file(s)", g.untracked_files.len()));
                        }
                    }
                    let msg = if dirty_items.is_empty() {
                        "Workspace state is clean. No dirty facts or uncommitted changes detected."
                            .to_string()
                    } else {
                        format!("Workspace has dirty state: {}.", dirty_items.join(", "))
                    };
                    return Ok(AiLaneOutput {
                        configured: true,
                        query: query.to_string(),
                        response_text: format!("Agent\n\n{msg}"),
                        suggested_commands: vec![":status".to_string()],
                        proposed_actions: Vec::new(),
                        references: Vec::new(),
                        stats,
                    });
                }
                omen_agent::DeterministicKind::SymbolDefinition(sym) => {
                    stats.semantic_provider_calls += 1;
                    let reg = crate::semantic_service::build_semantic_registry(workspace_root);
                    let res = run_future_blocking(async {
                        reg.find_definition(&sym, None, None, None, None).await
                    });
                    let (msg, refs) = match res {
                        Ok(omen_semantic::SemanticLookupResult::Resolved(loc)) => (
                            format!(
                                "Symbol `{sym}` is defined at `{}:{}:{}`.",
                                loc.file,
                                loc.range.start_line + 1,
                                loc.range.start_col + 1
                            ),
                            vec![format!("@symbol://{sym}")],
                        ),
                        Ok(omen_semantic::SemanticLookupResult::Stale(loc)) => (
                            format!(
                                "Symbol `{sym}` is defined at `{}:{}:{}` (STALE index — source file has changed).",
                                loc.file,
                                loc.range.start_line + 1,
                                loc.range.start_col + 1
                            ),
                            vec![format!("@symbol://{sym}")],
                        ),
                        Ok(omen_semantic::SemanticLookupResult::Ambiguous(candidates)) => (
                            format!(
                                "Symbol `{sym}` is ambiguous (found {} candidates across workspace).",
                                candidates.len()
                            ),
                            candidates
                                .into_iter()
                                .map(|c| format!("@{}", c.uri.as_str()))
                                .collect(),
                        ),
                        Ok(omen_semantic::SemanticLookupResult::NotFound) => (
                            format!("Symbol `{sym}` was not found in workspace semantic index."),
                            Vec::new(),
                        ),
                        Ok(omen_semantic::SemanticLookupResult::Unsupported) | Err(_) => (
                            format!(
                                "Symbol definition for `{sym}` is unsupported by current semantic providers."
                            ),
                            Vec::new(),
                        ),
                    };

                    return Ok(AiLaneOutput {
                        configured: true,
                        query: query.to_string(),
                        response_text: format!("Agent\n\n{msg}"),
                        suggested_commands: vec![format!(":def {sym}")],
                        proposed_actions: Vec::new(),
                        references: refs,
                        stats,
                    });
                }
                omen_agent::DeterministicKind::SymbolReferences(sym) => {
                    stats.semantic_provider_calls += 1;
                    let reg = crate::semantic_service::build_semantic_registry(workspace_root);
                    let res = run_future_blocking(async {
                        reg.find_references(&sym, None, None, None, Some(50), None)
                            .await
                    });
                    let (msg, refs) = match res {
                        Ok(omen_semantic::SemanticLookupResult::Resolved(r_list)) => (
                            format!("Found {} reference(s) to symbol `{sym}`.", r_list.len()),
                            vec![format!("@symbol://{sym}")],
                        ),
                        Ok(omen_semantic::SemanticLookupResult::Stale(r_list)) => (
                            format!(
                                "Found {} reference(s) to symbol `{sym}` (STALE index — source file has changed).",
                                r_list.len()
                            ),
                            vec![format!("@symbol://{sym}")],
                        ),
                        Ok(omen_semantic::SemanticLookupResult::Ambiguous(candidates)) => (
                            format!(
                                "Symbol `{sym}` is ambiguous ({} candidates).",
                                candidates.len()
                            ),
                            vec![format!("@symbol://{sym}")],
                        ),
                        Ok(omen_semantic::SemanticLookupResult::NotFound) => (
                            format!("No references found for symbol `{sym}`."),
                            Vec::new(),
                        ),
                        Ok(omen_semantic::SemanticLookupResult::Unsupported) | Err(_) => (
                            format!(
                                "Reference lookup for `{sym}` is unsupported by current semantic providers."
                            ),
                            Vec::new(),
                        ),
                    };

                    return Ok(AiLaneOutput {
                        configured: true,
                        query: query.to_string(),
                        response_text: format!("Agent\n\n{msg}"),
                        suggested_commands: vec![format!(":refs {sym}")],
                        proposed_actions: Vec::new(),
                        references: refs,
                        stats,
                    });
                }
                omen_agent::DeterministicKind::PackageQuery(pkg_opt) => {
                    let reg = crate::semantic_service::build_semantic_registry(workspace_root);
                    let pkgs =
                        run_future_blocking(async { reg.packages(None).await }).unwrap_or_default();

                    let (msg, refs) = match pkg_opt {
                        Some(target) => {
                            if let Some(p) = pkgs.iter().find(|p| {
                                p.name == target || p.targets.iter().any(|t| t.name == target)
                            }) {
                                (
                                    format!(
                                        "Target `{target}` belongs to package `{}` (v{}) [{}].",
                                        p.name, p.version, p.ecosystem
                                    ),
                                    vec![format!("@{}", p.uri.as_str())],
                                )
                            } else {
                                (
                                    format!("No package found owning target `{target}`."),
                                    Vec::new(),
                                )
                            }
                        }
                        None => (
                            format!(
                                "Workspace contains {} package(s): {}.",
                                pkgs.len(),
                                pkgs.iter()
                                    .map(|p| p.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                            pkgs.into_iter()
                                .map(|p| format!("@{}", p.uri.as_str()))
                                .collect(),
                        ),
                    };

                    return Ok(AiLaneOutput {
                        configured: true,
                        query: query.to_string(),
                        response_text: format!("Agent\n\n{msg}"),
                        suggested_commands: vec![":packages".to_string()],
                        proposed_actions: Vec::new(),
                        references: refs,
                        stats,
                    });
                }
            }
        }

        // 2. ONLY WHEN REASONING IS ACTUALLY NEEDED:
        // Build bounded AgentContext with necessary probes
        stats.git_probes += 1;
        stats.db_context_queries += 1;
        stats.service_scans += 1;

        let mut suggestions = vec![":status".to_string()];

        let has_failed = if let Some(db_ref) = db {
            matches!(
                ExecutionHistory::get_last_failed_execution(db_ref, session_id),
                Ok(Some(_))
            )
        } else {
            false
        };

        if has_failed {
            suggestions.insert(0, ":show @failed".to_string());
            suggestions.insert(1, ":why @last".to_string());
            suggestions.insert(2, ":rerun @failed".to_string());
        } else {
            suggestions.insert(0, ":inspect @last".to_string());
            suggestions.insert(1, ":history".to_string());
        }

        let context =
            build_agent_context_with_workspace(session_id, workspace_root, cwd, db, comp_ctx);

        // If an AgentProvider is available, evaluate the query using structured AgentContext
        if let Some(p) = provider {
            stats.provider_calls += 1;
            let request = AgentRequest {
                prompt: query.to_string(),
                context,
                conversation: Vec::new(),
            };

            // Invoke provider (wrapped with bounded timeout)
            let result = crate::session::block_on_async(p.respond(request));
            match result {
                Ok(resp) => {
                    let mut formatted = format!("Agent\n\n{}", resp.message);
                    if !resp.proposed_actions.is_empty() {
                        formatted.push_str("\n\nProposed action:");
                        for action in &resp.proposed_actions {
                            match action {
                                ProposedAction::ExecuteTool {
                                    tool,
                                    operation,
                                    args,
                                    cwd,
                                } => {
                                    let in_cwd = cwd
                                        .as_ref()
                                        .map(|c| format!(" (in {c})"))
                                        .unwrap_or_default();
                                    if operation.is_empty() {
                                        formatted.push_str(&format!(
                                            "\n  :{tool} {}{in_cwd}",
                                            args.join(" ")
                                        ));
                                    } else {
                                        formatted.push_str(&format!(
                                            "\n  :{tool} {operation} {}{in_cwd}",
                                            args.join(" ")
                                        ));
                                    }
                                }
                                ProposedAction::ExecuteCommand { argv, cwd } => {
                                    let in_cwd = cwd
                                        .as_ref()
                                        .map(|c| format!(" (in {c})"))
                                        .unwrap_or_default();
                                    formatted.push_str(&format!("\n  {}{in_cwd}", argv.join(" ")));
                                }
                                ProposedAction::SemanticAction { action, args } => {
                                    formatted
                                        .push_str(&format!("\n  :{action} {}", args.join(" ")));
                                }
                                ProposedAction::ChangeDirectory { path } => {
                                    formatted.push_str(&format!("\n  cd {}", path.display()));
                                }
                            }
                        }
                    }

                    // Dynamically map proposed actions to suggested commands
                    let mut cmds = suggestions.clone();
                    for action in &resp.proposed_actions {
                        match action {
                            ProposedAction::ExecuteTool {
                                tool,
                                operation,
                                args,
                                ..
                            } => {
                                if operation.is_empty() {
                                    cmds.insert(0, format!(":{tool} {}", args.join(" ")));
                                } else {
                                    cmds.insert(
                                        0,
                                        format!(":{tool} {operation} {}", args.join(" ")),
                                    );
                                }
                            }
                            ProposedAction::ExecuteCommand { argv, .. } => {
                                cmds.insert(0, argv.join(" "));
                            }
                            ProposedAction::SemanticAction { action, args } => {
                                cmds.insert(0, format!(":{action} {}", args.join(" ")));
                            }
                            ProposedAction::ChangeDirectory { path } => {
                                cmds.insert(0, format!("cd {}", path.display()));
                            }
                        }
                    }

                    return Ok(AiLaneOutput {
                        configured: true,
                        query: query.to_string(),
                        response_text: formatted,
                        suggested_commands: cmds,
                        proposed_actions: resp.proposed_actions,
                        references: resp.references,
                        stats,
                    });
                }
                Err(AgentError::Timeout(d)) => {
                    return Ok(AiLaneOutput {
                        configured: false,
                        query: query.to_string(),
                        response_text: format!(
                            "Agent\n\nAgent request exceeded {d:?}. Omen state is unchanged."
                        ),
                        suggested_commands: suggestions,
                        proposed_actions: Vec::new(),
                        references: Vec::new(),
                        stats,
                    });
                }
                Err(e) => {
                    return Ok(AiLaneOutput {
                        configured: false,
                        query: query.to_string(),
                        response_text: format!("Agent\n\nReasoning unavailable: {e}"),
                        suggested_commands: suggestions,
                        proposed_actions: Vec::new(),
                        references: Vec::new(),
                        stats,
                    });
                }
            }
        }

        // Unconfigured fallback (deterministic machine alternatives)
        let response_text = format!(
            "AI reasoning lane is not configured.\n\nQuery received: \"{}\"\n\nDeterministic alternatives available directly from machine state:",
            query
        );

        Ok(AiLaneOutput {
            configured: false,
            query: query.to_string(),
            response_text,
            suggested_commands: suggestions,
            proposed_actions: Vec::new(),
            references: Vec::new(),
            stats,
        })
    }
}

/// Builds bounded AgentContext from live session state with explicit workspace root.
pub fn build_agent_context(
    session_id: &InteractiveSessionId,
    cwd: &Path,
    db: Option<&Database>,
    comp_ctx: Option<&Arc<Mutex<crate::completion::CompletionContext>>>,
) -> AgentContext {
    build_agent_context_with_workspace(session_id, cwd, cwd, db, comp_ctx)
}

/// Builds bounded AgentContext from live session state with distinct workspace_root and cwd.
pub fn build_agent_context_with_workspace(
    session_id: &InteractiveSessionId,
    workspace_root: &Path,
    cwd: &Path,
    db: Option<&Database>,
    comp_ctx: Option<&Arc<Mutex<crate::completion::CompletionContext>>>,
) -> AgentContext {
    let ws_id = omen_knowledge::deterministic_workspace_id(workspace_root);
    let mut ctx = AgentContext::new(
        &ws_id,
        workspace_root.to_path_buf(),
        session_id.clone(),
        cwd.to_path_buf(),
    );

    // 1. Detect Git status in background CWD (bounded with timeout)
    ctx.git = AgentContext::detect_git_status(cwd);

    // 2. Query execution history from SQLite
    if let Some(db_ref) = db {
        if let Ok(Some(last)) =
            omen_knowledge::ExecutionHistory::get_last_execution(db_ref, session_id)
        {
            ctx.recent_execution = Some(ExecutionSummary {
                execution_id: last.execution_id.to_string(),
                command: last.command,
                exit_code: last.exit_code,
                duration_ms: last.duration_ms.map(|d| d as u64),
                stdout_artifact: last.stdout_artifact,
                stderr_artifact: last.stderr_artifact,
                stdout_excerpt: None,
                stderr_excerpt: None,
                timestamp: last.created_at,
            });
        }
        if let Ok(Some(failed)) =
            omen_knowledge::ExecutionHistory::get_last_failed_execution(db_ref, session_id)
        {
            let stderr_excerpt = failed.stderr_artifact.as_ref().and_then(|uri| {
                let digest = uri.trim_start_matches("artifact://sha256/");
                let state_dir = omen_knowledge::resolve_workspace_dir(workspace_root);
                let cas = omen_knowledge::ContentAddressedStore::new(state_dir.join("cas"));
                let blob = cas.blob_path(digest);
                if let Ok(bytes) = std::fs::read(&blob) {
                    let s = String::from_utf8_lossy(&bytes);
                    Some(AgentContext::bounded_excerpt(&s, 512))
                } else {
                    None
                }
            });

            ctx.recent_failed_execution = Some(ExecutionSummary {
                execution_id: failed.execution_id.to_string(),
                command: failed.command,
                exit_code: failed.exit_code,
                duration_ms: failed.duration_ms.map(|d| d as u64),
                stdout_artifact: failed.stdout_artifact,
                stderr_artifact: failed.stderr_artifact,
                stdout_excerpt: None,
                stderr_excerpt,
                timestamp: failed.created_at,
            });
        }
    }

    // 3. Extract facts from database if available
    if let Some(db_conn) = db
        && let Ok(active) = omen_knowledge::FactRegistry::list_active_facts(db_conn, 50)
    {
        for fact in active {
            let info = omen_ipc::protocol::FactInfo {
                fact_id: fact.fact_id.to_string(),
                resource_uri: fact.resource_uri.to_string(),
                value: fact.value.to_string(),
                validity: format!("{:?}", fact.validity).to_uppercase(),
                assurance: format!("{:?}", fact.assurance).to_uppercase(),
            };
            if fact.validity == omen_core::ValidityState::Dirty {
                if !ctx
                    .dirty_facts
                    .iter()
                    .any(|f| f.resource_uri == info.resource_uri)
                {
                    ctx.dirty_facts.push(info);
                }
            } else if !ctx
                .current_facts
                .iter()
                .any(|f| f.resource_uri == info.resource_uri)
            {
                ctx.current_facts.push(info);
            }
        }
    }

    // 4. Supplement with any hot index facts from completion context
    if let Some(comp_lock) = comp_ctx
        && let Ok(c) = comp_lock.lock()
    {
        for fact in &c.hot_index.active_facts {
            let is_dirty = fact.validity == omen_core::ValidityState::Dirty;
            let validity_str = if is_dirty { "DIRTY" } else { "CURRENT" }.to_string();
            let info = omen_ipc::protocol::FactInfo {
                fact_id: String::new(),
                resource_uri: fact.resource_uri.clone(),
                value: String::new(),
                validity: validity_str,
                assurance: "OBSERVED".to_string(),
            };
            if is_dirty {
                if !ctx
                    .dirty_facts
                    .iter()
                    .any(|f| f.resource_uri == info.resource_uri)
                {
                    ctx.dirty_facts.push(info);
                }
            } else {
                if !ctx
                    .current_facts
                    .iter()
                    .any(|f| f.resource_uri == info.resource_uri)
                {
                    ctx.current_facts.push(info);
                }
            }
        }
    }

    // Populate services from SQLite workspace persistence if available
    if let Some(db_conn) = db
        && let Ok(records) = omen_knowledge::WorkspacePersistence::list_services(db_conn, &ws_id)
    {
        for s in records {
            ctx.services.push(omen_ipc::protocol::ManagedServiceInfo {
                name: s.name.clone(),
                resource_uri: format!("proc://workspace/{}", s.name),
                pid: s.pid,
                command: s.command,
                state: s.state,
                uptime_secs: 0,
                lease_id: None,
            });
        }
    }

    // Populate tools from shared completion context hot index (bounded PATH cache)
    if let Some(comp_lock) = comp_ctx
        && let Ok(c) = comp_lock.lock()
    {
        ctx.available_tools = c.hot_index.path_commands.clone();
    }

    ctx
}

fn run_future_blocking<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T> + Send,
    T: Send,
{
    std::thread::scope(|s| {
        s.spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("Failed to build tokio runtime")
                .block_on(future)
        })
        .join()
        .expect("Thread panicked")
    })
}
