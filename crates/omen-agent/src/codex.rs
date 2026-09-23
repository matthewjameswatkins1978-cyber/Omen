//! Codex external reference route.
//!
//! Drives the installed `codex exec --json` machine surface as an
//! [`AgentProvider`]. Omen owns the invocation (argv pinned,
//! `--ignore-user-config` so personal Codex config cannot change Omen
//! semantics, sandbox `read-only` so Codex cannot bypass Omen for
//! consequential actions), translates JSONL events into canonical
//! `AgentResponse` / `AgentError`, and runs the structured tool loop with
//! Omen as the executing authority.
//!
//! Codex authentication remains owned by Codex (its own CODEX_HOME store).
//! Omen only determines configured-enough-to-attempt at use time; remote
//! validity is runtime truth. No network probing at startup or registry
//! construction: availability is binary-on-PATH only.

use crate::provider::{
    AgentError, AgentProvider, AgentRequest, AgentResponse, AgentResponseKind, ProposedAction,
};
use omen_agent_adapter::{EnvPolicy, codex_env_policy, fingerprint_pairs, sha256_hex};
use std::future::Future;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

/// Provider id for the Codex reference route. Omen speaks Omen:
/// `:agent use codex`, never `:codex`.
pub const CODEX_PROVIDER_ID: &str = "codex";
/// Codex routes are agentic and out-of-process; ceiling is generous but explicit.
pub const CODEX_ROUTE_TIMEOUT: Duration = Duration::from_secs(300);
/// Probe ceiling: `codex --version` must answer promptly. A broken or
/// hijacked executable fails closed instead of stalling the request path.
pub const CODEX_PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// Hard cap on total captured Codex stdout (JSONL events).
pub const CODEX_MAX_OUTPUT_BYTES: usize = 512 * 1024;
/// Bootstrap prompt budget: Codex must not need a giant manual.
pub const CODEX_BOOTSTRAP_BUDGET_BYTES: usize = 4096;
/// Bounded stderr retained for failure diagnosis.
pub const CODEX_MAX_STDERR_BYTES: usize = 8192;

/// Operations Omen executes for Codex. Anything else Codex wants becomes
/// proposal data, never executed by Omen without admission.
pub const CODEX_TOOL_ALLOWLIST: &[&str] = &["omen.workspace_status", "omen.describe_capability"];

#[derive(Clone)]
pub struct CodexRouteConfig {
    pub exe: PathBuf,
    pub sandbox: String,
    pub timeout: Duration,
    /// `None` means server-assigned (Omen never passes `-m` unless the
    /// supported interface requires one). Recorded as UNKNOWN, not invented.
    pub model: Option<String>,
    pub parent_env: Vec<(String, String)>,
    pub env_policy: EnvPolicy,
}

impl CodexRouteConfig {
    pub fn new(exe: PathBuf, parent_env: Vec<(String, String)>) -> Self {
        Self {
            exe,
            sandbox: "read-only".to_string(),
            timeout: CODEX_ROUTE_TIMEOUT,
            model: None,
            env_policy: codex_env_policy(),
            parent_env,
        }
    }
}

/// Resolves the Codex executable without spawning anything.
/// Order: OMEN_CODEX_EXE override, codex.exe on PATH, vendored real exe
/// behind an npm shim (Windows). `None` means the route is not installed;
/// registry construction stays cheap.
///
/// Executable boundary is argv-only with no shell-command semantics: a
/// `.cmd`/`.bat` script is NEVER returned. When only a shim is present or
/// vendored resolution is ambiguous, resolution fails closed so the caller
/// reports the route unavailable instead of executing a command script.
pub fn resolve_codex_exe() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("OMEN_CODEX_EXE") {
        let explicit = PathBuf::from(path);
        if has_executable_boundary(&explicit) && explicit.is_file() {
            return Some(explicit);
        }
    }
    let file = if cfg!(windows) { "codex.exe" } else { "codex" };
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(file);
        if has_executable_boundary(&candidate) && candidate.is_file() {
            return Some(candidate);
        }
        // Windows npm installs ship only shims (codex.cmd) on PATH; the
        // real binary lives under the package vendor directory. Use the
        // real exe when it resolves uniquely; otherwise KEEP SCANNING and
        // ultimately fail closed. The shim itself is never returned.
        if cfg!(windows) {
            let shim = dir.join("codex.cmd");
            if shim.is_file()
                && let Some(real) = resolve_npm_shim_target(&shim)
            {
                return Some(real);
            }
        }
    }
    None
}

