use crate::context::AgentContext;
use crate::provider::AgentResponse;

/// Deterministic classifier providing instant answers directly from Omen machine truth
/// without performing expensive or unnecessary model calls.
pub struct DeterministicClassifier;

impl DeterministicClassifier {
    /// Evaluates if the given query can be answered deterministically with zero model calls.
    pub fn try_answer(query: &str, ctx: &AgentContext) -> Option<AgentResponse> {
        let q = query.trim().to_lowercase();
        let q_clean = q.trim_end_matches('?').trim();

        // 1. "what folder am I in?" / "where am I?" / "pwd"
        if q_clean == "what folder am i in"
            || q_clean == "what folder"
            || q_clean == "what directory am i in"
            || q_clean == "what directory"
            || q_clean == "where am i"
            || q_clean == "pwd"
        {
            let folder_name = ctx
                .cwd
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| ctx.cwd.display().to_string());
            let msg = format!(
                "You are in folder `{folder_name}` (`{}`).",
                ctx.cwd.display()
            );
            return Some(AgentResponse::explanation(msg));
        }

        // 2. "what branch am I on?" / "what branch"
        if q_clean == "what branch am i on"
            || q_clean == "what branch"
            || q_clean == "which branch am i on"
            || q_clean == "current branch"
        {
            let branch = ctx
                .git
                .as_ref()
                .and_then(|g| g.branch.as_deref())
                .unwrap_or("no active branch or not in a git repository");
            let msg = format!("Active git branch is `{branch}`.");
            return Some(AgentResponse::explanation(msg));
        }

        // 3. "what was the last command?" / "what did I just run?"
        if q_clean == "what was the last command"
            || q_clean == "what did that command just do"
            || q_clean == "what was my last command"
            || q_clean == "what did i just run"
            || q_clean == "last command"
        {
            if let Some(ref exec) = ctx.recent_execution {
                let code = exec
                    .exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "none".into());
                let msg = format!(
                    "The last command was `{}` (exit code {code}).",
                    exec.command
                );
                return Some(AgentResponse::explanation(msg));
            } else {
                return Some(AgentResponse::explanation(
                    "No previous command recorded in this session.",
                ));
            }
        }

        // 4. "is anything dirty?" / "is work tree clean?"
        if q_clean == "is anything dirty"
            || q_clean == "what is dirty"
            || q_clean == "is the workspace dirty"
            || q_clean == "are there dirty facts"
            || q_clean == "is anything broken"
        {
            let mut dirty_items = Vec::new();
            if !ctx.dirty_facts.is_empty() {
                dirty_items.push(format!("{} dirty fact(s)", ctx.dirty_facts.len()));
            }
            if let Some(ref git) = ctx.git {
                if !git.modified_files.is_empty() {
                    dirty_items.push(format!("{} modified file(s)", git.modified_files.len()));
                }
                if !git.untracked_files.is_empty() {
                    dirty_items.push(format!("{} untracked file(s)", git.untracked_files.len()));
                }
            }
            let msg = if dirty_items.is_empty() {
                "Workspace state is clean. No dirty facts or uncommitted changes detected."
                    .to_string()
            } else {
                format!("Workspace has dirty state: {}.", dirty_items.join(", "))
            };
            return Some(AgentResponse::explanation(msg));
        }

        // 5. "what file is this?"
        if q_clean == "what file is this" || q_clean == "what file am i looking at" {
            let msg = format!("Current focus is directory `{}`.", ctx.cwd.display());
            return Some(AgentResponse::explanation(msg));
        }

        None
    }
}
