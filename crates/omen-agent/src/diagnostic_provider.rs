use crate::context::AgentContext;
use crate::provider::{
    AgentError, AgentProvider, AgentRequest, AgentResponse, AgentResponseKind, ProposedAction,
};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

/// Built-in deterministic agent reasoning engine.
/// Analyzes structured Omen machine state to answer questions without terminal scraping.
#[derive(Debug, Default)]
pub struct DiagnosticAgentProvider {
    known_directories: Vec<(String, PathBuf)>,
}

impl DiagnosticAgentProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_known_directories(mut self, dirs: Vec<(String, PathBuf)>) -> Self {
        self.known_directories = dirs;
        self
    }

    /// Evaluates user query against structured Omen context.
    pub fn evaluate(&self, request: &AgentRequest) -> AgentResponse {
        let q = request.prompt.trim().to_lowercase();
        let ctx = &request.context;

        // 1. "I'm lost"
        if q == "i'm lost" || q == "im lost" || q == "where am i and what was i doing?" {
            return self.explain_im_lost(ctx);
        }

        // 2. "why did that fail?" or "why did it fail?"
        if q.contains("why did that fail")
            || q.contains("why did it fail")
            || q.contains("what failed")
            || q.contains("why did my test fail")
        {
            return self.explain_failure(ctx);
        }

        // 3. "what folder am I in?" or "where am I?"
        if q.contains("what folder")
            || q.contains("what directory")
            || q == "where am i?"
            || q == "pwd"
        {
            return self.explain_folder(ctx);
        }

        // 4. "what is interesting in here?"
        if q.contains("what is interesting")
            || q.contains("what's in here")
            || q.contains("interesting in here")
        {
            return self.explain_interesting(ctx);
        }

        // 5. "what did that command just do?"
        if q.contains("what did that command just do") || q.contains("what did that do") {
            return self.explain_last_command(ctx);
        }

        // 6. "have I broken anything?"
        if q.contains("broken anything") || q.contains("did i break anything") {
            return self.explain_broken_state(ctx);
        }

        // 7. Navigation requests: "take me to <target>" or "go to <target>"
        if let Some(target) = extract_navigation_target(&q) {
            return self.resolve_navigation(&target, ctx);
        }

        // 8. Build delegation: "check whether this project still builds"
        if q.contains("still build") || q.contains("check build") || q.contains("run check") {
            return AgentResponse::proposal(
                "I propose running `cargo check` using Omen's canonical cargo tool adapter.",
                vec![ProposedAction::ExecuteTool {
                    tool: "cargo".into(),
                    operation: "check".into(),
                    args: vec![],
                    cwd: Some(ctx.cwd.to_string_lossy().to_string()),
                }],
            );
        }

        // Default response: provide overview and relevant deterministic actions
        let mut msg = format!(
            "You are in '{}'.\n\nI am observing the shared Omen machine state.",
            ctx.cwd.display()
        );
        if let Some(ref git) = ctx.git
            && let Some(ref b) = git.branch
        {
            msg.push_str(&format!(" Active branch is `{b}`."));
        }
        if !ctx.dirty_facts.is_empty() {
            msg.push_str(&format!(
                "\nWarning: {} fact(s) are currently DIRTY.",
                ctx.dirty_facts.len()
            ));
        }

        let mut actions = Vec::new();
        if ctx.recent_failed_execution.is_some() {
            actions.push(ProposedAction::SemanticAction {
                action: "show".into(),
                args: vec!["@failed".into()],
            });
        }

        AgentResponse::proposal(msg, actions)
    }

    fn explain_im_lost(&self, ctx: &AgentContext) -> AgentResponse {
        let repo_name = ctx
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| ctx.cwd.display().to_string());

        let mut lines = Vec::new();
        lines.push(format!("You are in:\n  {repo_name}"));

        // Recent changes from Git
        if let Some(ref git) = ctx.git {
            if !git.modified_files.is_empty() {
                lines.push(format!(
                    "\nRecent change:\n  {} modified",
                    git.modified_files.join(", ")
                ));
            } else if !git.staged_files.is_empty() {
                lines.push(format!(
                    "\nRecent change:\n  {} staged",
                    git.staged_files.join(", ")
                ));
            } else {
                lines.push("\nRecent change:\n  working tree clean".into());
            }

            let ahead = git.commits_ahead;
            lines.push(format!(
                "\nGit:\n  {} modified file(s)\n  {ahead} commit(s) ahead",
                git.modified_files.len()
            ));
        }

        // Build & Test state
        let failed_exec = ctx.recent_failed_execution.as_ref().or_else(|| {
            ctx.recent_execution
                .as_ref()
                .filter(|e| e.exit_code != Some(0))
        });

        if let Some(f) = failed_exec {
            lines.push(format!(
                "\nTests:\n  recent execution '{}' exited with code {}",
                f.command,
                f.exit_code.unwrap_or(-1)
            ));

            let problem = if let Some(ref stderr) = f.stderr_excerpt {
                extract_likely_problem(stderr)
            } else {
                "process returned non-zero exit status".to_string()
            };
            lines.push(format!("\nLikely current problem:\n  {problem}"));
            lines.push(format!(
                "\nSuggested next step:\n  inspect '{}' and repair the failing test assertion",
                f.command
            ));
        } else {
            lines.push("\nBuild:\n  clean".into());
            lines.push("\nTests:\n  no recorded failures in this session".into());
            lines.push(
                "\nSuggested next step:\n  continue development or run tests with ':test'".into(),
            );
        }

        if !ctx.dirty_facts.is_empty() {
            let uris: Vec<_> = ctx
                .dirty_facts
                .iter()
                .map(|f| f.resource_uri.as_str())
                .collect();
            lines.push(format!(
                "\nDirty facts:\n  {} fact(s) currently DIRTY ({})",
                ctx.dirty_facts.len(),
                uris.join(", ")
            ));
        }

        let mut actions = Vec::new();
        if let Some(_f) = failed_exec {
            actions.push(ProposedAction::SemanticAction {
                action: "show".into(),
                args: vec!["@failed".into()],
            });
        }

        AgentResponse::proposal(lines.join("\n"), actions)
    }

    fn explain_failure(&self, ctx: &AgentContext) -> AgentResponse {
        let failed = ctx.recent_failed_execution.as_ref().or_else(|| {
            ctx.recent_execution
                .as_ref()
                .filter(|e| e.exit_code != Some(0))
        });

        if let Some(f) = failed {
            let mut msg = format!(
                "The command '{}' failed with exit code {}.\n",
                f.command,
                f.exit_code.unwrap_or(-1)
            );

            if let Some(ref excerpt) = f.stderr_excerpt {
                let problem = extract_likely_problem(excerpt);
                msg.push_str(&format!("\nFailure detail:\n  {problem}\n"));
            }

            if let Some(ref art) = f.stderr_artifact {
                msg.push_str(&format!("\nFull stderr evidence stored in CAS:\n  {art}\n"));
            }

            if !ctx.dirty_facts.is_empty() {
                msg.push_str("\nInvalidated facts affected by this failure:\n");
                for fact in &ctx.dirty_facts {
                    msg.push_str(&format!("  - {} ({})\n", fact.resource_uri, fact.fact_id));
                }
            }

            let actions = vec![
                ProposedAction::SemanticAction {
                    action: "why".into(),
                    args: vec!["@last".into()],
                },
                ProposedAction::SemanticAction {
                    action: "show".into(),
                    args: vec!["@failed".into()],
                },
            ];

            let mut resp = AgentResponse::proposal(msg, actions);
            if let Some(ref art) = f.stderr_artifact {
                resp.references.push(art.clone());
            }
            resp
        } else {
            AgentResponse::explanation(
                "No failed executions are recorded in the current session. All recent commands exited successfully.",
            )
        }
    }

    fn explain_folder(&self, ctx: &AgentContext) -> AgentResponse {
        let mut msg = format!("You are currently in: `{}`\n", ctx.cwd.display());
        if let Some(ref git) = ctx.git {
            if let Some(ref b) = git.branch {
                msg.push_str(&format!("Git branch: `{b}`\n"));
            }
            if let Some(ref origin) = git.remote_origin {
                msg.push_str(&format!("Remote origin: `{origin}`\n"));
            }
        }
        AgentResponse::explanation(msg)
    }

    fn explain_interesting(&self, ctx: &AgentContext) -> AgentResponse {
        let mut interesting = Vec::new();
        if ctx.cwd.join("Cargo.toml").exists() {
            interesting.push("Cargo.toml (Rust workspace/project)");
        }
        if ctx.cwd.join("package.json").exists() {
            interesting.push("package.json (Node/Web project)");
        }
        if ctx.cwd.join(".git").exists() {
            interesting.push(".git (Version controlled repository)");
        }
        if ctx.cwd.join("README.md").exists() {
            interesting.push("README.md (Documentation)");
        }

        let mut msg = format!("Directory analysis for `{}`:\n", ctx.cwd.display());
        if interesting.is_empty() {
            msg.push_str(
                "Standard workspace directory without notable top-level project descriptors.",
            );
        } else {
            msg.push_str("Notable project resources:\n");
            for item in interesting {
                msg.push_str(&format!("  - {item}\n"));
            }
        }

        if !ctx.dirty_facts.is_empty() {
            msg.push_str(&format!(
                "\nThere are {} facts currently marked DIRTY in this workspace.",
                ctx.dirty_facts.len()
            ));
        }

        AgentResponse::explanation(msg)
    }

    fn explain_last_command(&self, ctx: &AgentContext) -> AgentResponse {
        if let Some(ref exec) = ctx.recent_execution {
            let status = match exec.exit_code {
                Some(0) => "succeeded (exit 0)".to_string(),
                Some(c) => format!("failed with exit code {c}"),
                None => "terminated abnormally".to_string(),
            };
            let duration = exec.duration_ms.unwrap_or(0);
            let mut msg = format!(
                "The last command executed was:\n  `{}`\n\nResult:\n  {}\nDuration:\n  {} ms\nExecution ID:\n  `{}`\n",
                exec.command, status, duration, exec.execution_id
            );
            if let Some(ref art) = exec.stdout_artifact {
                msg.push_str(&format!("Stdout CAS artifact: `{art}`\n"));
            }
            if let Some(ref art) = exec.stderr_artifact {
                msg.push_str(&format!("Stderr CAS artifact: `{art}`\n"));
            }
            AgentResponse::explanation(msg)
        } else {
            AgentResponse::explanation("No previous execution is recorded in the current session.")
        }
    }

    fn explain_broken_state(&self, ctx: &AgentContext) -> AgentResponse {
        let mut broken_items = Vec::new();

        if let Some(ref failed) = ctx.recent_failed_execution {
            broken_items.push(format!(
                "Command '{}' failed with exit code {}",
                failed.command,
                failed.exit_code.unwrap_or(-1)
            ));
        }

        for fact in &ctx.dirty_facts {
            broken_items.push(format!(
                "Fact for '{}' is DIRTY ({})",
                fact.resource_uri, fact.fact_id
            ));
        }

        for svc in &ctx.services {
            if svc.state == "crashed" {
                broken_items.push(format!("Service '{}' has crashed", svc.name));
            }
        }

        if broken_items.is_empty() {
            AgentResponse::explanation(
                "Everything looks clean: no failing executions, no crashed services, and zero dirty facts.",
            )
        } else {
            let mut msg = "The following potential issues were detected:\n".to_string();
            for item in broken_items {
                msg.push_str(&format!("  - {item}\n"));
            }
            AgentResponse::explanation(msg)
        }
    }

    fn resolve_navigation(&self, target: &str, ctx: &AgentContext) -> AgentResponse {
        let target_norm = target.trim().to_lowercase();

        // Check if target matches an exact subfolder of CWD
        let direct_sub = ctx.cwd.join(target);
        if direct_sub.is_dir() {
            return AgentResponse::proposal(
                format!("Navigating to `{}`.", direct_sub.display()),
                vec![ProposedAction::ChangeDirectory { path: direct_sub }],
            );
        }

        // Match against known directories (e.g., Lantern Warden, Lantern Keeper, Lantern docs)
        let mut matches: Vec<(String, PathBuf)> = self
            .known_directories
            .iter()
            .filter(|(name, _)| name.to_lowercase().contains(&target_norm))
            .map(|(n, p)| (n.clone(), p.clone()))
            .collect();

        if matches.is_empty() {
            // Scan directories under ctx.cwd recursively up to depth 3
            let mut stack = vec![(ctx.cwd.clone(), 0usize)];
            while let Some((dir, depth)) = stack.pop() {
                if depth >= 3 {
                    continue;
                }
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if name.starts_with('.') || name == "target" || name == "node_modules" {
                                continue;
                            }
                            if name.to_lowercase().contains(&target_norm) {
                                let display_name = path
                                    .strip_prefix(&ctx.cwd)
                                    .map(|p| p.to_string_lossy().to_string())
                                    .unwrap_or_else(|_| name.clone());
                                matches.push((display_name, path.clone()));
                            }
                            stack.push((path, depth + 1));
                        }
                    }
                }
            }
        }

        if matches.len() == 1 {
            let (name, path) = &matches[0];
            return AgentResponse::proposal(
                format!("Navigating to {name} at `{}`.", path.display()),
                vec![ProposedAction::ChangeDirectory { path: path.clone() }],
            );
        }

        if matches.len() > 1 {
            matches.sort_by(|a, b| a.0.cmp(&b.0));
            // Rule: "Infer what has already been decided. Never invent what has not."
            // Multiple legitimate interpretations -> show concise choices
            let mut msg = format!("I found multiple likely matches for '{target}':\n\n");
            for (i, (name, path)) in matches.iter().enumerate() {
                msg.push_str(&format!("{}. {} (`{}`)\n", i + 1, name, path.display()));
            }
            msg.push_str("\nWhich one would you like to navigate to?");

            return AgentResponse {
                kind: AgentResponseKind::Question,
                message: msg,
                proposed_actions: Vec::new(),
                references: Vec::new(),
                uncertainty: Some("Multiple candidate directories found".into()),
            };
        }

        // Check parent directory search
        let parent = ctx.cwd.parent();
        if let Some(p) = parent {
            let cand = p.join(target);
            if cand.is_dir() {
                return AgentResponse::proposal(
                    format!("Navigating to `{}`.", cand.display()),
                    vec![ProposedAction::ChangeDirectory { path: cand }],
                );
            }
        }

        AgentResponse::refusal(format!(
            "Could not find a matching directory for '{target}' in this workspace."
        ))
    }
}

impl AgentProvider for DiagnosticAgentProvider {
    fn respond<'a>(
        &'a self,
        request: AgentRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AgentResponse, AgentError>> + Send + 'a>> {
        Box::pin(async move { Ok(self.evaluate(&request)) })
    }
}

fn extract_navigation_target(query: &str) -> Option<String> {
    for prefix in [
        "switch to ",
        "switch into ",
        "take me to ",
        "go to ",
        "cd to ",
        "cd ",
    ] {
        if let Some(rest) = query.strip_prefix(prefix) {
            let cleaned = rest.trim_end_matches('.').trim();
            if !cleaned.is_empty() {
                return Some(cleaned.to_string());
            }
        }
    }
    None
}

fn extract_likely_problem(stderr: &str) -> String {
    for line in stderr.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("error[")
            || trimmed.starts_with("error:")
            || trimmed.contains("FAILED")
            || trimmed.starts_with("assertion failed:")
            || trimmed.starts_with("panicked at")
        {
            return trimmed.to_string();
        }
    }
    // Fallback to first non-empty line
    stderr
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("error output in stderr")
        .to_string()
}