/// True unless the path ends in a command-script extension. One shared
/// gate: neither OMEN_CODEX_EXE nor PATH scanning may yield `.cmd`/`.bat`.
fn has_executable_boundary(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => !ext.eq_ignore_ascii_case("cmd") && !ext.eq_ignore_ascii_case("bat"),
        None => true,
    }
}

/// Resolves an npm `codex.cmd` shim to its vendored codex.exe when the
/// install layout yields exactly one candidate; ambiguous or changed
/// layouts return None so the caller keeps scanning / fails closed and
/// never executes the shim itself.
fn resolve_npm_shim_target(shim: &Path) -> Option<PathBuf> {
    let base = shim
        .parent()?
        .join("node_modules")
        .join("@openai")
        .join("codex")
        .join("node_modules")
        .join("@openai");
    let mut hits = Vec::new();
    for package in std::fs::read_dir(&base).ok()? {
        let package = package.ok()?;
        if !package.file_name().to_string_lossy().starts_with("codex-") {
            continue;
        }
        for target in std::fs::read_dir(package.path().join("vendor")).ok()? {
            let exe = target.ok()?.path().join("bin").join("codex.exe");
            if exe.is_file() {
                hits.push(exe);
            }
        }
    }
    if hits.len() == 1 { hits.pop() } else { None }
}

/// Shared helper: the exact child environment for EVERY Codex process Omen
/// launches — probe and reasoning route alike. `env_clear` plus the
/// canonical [`codex_env_policy`]: only approved passthrough entries plus
/// Omen-set literals cross. Codex authentication stays Codex-owned: no
/// credential value (OPENAI_API_KEY, provider keys, synthetic secrets) is
/// ever injected or inherited.
pub fn codex_child_env(
    parent_env: &[(String, String)],
    policy: &EnvPolicy,
) -> Vec<(String, String)> {
    policy.build_env(parent_env, &["codex-auth".to_string()])
}

/// Shared helper: a `codex --version` probe command under the same
/// environment doctrine as the main route. No shell, no batch execution.
fn new_codex_probe_command(
    exe: &Path,
    parent_env: &[(String, String)],
    policy: &EnvPolicy,
) -> std::process::Command {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--version");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.env_clear();
    for (k, v) in codex_child_env(parent_env, policy) {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000);
    }
    cmd
}

/// Explicit probe (spawns `codex --version` + reads exe bytes). Use-time or
/// explicit diagnostics only — never at startup. Isolated and bounded like
/// the main route: allowlisted env, explicit deadline, kill on expiry.
#[derive(Debug, Clone)]
pub struct CodexProbe {
    pub version: String,
    pub exe_digest: String,
}

pub fn probe_codex(
    exe: &Path,
    parent_env: &[(String, String)],
    policy: &EnvPolicy,
) -> Result<CodexProbe, AgentError> {
    probe_codex_with_timeout(exe, parent_env, policy, CODEX_PROBE_TIMEOUT)
}

