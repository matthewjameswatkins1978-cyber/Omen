use omen_agent::{
    AgentContext, AgentError, AgentProvider, AgentRequest, ExecutionSummary, ProposedAction,
};
use omen_core::{CoreError, InteractiveSessionId};
use omen_knowledge::{Database, ExecutionHistory};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiLaneOutput {
    pub configured: bool,
    pub query: String,
    pub response_text: String,
    pub suggested_commands: Vec<String>,
    pub proposed_actions: Vec<ProposedAction>,
    pub references: Vec<String>,
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

        // If an AgentProvider is available, evaluate the query using structured AgentContext
        if let Some(p) = provider {
            let context = build_agent_context(session_id, cwd, db, comp_ctx);
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

                    let mut cmds = suggestions;
                    for action in &resp.proposed_actions {
                        match action {
                            ProposedAction::ExecuteTool {
                                tool,
                                operation,
                                args,
                                ..
                            } => {
                                if operation.is_empty() {
                                    cmds.push(format!(":{tool} {}", args.join(" ")));
                                } else {
                                    cmds.push(format!(":{tool} {operation} {}", args.join(" ")));
                                }
                            }
                            ProposedAction::ExecuteCommand { argv, .. } => {
                                cmds.push(argv.join(" "));
                            }
                            ProposedAction::SemanticAction { action, args } => {
                                cmds.push(format!(":{action} {}", args.join(" ")));
                            }
                            ProposedAction::ChangeDirectory { path } => {
                                cmds.push(format!("cd {}", path.display()));
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
                    });
                }
                Err(AgentError::Timeout(d)) => {
                    return Ok(AiLaneOutput {
                        configured: false,
                        query: query.to_string(),
                        response_text: format!(
                            "Agent unavailable: request exceeded {d:?}. Omen state is unchanged."
                        ),
                        suggested_commands: suggestions,
                        proposed_actions: Vec::new(),
                        references: Vec::new(),
                    });
                }
                Err(e) => {
                    return Ok(AiLaneOutput {
                        configured: false,
                        query: query.to_string(),
                        response_text: format!("Agent provider error: {e}"),
                        suggested_commands: suggestions,
                        proposed_actions: Vec::new(),
                        references: Vec::new(),
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
        })
    }
}

/// Builds bounded AgentContext from live session state.
pub fn build_agent_context(
    session_id: &InteractiveSessionId,
    cwd: &Path,
    db: Option<&Database>,
    comp_ctx: Option<&Arc<Mutex<crate::completion::CompletionContext>>>,
) -> AgentContext {
    let ws_id = omen_knowledge::deterministic_workspace_id(cwd);
    let mut ctx = AgentContext::new(
        ws_id,
        cwd.to_path_buf(),
        session_id.clone(),
        cwd.to_path_buf(),
    );

    // 1. Detect Git status in background CWD
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
                let state_dir = omen_knowledge::resolve_workspace_dir(cwd);
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

    ctx
}