pub fn probe_codex_with_timeout(
    exe: &Path,
    parent_env: &[(String, String)],
    policy: &EnvPolicy,
    timeout: Duration,
) -> Result<CodexProbe, AgentError> {
    let exe_bytes = std::fs::read(exe).map_err(|e| AgentError::ProviderUnavailable {
        provider: CODEX_PROVIDER_ID.into(),
        message: format!("codex executable unreadable: {e}"),
    })?;
    let mut child = new_codex_probe_command(exe, parent_env, policy)
        .spawn()
        .map_err(|e| AgentError::ProviderUnavailable {
            provider: CODEX_PROVIDER_ID.into(),
            message: format!("codex --version spawn failed: {e}"),
        })?;
    // Bounded wait WITHOUT hiding it in a worker thread: poll try_wait
    // against an explicit deadline. The child is already exited when we
    // proceed, so wait_with_output only drains the OS pipe buffer (64 KiB
    // class) — time and memory both bounded.
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    kill_probe_tree(&mut child);
                    let _ = child.wait();
                    return Err(AgentError::Timeout(timeout));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                kill_probe_tree(&mut child);
                return Err(AgentError::ProviderUnavailable {
                    provider: CODEX_PROVIDER_ID.into(),
                    message: format!("codex --version wait failed: {e}"),
                });
            }
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| AgentError::ProviderUnavailable {
            provider: CODEX_PROVIDER_ID.into(),
            message: format!("codex --version output failed: {e}"),
        })?;
    if !out.status.success() {
        return Err(AgentError::ProviderUnavailable {
            provider: CODEX_PROVIDER_ID.into(),
            message: format!("codex --version exited {}", out.status),
        });
    }
    Ok(CodexProbe {
        version: truncate_text(&String::from_utf8_lossy(&out.stdout), 512)
            .trim()
            .to_string(),
        exe_digest: sha256_hex(&exe_bytes),
    })
}

/// Best-effort kill of a hung probe child (tree on Windows). Synchronous
/// and bounded: the taskkill helper is itself polled against a short
/// deadline, never waited on blindly.
fn kill_probe_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let pid = child.id();
        if let Ok(mut killer) = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .spawn()
        {
            let helper_deadline = std::time::Instant::now() + Duration::from_secs(3);
            loop {
                match killer.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) => {
                        if std::time::Instant::now() >= helper_deadline {
                            let _ = killer.kill();
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(25));
                    }
                }
            }
        }
    }
    let _ = child.kill();
}

pub struct CodexAdapter {
    config: CodexRouteConfig,
    probe_cache: Mutex<Option<CodexProbe>>,
}

impl CodexAdapter {
    pub fn new(config: CodexRouteConfig) -> Self {
        Self {
            config,
            probe_cache: Mutex::new(None),
        }
    }

    pub fn config_fingerprint(&self, schema_digest: &str) -> String {
        fingerprint_pairs(&[
            ("argv0", &self.config.exe.to_string_lossy()),
            ("sandbox", &self.config.sandbox),
            ("ignore_user_config", "true"),
            ("skip_git_repo_check", "true"),
            ("json", "true"),
            (
                "model",
                self.config.model.as_deref().unwrap_or("server-assigned"),
            ),
            ("output_schema_digest", schema_digest),
        ])
    }

    fn ensure_probe(&self) -> Result<CodexProbe, AgentError> {
        let mut guard = self.probe_cache.lock().unwrap();
        if let Some(probe) = guard.clone() {
            return Ok(probe);
        }
        let probe = probe_codex(
            &self.config.exe,
            &self.config.parent_env,
            &self.config.env_policy,
        )?;
        *guard = Some(probe.clone());
        Ok(probe)
    }

    async fn exchange(&self, request: AgentRequest) -> Result<AgentResponse, AgentError> {
        let probe = self.ensure_probe()?;
        let schema = codex_final_schema();
        let schema_digest = sha256_hex(schema.as_bytes());
        let _ = (probe, schema_digest);
        let total = self.config.timeout;
        match tokio::time::timeout(total, self.exchange_inner(request, &schema)).await {
            Ok(result) => result,
            Err(_) => Err(AgentError::Timeout(total)),
        }
    }

    async fn exchange_inner(
        &self,
        request: AgentRequest,
        schema: &str,
    ) -> Result<AgentResponse, AgentError> {
        // Round 1: ask Codex what it concludes / requests, schema-constrained.
        let prompt = build_codex_prompt(&request, 1, None);
        let round1 = self.run_codex(&prompt, schema, &request).await?;
        let parsed = parse_codex_final(&round1.final_text)?;
        if parsed.tool_requests.is_empty() {
            return self.admit_final(&parsed, &round1);
        }
        // Round 2: execute allowlisted operations under Omen authority,
        // refuse anything else explicitly, then ask for the final answer.
        let mut results = Vec::new();
        for tool_request in &parsed.tool_requests {
            let outcome = self.execute_codex_tool(tool_request, &request);
            results.push(outcome);
        }
        let prompt2 = build_codex_prompt(&request, 2, Some(&results));
        let round2 = self.run_codex(&prompt2, schema, &request).await?;
        let parsed2 = parse_codex_final(&round2.final_text)?;
        if !parsed2.tool_requests.is_empty() {
            // One tool round trip per request: a second ask is a proposal,
            // never an unbounded loop.
            return Err(AgentError::Rejected(
                "codex requested further tool calls after the tool round trip; refusing rather than looping".into(),
            ));
        }
        self.admit_final(&parsed2, &round2)
    }

    fn execute_codex_tool(
        &self,
        tool_request: &CodexToolRequest,
        request: &AgentRequest,
    ) -> serde_json::Value {
        if !CODEX_TOOL_ALLOWLIST.contains(&tool_request.tool.as_str()) {
            return serde_json::json!({
                "tool": tool_request.tool,
                "ok": false,
                "error": {"code": "unsupported_capability",
                          "message": format!("tool {:?} is not allowlisted by Omen", tool_request.tool)},
            });
        }
        let result = match tool_request.tool.as_str() {
            "omen.workspace_status" => Ok(serde_json::json!({
                "workspace_root": request.context.workspace_root.to_string_lossy(),
                "cwd": request.context.cwd.to_string_lossy(),
                "omen_contract": "0.8",
            })),
            "omen.describe_capability" => {
                // Positional args channel (the strict output schema has no
                // free-form input object): args[0] is the capability name.
                let name = tool_request.args.first().map(|s| s.as_str()).unwrap_or("");
                if name.len() > 128 {
                    Err("capability name too long".into())
                } else {
                    match describe_capability(name) {
                        Some(description) => {
                            Ok(serde_json::json!({"name": name, "description": description}))
                        }
                        None => Err(format!("unknown capability {name:?}")),
                    }
                }
            }
            _ => Err("unreachable: allowlist checked above".into()),
        };
        match result {
            Ok(value) => {
                serde_json::json!({"tool": tool_request.tool, "ok": true, "result": value})
            }
            Err(message) => serde_json::json!({"tool": tool_request.tool, "ok": false,
                "error": {"code": "tool_failed", "message": message}}),
        }
    }

    fn admit_final(
        &self,
        parsed: &CodexFinal,
        _round: &CodexRound,
    ) -> Result<AgentResponse, AgentError> {
        let proposal: Option<&Vec<String>> = match &parsed.proposal_argv {
            Some(argv) if !argv.is_empty() => Some(argv),
            _ => None,
        };
        let proposal_valid = proposal
            .map(|argv| argv.len() <= 12 && !argv.iter().any(|a| a.len() > 1024))
            .unwrap_or(true);
        if !proposal_valid {
            return Err(AgentError::Rejected(
                "codex proposal_argv failed bounds (1..=12 entries, each <=1024)".into(),
            ));
        }
        if let Some(argv) = proposal {
            // Proposal data only: admission happens elsewhere or never.
            return Ok(AgentResponse {
                kind: AgentResponseKind::Proposal,
                message: truncate_text(&parsed.message, 8192),
                proposed_actions: vec![ProposedAction::ExecuteCommand {
                    argv: argv.clone(),
                    cwd: None,
                }],
                references: vec![],
                uncertainty: None,
            });
        }
        Ok(AgentResponse {
            kind: AgentResponseKind::Explanation,
            message: truncate_text(&parsed.message, 8192),
            proposed_actions: vec![],
            references: vec![],
            uncertainty: None,
        })
    }

    /// Spawns one `codex exec` child: pinned argv, allowlisted env, bounded
    /// capture, explicit timeout, kill-tree on expiry or future-drop.
    async fn run_codex(
        &self,
        prompt: &str,
        schema: &str,
        request: &AgentRequest,
    ) -> Result<CodexRound, AgentError> {
        // The output schema travels via temp file (argv stays a path, not content).
        let schema_path = write_temp_schema(schema)?;
        let cwd = pick_codex_cwd(request);
        let env = codex_child_env(&self.config.parent_env, &self.config.env_policy);
        let mut argv: Vec<String> = vec![
            "exec".into(),
            "--json".into(),
            "--ignore-user-config".into(),
            "--skip-git-repo-check".into(),
            "-s".into(),
            self.config.sandbox.clone(),
            "--output-schema".into(),
            schema_path.to_string_lossy().into_owned(),
            "-C".into(),
            cwd.to_string_lossy().into_owned(),
            prompt.to_string(),
        ];
        if let Some(model) = &self.config.model {
            argv.push("-m".into());
            argv.push(model.clone());
        }
        let mut cmd = tokio::process::Command::new(&self.config.exe);
        cmd.args(&argv);
        cmd.current_dir(&cwd);
        cmd.env_clear();
        for (k, v) in &env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd.spawn().map_err(|e| AgentError::ProviderUnavailable {
            provider: CODEX_PROVIDER_ID.into(),
            message: format!("codex spawn failed: {e}"),
        })?;
        // Drop-kill guard: task abort / future drop cannot strand the child.
        struct KillGuard<'a> {
            child: &'a mut tokio::process::Child,
        }
        impl Drop for KillGuard<'_> {
            fn drop(&mut self) {
                let _ = self.child.start_kill();
            }
        }
        let guard = KillGuard { child: &mut child };
        let stdout = guard
            .child
            .stdout
            .take()
            .ok_or_else(|| AgentError::Provider("codex child has no stdout".into()))?;
        let stderr = guard
            .child
            .stderr
            .take()
            .ok_or_else(|| AgentError::Provider("codex child has no stderr".into()))?;
        let reader = CodexJsonlReader {
            stdout,
            stderr,
            pid: guard.child.id().unwrap_or(0),
        };
        let timeout = self.config.timeout;
        let output = match tokio::time::timeout(timeout, reader.read_all()).await {
            Ok(output) => output,
            Err(_) => {
                kill_codex_tree(guard.child).await;
                let _ = std::fs::remove_file(&schema_path);
                return Err(AgentError::Timeout(timeout));
            }
        };
        // Reap; disarm the guard by forgetting after explicit kill-safe wait.
        std::mem::forget(guard);
        let status = child
            .wait()
            .await
            .map_err(|e| AgentError::Provider(format!("codex reap failed: {e}")))?;
        let _ = std::fs::remove_file(&schema_path);
        translate_codex_output(output, status.code())
    }
}

impl AgentProvider for CodexAdapter {
    fn respond<'a>(
        &'a self,
        request: AgentRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AgentResponse, AgentError>> + Send + 'a>> {
        Box::pin(self.exchange(request))
    }
}

/// One completed `codex exec` invocation, translated but not yet admitted.
#[derive(Debug)]
pub struct CodexRound {
    pub thread_id: Option<String>,
    pub final_text: String,
    pub usage: Option<serde_json::Value>,
    pub events_seen: u64,
}

/// Schema-constrained final message from Codex.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct CodexFinal {
    pub message: String,
    #[serde(default)]
    pub tool_requests: Vec<CodexToolRequest>,
    #[serde(default)]
    pub proposal_argv: Option<Vec<String>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct CodexToolRequest {
    pub tool: String,
    #[serde(default)]
    pub operation: String,
    #[serde(default)]
    pub input: serde_json::Value,
    #[serde(default)]
    pub args: Vec<String>,
}

/// JSON Schema constraining the Codex final message. Omen validates the
/// parsed result itself; the schema is instruction, not trust.
pub fn codex_final_schema() -> String {
    serde_json::json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": "OmenCodexFinal",
        "type": "object",
        "required": ["message", "tool_requests", "proposal_argv"],
        "additionalProperties": false,
        "properties": {
            "message": {"type": "string", "maxLength": 8000},
            "tool_requests": {
                "type": "array", "maxItems": 4,
                "items": {
                    "type": "object",
                    "required": ["tool", "operation", "args"],
                    "additionalProperties": false,
                    "properties": {
                        "tool": {"type": "string", "maxLength": 128},
                        "operation": {"type": "string", "maxLength": 128},
                        "args": {"type": "array", "maxItems": 8, "items": {"type": "string", "maxLength": 512}}
                    }
                }
            },
            "proposal_argv": {
                "type": "array", "maxItems": 12,
                "items": {"type": "string", "maxLength": 1024}
            }
        }
    })
    .to_string()
}

/// Small bootstrap: orientation + allowlist + task. Never the manual.
pub fn build_codex_prompt(
    request: &AgentRequest,
    round: u8,
    tool_results: Option<&[serde_json::Value]>,
) -> String {
    let mut prompt = String::new();
    prompt.push_str("You are Codex operating as a reasoning route inside Omen. Omen is the substrate: you translate and propose; consequential actions execute only through Omen. Your sandbox is read-only: do not attempt filesystem or shell mutations yourself; express intent ONLY in the structured final message.\n");
    prompt.push_str("Omen contract 0.8. Allowlisted Omen operations you may request via tool_requests (all else -> proposal_argv data):\n");
    prompt.push_str(
        "- omen.workspace_status with args []: returns workspace_root, cwd, omen_contract.\n",
    );
    prompt.push_str(
        "- omen.describe_capability with args [name] (reasoning|tools|deterministic|orientation): returns a description.\n",
    );
    prompt.push_str("Final message MUST be JSON: {\"message\": \"...\", \"tool_requests\": [{\"tool\": \"...\", \"operation\": \"...\", \"args\": [...]}], \"proposal_argv\": [...] | null}. ");
    prompt.push_str("Use tool_requests ONLY for the two operations above. Use proposal_argv ONLY to propose a command for Omen to consider (it will NOT run without admission). Otherwise both stay empty/null.\n");
    prompt.push_str(&format!(
        "Workspace root: {}. Session cwd: {}.\n",
        truncate_text(&request.context.workspace_root.to_string_lossy(), 512),
        truncate_text(&request.context.cwd.to_string_lossy(), 512)
    ));
    if round == 1 {
        prompt.push_str("Task: ");
        prompt.push_str(&truncate_text(&request.prompt, 1500));
    } else {
        prompt.push_str("Omen executed your requested operations with these results (ok=false means refused or failed; do not retry, incorporate truthfully):\n");
        if let Some(results) = tool_results {
            prompt.push_str(&truncate_text(
                &serde_json::to_string_pretty(results).unwrap_or_default(),
                1500,
            ));
        }
        prompt.push_str("\nNow produce your final JSON message answering the original task: ");
        prompt.push_str(&truncate_text(&request.prompt, 500));
        prompt.push_str("\nYour tool phase is OVER: emit \"tool_requests\": [] and \"proposal_argv\": null. Do not request further operations for any reason.");
    }
    prompt.push('\n');
    truncate_text(&prompt, CODEX_BOOTSTRAP_BUDGET_BYTES)
}

/// Strict parse first, then one bounded fallback (largest {...} span).
/// Either way the result is schema-validated; chatter never sneaks through.
pub fn parse_codex_final(text: &str) -> Result<CodexFinal, AgentError> {
    if let Ok(parsed) = serde_json::from_str::<CodexFinal>(text.trim()) {
        return check_codex_final(parsed);
    }
    if let Some(span) = largest_brace_span(text)
        && let Ok(parsed) = serde_json::from_str::<CodexFinal>(&span)
    {
        return check_codex_final(parsed);
    }
    Err(AgentError::Rejected(
        "codex final message is not schema-valid JSON; refusing rather than guessing".into(),
    ))
}

fn check_codex_final(parsed: CodexFinal) -> Result<CodexFinal, AgentError> {
    if parsed.message.len() > 8000 {
        return Err(AgentError::Rejected(
            "codex message exceeds 8000 bytes".into(),
        ));
    }
    if parsed.tool_requests.len() > 4 {
        return Err(AgentError::Rejected("codex tool_requests exceeds 4".into()));
    }
    for tool_request in &parsed.tool_requests {
        if tool_request.tool.len() > 128 {
            return Err(AgentError::Rejected("codex tool name too long".into()));
        }
    }
    let proposal_valid = parsed
        .proposal_argv
        .as_ref()
        .map(|argv| argv.is_empty() || (argv.len() <= 12 && !argv.iter().any(|a| a.len() > 1024)))
        .unwrap_or(true);
    if !proposal_valid {
        return Err(AgentError::Rejected(
            "codex proposal_argv failed bounds".into(),
        ));
    }
    Ok(parsed)
}

fn largest_brace_span(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut best: Option<(usize, usize)> = None;
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'{' {
            if depth == 0 {
                start = i;
            }
            depth += 1;
        } else if b == b'}' && depth > 0 {
            depth -= 1;
            if depth == 0 {
                let len = i + 1 - start;
                if best.map(|(s, e)| e - s).unwrap_or(0) < len && len <= 32768 {
                    best = Some((start, i + 1));
                }
            }
        }
    }
    best.map(|(s, e)| text[s..e].to_string())
}

fn describe_capability(name: &str) -> Option<&'static str> {
    match name {
        "reasoning" => Some("External reasoning route: prompts in, typed Omen responses out."),
        "tools" => {
            Some("Structured tool round trip under Omen admission; see omen.workspace_status.")
        }
        "deterministic" => Some("Deterministic Omen answers bypass all providers with zero calls."),
        "orientation" => Some("Small bootstrap context; describe narrowly, load progressively."),
        _ => None,
    }
}

struct CodexJsonlReader {
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    pid: u32,
}

struct CodexRawOutput {
    lines: Vec<String>,
    stderr_tail: String,
    truncated: bool,
}

impl CodexJsonlReader {
    async fn read_all(mut self) -> CodexRawOutput {
        use tokio::io::AsyncReadExt;
        let mut total = 0usize;
        let mut lines = Vec::new();
        let mut truncated = false;
        let mut buf = vec![0u8; 8192];
        let mut pending: Vec<u8> = Vec::new();
        // Deadline-free read: the caller wraps this in the route timeout.
        // stdout EOF ends the loop; stderr is drained afterwards, bounded.
        loop {
            match self.stdout.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    if total > CODEX_MAX_OUTPUT_BYTES {
                        truncated = true;
                        break;
                    }
                    pending.extend_from_slice(&buf[..n]);
                    while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = pending.drain(..=pos).collect();
                        let mut line = line;
                        line.pop();
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                        // Transport requires UTF-8; invalid bytes are a
                        // truthful failure, not silent loss.
                        match String::from_utf8(line) {
                            Ok(text) => lines.push(text),
                            Err(_) => {
                                lines.push("__OMEN_INVALID_UTF8__".to_string());
                            }
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if !pending.is_empty() {
            match String::from_utf8(std::mem::take(&mut pending)) {
                Ok(text) if !text.trim().is_empty() => lines.push(text),
                _ => {}
            }
        }
        let mut err_buf = vec![0u8; CODEX_MAX_STDERR_BYTES + 1];
        let mut err_total = 0usize;
        let mut err_data = Vec::new();
        loop {
            match self.stderr.read(&mut err_buf).await {
                Ok(0) => break,
                Ok(n) => {
                    err_total += n;
                    err_data.extend_from_slice(&err_buf[..n]);
                    if err_data.len() > CODEX_MAX_STDERR_BYTES {
                        let drop_n = err_data.len() - CODEX_MAX_STDERR_BYTES;
                        err_data.drain(..drop_n);
                    }
                    if err_total > CODEX_MAX_STDERR_BYTES * 4 {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = self.pid;
        CodexRawOutput {
            lines,
            stderr_tail: String::from_utf8_lossy(&err_data).to_string(),
            truncated,
        }
    }
}

/// Maps raw Codex output + exit status to a round or a stable Omen error.
/// Codex-specific strings stay bounded underneath; they never widen semantics.
fn translate_codex_output(
    output: CodexRawOutput,
    exit_code: Option<i32>,
) -> Result<CodexRound, AgentError> {
    const P: &str = CODEX_PROVIDER_ID;
    let mut thread_id = None;
    let mut messages: Vec<String> = Vec::new();
    let mut usage = None;
    let mut failed_message: Option<String> = None;
    let mut events_seen = 0u64;
    for line in &output.lines {
        if line == "__OMEN_INVALID_UTF8__" {
            return Err(AgentError::Rejected(
                "codex emitted invalid UTF-8 on the JSONL stream".into(),
            ));
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                return Err(AgentError::Rejected(format!(
                    "codex emitted a non-JSON line: {}",
                    truncate_text(line, 256)
                )));
            }
        };
        events_seen += 1;
        match value.get("type").and_then(|v| v.as_str()) {
            Some("thread.started") => {
                thread_id = value
                    .get("thread_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
            Some("item.completed") => {
                if let Some(item) = value.get("item")
                    && item.get("type").and_then(|v| v.as_str()) == Some("agent_message")
                    && let Some(text) = item.get("text").and_then(|v| v.as_str())
                {
                    messages.push(text.to_string());
                }
            }
            Some("turn.completed") => {
                usage = value.get("usage").cloned();
            }
            Some("turn.failed") | Some("error") => {
                let message = value
                    .get("error")
                    .and_then(|e| {
                        e.get("message")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
                    .or_else(|| {
                        value
                            .get("message")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default();
                if failed_message.is_none() {
                    failed_message = Some(message);
                }
            }
            _ => {}
        }
    }
    if output.truncated {
        return Err(AgentError::Provider(format!(
            "codex output exceeded {CODEX_MAX_OUTPUT_BYTES} bytes; refusing partial stream"
        )));
    }
    if let Some(final_text) = messages.pop() {
        return Ok(CodexRound {
            thread_id,
            final_text,
            usage,
            events_seen,
        });
    }
    // No agent message: never synthesize one. Classify the failure.
    let haystack = format!(
        "{} {}",
        failed_message.clone().unwrap_or_default(),
        output.stderr_tail
    );
    let lower = haystack.to_lowercase();
    if exit_code == Some(0) {
        return Err(AgentError::Rejected(
            "codex exited 0 with no agent message; no synthetic response".into(),
        ));
    }
    if lower.contains("log in")
        || lower.contains("login")
        || lower.contains("auth")
        || lower.contains("401")
        || lower.contains("unauthorized")
    {
        return Err(AgentError::AuthenticationRequired {
            provider: P.into(),
            message: truncate_text(&haystack, 512),
        });
    }
    if lower.contains("429") || lower.contains("rate limit") {
        return Err(AgentError::RateLimited {
            provider: P.into(),
            retry_after_secs: None,
        });
    }
    Err(AgentError::Provider(format!(
        "codex transport failure (exit {exit_code:?}): {}",
        truncate_text(&haystack, 512)
    )))
}

async fn kill_codex_tree(child: &mut tokio::process::Child) {
    #[cfg(windows)]
    {
        if let Some(pid) = child.id() {
            let mut cmd = tokio::process::Command::new("taskkill");
            cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::null());
            #[cfg(windows)]
            {
                cmd.creation_flags(0x08000000);
            }
            if let Ok(mut killer) = cmd.spawn() {
                let _ = tokio::time::timeout(Duration::from_secs(5), killer.wait()).await;
            }
        }
    }
    let _ = child.start_kill();
}

fn pick_codex_cwd(request: &AgentRequest) -> PathBuf {
    if request.context.workspace_root.is_dir() {
        request.context.workspace_root.clone()
    } else {
        std::env::temp_dir()
    }
}

fn write_temp_schema(schema: &str) -> Result<PathBuf, AgentError> {
    let path = std::env::temp_dir().join(format!(
        "omen-codex-schema-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&path, schema)
        .map_err(|e| AgentError::Provider(format!("codex schema file write failed: {e}")))?;
    Ok(path)
}

fn truncate_text(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...[truncated]", &s[..max])
    }
}

pub fn codex_descriptor(available: bool) -> crate::registry::ProviderDescriptor {
    crate::registry::ProviderDescriptor {
        id: CODEX_PROVIDER_ID.into(),
        name: "Codex (external reference route)".into(),
        // Server-assigned: Omen never passes -m. UNKNOWN, not invented.
        model: None,
        credential_source: Some("codex-auth (Codex-managed)".into()),
        capabilities: vec![
            crate::conformance::ProviderCapability::Reasoning
                .as_str()
                .into(),
            crate::conformance::ProviderCapability::StructuredResponse
                .as_str()
                .into(),
            crate::conformance::ProviderCapability::Proposal
                .as_str()
                .into(),
            crate::conformance::ProviderCapability::ToolProposal
                .as_str()
                .into(),
            crate::conformance::ProviderCapability::FailureDiagnosis
                .as_str()
                .into(),
        ],
        is_available: available,
    }
}
