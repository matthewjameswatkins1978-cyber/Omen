use crate::protocol::*;
use omen_client::OmenClient;
use omen_core::{InteractiveSessionId, composition, machine_contract};
use omen_knowledge::{
    ContentAddressedStore, Database, FactRegistry, canonical_workspace_db_path,
    canonical_workspace_db_path_readonly, resolve_workspace_dir,
};
use omen_semantic::SemanticProviderRegistry;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub struct McpServer {
    workspace_path: PathBuf,
    session_id: String,
    client: Option<OmenClient>,
}

impl McpServer {
    pub fn new(workspace_path: PathBuf, client: Option<OmenClient>) -> Self {
        let session_id = format!("sess_mcp_{}", &uuid::Uuid::new_v4().to_string()[..8]);
        Self {
            workspace_path,
            session_id,
            client,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub async fn handle_request(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        match req.method.as_str() {
            "initialize" => self.handle_initialize(req.id, req.params).await,
            "notifications/initialized" | "initialized" => {
                JsonRpcResponse::success(req.id, json!({}))
            }
            "ping" => JsonRpcResponse::success(req.id, json!({})),
            "tools/list" => self.handle_tools_list(req.id).await,
            "tools/call" => self.handle_tools_call(req.id, req.params).await,
            "resources/list" => self.handle_resources_list(req.id).await,
            "resources/read" => self.handle_resources_read(req.id, req.params).await,
            other => JsonRpcResponse::error(req.id, -32601, format!("Method not found: {other}")),
        }
    }

    async fn handle_initialize(&self, id: Option<Value>, params: Option<Value>) -> JsonRpcResponse {
        if let Some(p) = params
            && let Some(ver) = p.get("protocolVersion").and_then(|v| v.as_str())
        {
            // Section 18: No silent reinterpretation of incompatible requests
            if ver != LATEST_MCP_PROTOCOL_VERSION && ver != "2024-10-07" {
                return JsonRpcResponse::error(
                    id,
                    -32002,
                    format!(
                        "Unsupported MCP protocol version: '{ver}', expected '{LATEST_MCP_PROTOCOL_VERSION}'"
                    ),
                );
            }
        }

        let result = InitializeResult {
            protocol_version: LATEST_MCP_PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability {
                    list_changed: Some(false),
                }),
                resources: Some(ResourcesCapability {
                    subscribe: Some(false),
                    list_changed: Some(false),
                }),
            },
            server_info: Implementation {
                name: "omen-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
    }

    async fn handle_tools_list(&self, id: Option<Value>) -> JsonRpcResponse {
        let tools = vec![
            ToolDefinition {
                name: "omen_orient".into(),
                description: "Read-only bootstrap discovery: canonical contract, digest, context status, capability groups, recipes, and next calls.".into(),
                input_schema: json!({"type":"object","properties":{}}),
            },
            ToolDefinition {
                name: "omen_capabilities".into(),
                description: "Read-only projection of canonical capability definitions and runtime overlays, optionally filtered by group.".into(),
                input_schema: json!({"type":"object","properties":{"group":{"type":"string"}}}),
            },
            ToolDefinition {
                name: "omen_describe".into(),
                description: "Read-only canonical definition plus runtime overlay for one capability.".into(),
                input_schema: json!({"type":"object","required":["capability_id"],"properties":{"capability_id":{"type":"string"}}}),
            },
            ToolDefinition {
                name: "omen_recipe".into(),
                description: "Read-only advisory recipe showing canonical capability steps; it grants no authority.".into(),
                input_schema: json!({"type":"object","required":["recipe_id"],"properties":{"recipe_id":{"type":"string"}}}),
            },
            ToolDefinition {
                name: "omen_context".into(),
                description: "Read-only current context snapshot, or DELTA_UNAVAILABLE for an unretained historical generation.".into(),
                input_schema: json!({"type":"object","properties":{"since":{"type":"integer","minimum":0}}}),
            },
            ToolDefinition {
                name: "omen_action_list".into(),
                description: "Read-only list of actions from the canonical Omen.toml parser; no probing or execution.".into(),
                input_schema: json!({"type":"object","properties":{}}),
            },
            ToolDefinition {
                name: "omen_action_show".into(),
                description: "Read-only display of one canonical Omen.toml action definition.".into(),
                input_schema: json!({"type":"object","required":["action_id"],"properties":{"action_id":{"type":"string"}}}),
            },
            ToolDefinition {
                name: "omen_action_plan".into(),
                description: "Build a deterministic read-only action plan and digest from canonical Omen.toml semantics.".into(),
                input_schema: json!({"type":"object","required":["action_id"],"properties":{"action_id":{"type":"string"}}}),
            },
            ToolDefinition {
                name: "omen_workspace_status".into(),
                description: "Query Omen structured workspace status, active facts, dirty count, and services.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                }),
            },
            ToolDefinition {
                name: "omen_facts_query".into(),
                description: "Query current and dirty facts published in this workspace.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "filter": {
                            "type": "string",
                            "enum": ["all", "current", "dirty"],
                            "description": "Filter facts by validity state (default: all)"
                        }
                    },
                }),
            },
            ToolDefinition {
                name: "omen_execute".into(),
                description: "Submit execution through Omen shared daemon broker. Executes physical work and records structured evidence.".into(),
                input_schema: json!({
                    "type": "object",
                    "required": ["argv"],
                    "properties": {
                        "tool": { "type": "string", "description": "Optional tool name (e.g. 'cargo', 'git')" },
                        "operation": { "type": "string", "description": "Optional tool operation (e.g. 'check', 'test')" },
                        "argv": { "type": "array", "items": { "type": "string" }, "description": "Command arguments" },
                        "cwd": { "type": "string", "description": "Working directory override" },
                        "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds (default 60000)" }
                    }
                }),
            },
            ToolDefinition {
                name: "omen_execution_status".into(),
                description: "Query status of an execution by consequential request receipt ID or execution ID.".into(),
                input_schema: json!({
                    "type": "object",
                    "required": ["request_id"],
                    "properties": {
                        "request_id": { "type": "string", "description": "Consequential request ID" }
                    }
                }),
            },
            ToolDefinition {
                name: "omen_history_query".into(),
                description: "Query subordinate physical execution history.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "all_sessions": { "type": "boolean", "description": "Whether to query all sessions or only current session" }
                    }
                }),
            },
            ToolDefinition {
                name: "omen_services_list".into(),
                description: "List managed background services running in the workspace.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolDefinition {
                name: "omen_services_control".into(),
                description: "Start, stop, or restart a managed background service.".into(),
                input_schema: json!({
                    "type": "object",
                    "required": ["action", "name"],
                    "properties": {
                        "action": { "type": "string", "enum": ["start", "stop", "restart"] },
                        "name": { "type": "string", "description": "Service name" },
                        "command": { "type": "string", "description": "Command to run (for start)" }
                    }
                }),
            },
            ToolDefinition {
                name: "omen_capabilities_discover".into(),
                description: "Discover Omen runtime physical capabilities and constraints.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolDefinition {
                name: "omen_symbol_search".into(),
                description: "Search for symbols across workspace semantic providers (LSP, SCIP).".into(),
                input_schema: json!({
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": { "type": "string", "description": "Symbol name or substring" },
                        "limit": { "type": "integer", "description": "Maximum results (default: 50)" }
                    }
                }),
            },
            ToolDefinition {
                name: "omen_symbol_definition".into(),
                description: "Find exact definition location for a symbol across workspace providers.".into(),
                input_schema: json!({
                    "type": "object",
                    "required": ["symbol"],
                    "properties": {
                        "symbol": { "type": "string", "description": "Exact symbol name" },
                        "file": { "type": "string", "description": "Optional file path hint" },
                        "line": { "type": "integer", "description": "Optional 0-indexed line hint" },
                        "col": { "type": "integer", "description": "Optional 0-indexed column hint" }
                    }
                }),
            },
            ToolDefinition {
                name: "omen_symbol_references".into(),
                description: "Find call sites and references to a symbol across workspace providers.".into(),
                input_schema: json!({
                    "type": "object",
                    "required": ["symbol"],
                    "properties": {
                        "symbol": { "type": "string", "description": "Symbol identifier" },
                        "file": { "type": "string", "description": "Optional file path hint" },
                        "line": { "type": "integer", "description": "Optional 0-indexed line hint" },
                        "col": { "type": "integer", "description": "Optional 0-indexed column hint" },
                        "limit": { "type": "integer", "description": "Maximum references (default: 50)" }
                    }
                }),
            },
            ToolDefinition {
                name: "omen_structure_search".into(),
                description: "Search for code structure patterns using ast-grep syntax matching.".into(),
                input_schema: json!({
                    "type": "object",
                    "required": ["pattern"],
                    "properties": {
                        "pattern": { "type": "string", "description": "ast-grep structural pattern (e.g. 'fn $NAME($$$) { $$$ }')" },
                        "language": { "type": "string", "description": "Language (default: 'rust')" },
                        "limit": { "type": "integer", "description": "Maximum matches (default: 50)" }
                    }
                }),
            },
            ToolDefinition {
                name: "omen_package_query".into(),
                description: "Query workspace packages, dependencies, targets, and tasks across ecosystems (Cargo, npm, uv, go, Docker).".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "Optional package or target name to filter" }
                    }
                }),
            },
        ];

        JsonRpcResponse::success(id, json!({ "tools": tools }))
    }

    async fn handle_tools_call(&self, id: Option<Value>, params: Option<Value>) -> JsonRpcResponse {
        let params = match params {
            Some(p) => p,
            None => return JsonRpcResponse::error(id, -32602, "Missing params for tools/call"),
        };

        let tool_name = match params.get("name").and_then(|v| v.as_str()) {
            Some(n) => n,
            None => {
                return JsonRpcResponse::error(id, -32602, "Missing 'name' in tools/call params");
            }
        };

        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        let result = match tool_name {
            "omen_orient" => self.tool_orient().await,
            "omen_capabilities" => self.tool_capabilities(&arguments).await,
            "omen_describe" => self.tool_describe(&arguments).await,
            "omen_recipe" => self.tool_recipe(&arguments).await,
            "omen_context" => self.tool_context(&arguments).await,
            "omen_action_list" => self.tool_action_list().await,
            "omen_action_show" => self.tool_action_show(&arguments).await,
            "omen_action_plan" => self.tool_action_plan(&arguments).await,
            "omen_workspace_status" => self.tool_workspace_status().await,
            "omen_facts_query" => self.tool_facts_query(&arguments).await,
            "omen_execute" => self.tool_execute(&arguments).await,
            "omen_execution_status" => self.tool_execution_status(&arguments).await,
            "omen_history_query" => self.tool_history_query(&arguments).await,
            "omen_services_list" => self.tool_services_list().await,
            "omen_services_control" => self.tool_services_control(&arguments).await,
            "omen_capabilities_discover" => self.tool_capabilities_discover().await,
            "omen_symbol_search" => self.tool_symbol_search(&arguments).await,
            "omen_symbol_definition" => self.tool_symbol_definition(&arguments).await,
            "omen_symbol_references" => self.tool_symbol_references(&arguments).await,
            "omen_structure_search" => self.tool_structure_search(&arguments).await,
            "omen_package_query" => self.tool_package_query(&arguments).await,
            other => CallToolResult::error(format!("Unknown tool: '{other}'")),
        };

        JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
    }

    fn machine_context(&self) -> machine_contract::MachineContext {
        let generation =
            Database::open_read_only(&canonical_workspace_db_path_readonly(&self.workspace_path))
                .ok()
                .and_then(|db| FactRegistry::get_generation_if_present(&db, "fs:workspace").ok())
                .flatten();
        machine_contract::context_with_generation(generation)
    }

    async fn tool_orient(&self) -> CallToolResult {
        let contract = machine_contract::contract();
        let context = self.machine_context();
        let groups: Vec<String> = contract
            .capability_definitions
            .iter()
            .map(|definition| definition.group.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        CallToolResult::text(serde_json::to_string_pretty(&json!({
            "contract_version": machine_contract::CONTRACT_VERSION,
            "omen_version": env!("CARGO_PKG_VERSION"),
            "contract_digest": machine_contract::contract_digest(),
            "context_generation": context.context_generation,
            "generation_status": context.generation_status,
            "workspace": {"name": self.workspace_path.file_name().and_then(|s| s.to_str()).unwrap_or("workspace"), "root": "."},
            "platform": std::env::consts::OS,
            "backend": "native",
            "capability_groups": groups,
            "references": ["@last", "@failed"],
            "recipes": contract.recipe_definitions.iter().map(|recipe| &recipe.id).collect::<Vec<_>>(),
            "next": ["capabilities", "describe <capability>", "recipe <recipe>", "context"]
        })).unwrap())
    }

    async fn tool_capabilities(&self, args: &Value) -> CallToolResult {
        let group = args.get("group").and_then(Value::as_str);
        let contract = machine_contract::contract();
        let context = self.machine_context();
        let capabilities: Vec<_> = machine_contract::project(&contract, &context)
            .into_iter()
            .filter(|entry| group.is_none_or(|wanted| entry.definition.group == wanted))
            .collect();
        CallToolResult::text(
            serde_json::to_string_pretty(&json!({"capabilities": capabilities})).unwrap(),
        )
    }

    async fn tool_describe(&self, args: &Value) -> CallToolResult {
        let Some(id) = args.get("capability_id").and_then(Value::as_str) else {
            return CallToolResult::error("Missing 'capability_id'");
        };
        let Some(definition) = machine_contract::capability(id) else {
            return CallToolResult::error(serde_json::to_string(&json!({
                "error":"CAPABILITY_NOT_FOUND", "operation":id, "next_actions":["omen_capabilities"]
            })).unwrap());
        };
        let context = self.machine_context();
        let status = context
            .capability_statuses
            .into_iter()
            .find(|status| status.id == id)
            .unwrap();
        CallToolResult::text(
            serde_json::to_string_pretty(&machine_contract::CapabilityProjection {
                definition,
                status,
            })
            .unwrap(),
        )
    }

    async fn tool_recipe(&self, args: &Value) -> CallToolResult {
        let Some(id) = args.get("recipe_id").and_then(Value::as_str) else {
            return CallToolResult::error("Missing 'recipe_id'");
        };
        match machine_contract::recipe(id) {
            Some(recipe) => CallToolResult::text(serde_json::to_string_pretty(&recipe).unwrap()),
            None => CallToolResult::error(
                serde_json::to_string(&json!({
                    "error":"RECIPE_NOT_FOUND", "operation":id, "next_actions":["omen_orient"]
                }))
                .unwrap(),
            ),
        }
    }

    async fn tool_context(&self, args: &Value) -> CallToolResult {
        let context = self.machine_context();
        match args.get("since").and_then(Value::as_i64) {
            None => CallToolResult::text(serde_json::to_string_pretty(&json!({
                "context_generation":context.context_generation,
                "generation_status":context.generation_status,
                "delta":"CURRENT_SNAPSHOT",
                "workspace":{"root":"."},
                "provider_status":"not_probed"
            })).unwrap()),
            Some(generation) if Some(generation) == context.context_generation =>
                CallToolResult::text(serde_json::to_string_pretty(&json!({"changed":false,"from_generation":generation,"context_generation":generation,"changes":[]})).unwrap()),
            Some(generation) => CallToolResult::text(serde_json::to_string_pretty(&json!({
                "error":"DELTA_UNAVAILABLE", "from_generation":generation, "state_changed":false,
                "retryable":true, "context_generation":context.context_generation,
                "reason":"Omen does not retain that historical generation", "next_actions":["omen_context"]
            })).unwrap()),
        }
    }

    fn load_action_config(
        &self,
    ) -> Result<Option<omen_core::composition::OmenWorkspaceConfig>, String> {
        let path = self.workspace_path.join("Omen.toml");
        if !path.exists() {
            return Ok(None);
        }
        let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
        if metadata.len() > 256 * 1024 {
            return Err("OMEN_CONFIG_TOO_LARGE".into());
        }
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let config = toml::from_str(&text).map_err(|e| e.to_string())?;
        composition::validate_config(&config).map_err(|e| e.to_string())?;
        Ok(Some(config))
    }

    async fn tool_action_list(&self) -> CallToolResult {
        match self.load_action_config() {
            Ok(None) => CallToolResult::text(serde_json::to_string_pretty(&json!({"config_present":false,"actions":[]})).unwrap()),
            Ok(Some(config)) => CallToolResult::text(serde_json::to_string_pretty(&json!({
                "config_present":true,
                "schema_version":config.schema_version,
                "project":config.project.as_ref().and_then(|project| project.name.clone()),
                "actions":config.actions.iter().map(|(id, action)| json!({"action_id":id,"description":action.description,"step_count":action.steps.len()})).collect::<Vec<_>>()
            })).unwrap()),
            Err(error) => CallToolResult::error(error),
        }
    }

    async fn tool_action_show(&self, args: &Value) -> CallToolResult {
        let Some(id) = args.get("action_id").and_then(Value::as_str) else {
            return CallToolResult::error("Missing 'action_id'");
        };
        match self.load_action_config() {
            Ok(Some(config)) => match config.actions.get(id) {
                Some(action) => CallToolResult::text(serde_json::to_string_pretty(action).unwrap()),
                None => {
                    CallToolResult::error(format!("ACTION_NOT_FOUND: action '{id}' does not exist"))
                }
            },
            Ok(None) => CallToolResult::error("ACTION_NOT_FOUND: Omen.toml is not present"),
            Err(error) => CallToolResult::error(error),
        }
    }

    async fn tool_action_plan(&self, args: &Value) -> CallToolResult {
        let Some(id) = args.get("action_id").and_then(Value::as_str) else {
            return CallToolResult::error("Missing 'action_id'");
        };
        match self.load_action_config() {
            Ok(Some(config)) => match composition::plan_action(
                &config,
                id,
                &machine_contract::contract(),
                &self.machine_context(),
            ) {
                Ok(plan) => CallToolResult::text(serde_json::to_string_pretty(&plan).unwrap()),
                Err(error) => CallToolResult::error(error.to_string()),
            },
            Ok(None) => CallToolResult::error("ACTION_NOT_FOUND: Omen.toml is not present"),
            Err(error) => CallToolResult::error(error),
        }
    }

    async fn tool_workspace_status(&self) -> CallToolResult {
        if let Some(ref c) = self.client
            && let Ok(snap) = c.get_snapshot().await
        {
            return CallToolResult::text(
                serde_json::to_string_pretty(&json!({
                    "workspace_id": snap.workspace_id,
                    "workspace_path": self.workspace_path.display().to_string(),
                    "epoch": snap.epoch,
                    "sequence": snap.sequence,
                    "active_facts_count": snap.facts.len(),
                    "dirty_facts_count": snap.dirty_facts_count,
                    "services_count": snap.services.len(),
                    "available_tools": snap.tools,
                }))
                .unwrap(),
            );
        }

        CallToolResult::text(
            serde_json::to_string_pretty(&json!({
                "workspace_path": self.workspace_path.display().to_string(),
                "workspace_root": self.workspace_path.display().to_string(),
                "mode": "standalone",
            }))
            .unwrap(),
        )
    }

    async fn tool_facts_query(&self, args: &Value) -> CallToolResult {
        let filter = args.get("filter").and_then(|v| v.as_str()).unwrap_or("all");

        if let Some(ref c) = self.client
            && let Ok(snap) = c.get_snapshot().await
        {
            let filtered: Vec<_> = snap
                .facts
                .into_iter()
                .filter(|f| match filter {
                    "current" => f.validity == "CURRENT",
                    "dirty" => f.validity == "DIRTY",
                    _ => true,
                })
                .collect();

            return CallToolResult::text(serde_json::to_string_pretty(&filtered).unwrap());
        }

        CallToolResult::text("[]")
    }

    async fn tool_execute(&self, args: &Value) -> CallToolResult {
        let argv = match args.get("argv").and_then(|v| v.as_array()) {
            Some(arr) => arr
                .iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect::<Vec<String>>(),
            None => return CallToolResult::error("Missing 'argv' array"),
        };

        if argv.is_empty() {
            return CallToolResult::error("'argv' cannot be empty");
        }

        let tool = args.get("tool").and_then(|v| v.as_str()).unwrap_or("exec");
        let operation = args.get("operation").and_then(|v| v.as_str()).unwrap_or("");
        let cwd = args
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| self.workspace_path.to_str().unwrap_or("."));
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(60000);

        if let Some(ref c) = self.client {
            match c
                .submit_execution(tool, operation, argv, cwd, timeout_ms)
                .await
            {
                Ok(summary) => {
                    let out = json!({
                        "execution_id": summary.execution_id,
                        "exit_code": summary.exit_code,
                        "duration_ms": summary.duration_ms,
                        "stdout_preview": summary.stdout_preview,
                        "stderr_preview": summary.stderr_preview,
                        "stdout_artifact": summary.stdout_artifact,
                        "stderr_artifact": summary.stderr_artifact,
                    });
                    CallToolResult::text(serde_json::to_string_pretty(&out).unwrap())
                }
                Err(e) => CallToolResult::error(format!("Execution broker error: {e}")),
            }
        } else {
            let supervisor = omen_engine::ProcessSupervisor::new();
            let req = omen_engine::ExecutionRequest {
                argv: argv.clone(),
                cwd: std::path::PathBuf::from(cwd),
                env: vec![],
                stdin_mode: omen_core::StdioMode::Closed,
                stdin_payload: None,
                timeout_ms,
                inline_budget: 8192,
                required_assurance: omen_core::RequiredAssurance::default(),
                secrets: vec![],
            };
            match supervisor.execute(req).await {
                Ok(output) => {
                    let state_dir = resolve_workspace_dir(&self.workspace_path);
                    let db_path = canonical_workspace_db_path(&self.workspace_path);
                    let cas_dir = state_dir.join("cas");
                    let cas = ContentAddressedStore::new(cas_dir);

                    let mut db = Database::open(&db_path).ok();
                    let stdout_artifact = if !output.stdout_all.is_empty() {
                        if let Some(ref mut d) = db {
                            cas.store(
                                d,
                                &output.stdout_all,
                                "text/plain",
                                "agent",
                                omen_core::RetentionClass::Referenced,
                            )
                            .ok()
                            .map(|m| m.uri.to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let stderr_artifact = if !output.stderr_all.is_empty() {
                        if let Some(ref mut d) = db {
                            cas.store(
                                d,
                                &output.stderr_all,
                                "text/plain",
                                "agent",
                                omen_core::RetentionClass::Referenced,
                            )
                            .ok()
                            .map(|m| m.uri.to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    let out = json!({
                        "execution_id": format!("exec-local-{}", &uuid::Uuid::new_v4().to_string()[..8]),
                        "exit_code": output.process_exit.code.unwrap_or(0),
                        "duration_ms": 0,
                        "stdout_preview": String::from_utf8_lossy(&output.stdout_bounded).to_string(),
                        "stderr_preview": String::from_utf8_lossy(&output.stderr_bounded).to_string(),
                        "stdout_artifact": stdout_artifact,
                        "stderr_artifact": stderr_artifact,
                    });
                    CallToolResult::text(serde_json::to_string_pretty(&out).unwrap())
                }
                Err(e) => CallToolResult::error(format!("Local execution error: {e}")),
            }
        }
    }

    async fn tool_execution_status(&self, args: &Value) -> CallToolResult {
        let req_id = match args.get("request_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => return CallToolResult::error("Missing 'request_id'"),
        };

        if let Some(ref c) = self.client {
            match c.query_request_status(req_id).await {
                Ok(receipt) => {
                    CallToolResult::text(serde_json::to_string_pretty(&receipt).unwrap())
                }
                Err(e) => CallToolResult::error(format!("Query status error: {e}")),
            }
        } else {
            CallToolResult::error("Shared daemon client not connected")
        }
    }

    async fn tool_history_query(&self, args: &Value) -> CallToolResult {
        let all = args
            .get("all_sessions")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let db_path = canonical_workspace_db_path(&self.workspace_path);

        if let Ok(db) = Database::open(&db_path) {
            if all {
                let records = omen_knowledge::ExecutionHistory::list_all_executions(&db, 20)
                    .unwrap_or_default();
                CallToolResult::text(serde_json::to_string_pretty(&records).unwrap())
            } else {
                let sid = InteractiveSessionId::new(&self.session_id).unwrap();
                let last = omen_knowledge::ExecutionHistory::get_last_execution(&db, &sid)
                    .unwrap_or_default();
                CallToolResult::text(serde_json::to_string_pretty(&last).unwrap())
            }
        } else {
            CallToolResult::error("Failed to open canonical workspace database")
        }
    }

    async fn tool_services_list(&self) -> CallToolResult {
        if let Some(ref c) = self.client {
            match c.list_services().await {
                Ok(services) => {
                    CallToolResult::text(serde_json::to_string_pretty(&services).unwrap())
                }
                Err(e) => CallToolResult::error(format!("List services error: {e}")),
            }
        } else {
            CallToolResult::error("Shared daemon client not connected")
        }
    }

    async fn tool_services_control(&self, args: &Value) -> CallToolResult {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return CallToolResult::error("Missing 'action'"),
        };
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) => n,
            None => return CallToolResult::error("Missing 'name'"),
        };

        if let Some(ref c) = self.client {
            let res: Result<String, omen_ipc::LocalIpcError> = match action {
                "start" => {
                    let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                    c.start_service(name, cmd, vec![])
                        .await
                        .map(|s| format!("Service '{}' started (PID {:?})", s.name, s.pid))
                }
                "stop" => c.stop_service(name).await,
                "restart" => c
                    .restart_service(name)
                    .await
                    .map(|s| format!("Service '{}' restarted (PID {:?})", s.name, s.pid)),
                other => Err(omen_ipc::LocalIpcError::Io(format!(
                    "Unknown service action: {other}"
                ))),
            };

            match res {
                Ok(msg) => CallToolResult::text(format!(
                    "Service action '{action}' on '{name}' completed: {msg}"
                )),
                Err(e) => CallToolResult::error(format!("Service control error: {e}")),
            }
        } else {
            CallToolResult::error("Shared daemon client not connected")
        }
    }

    async fn tool_capabilities_discover(&self) -> CallToolResult {
        let caps = json!({
            "product": "Omen",
            "version": env!("CARGO_PKG_VERSION"),
            "doctrine": "substrate, not sovereign",
            "boundaries": {
                "lantern": "memory and provenance",
                "resolve": "live guards and locks",
                "tethers": "permissions, policy, approval, durable intent, and outcome truth",
                "omen": "physical execution, containment, facts, and CAS artifacts",
                "threadmoth": "deterministic bounded structural mutation"
            },
            "tools": [
                "omen_workspace_status",
                "omen_facts_query",
                "omen_execute",
                "omen_execution_status",
                "omen_history_query",
                "omen_services_list",
                "omen_services_control",
                "omen_capabilities_discover"
            ],
            "capabilities": {
                "execution": {
                    "broker": "shared_daemon",
                    "containment": if cfg!(windows) { "JobObjects" } else { "ProcessGroup" },
                    "closed_stdin_default": true,
                    "bounded_context": true
                },
                "facts": {
                    "registry": "lazy_pessimism",
                    "states": ["CURRENT", "DIRTY", "SUPERSEDED"]
                },
                "cas": {
                    "hasher": "sha256",
                    "uri_scheme": "artifact://sha256/<hash>",
                    "storage": "content_addressed"
                },
                "services": {
                    "supervision": "managed",
                    "ownership_states": ["RUNNING_OWNED", "OBSERVED", "STOPPED", "CRASHED"]
                },
                "authority": {
                    "sovereign": false,
                    "doctrine": "Omen is substrate, not sovereign. Authority belongs to Tethers."
                }
            }
        });
        CallToolResult::text(serde_json::to_string_pretty(&caps).unwrap())
    }

    fn build_registry(&self) -> std::sync::Arc<SemanticProviderRegistry> {
        omen_adapters::get_workspace_semantic_registry(&self.workspace_path)
    }

    async fn tool_symbol_search(&self, args: &Value) -> CallToolResult {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let reg = self.build_registry();
        match reg.symbol_search(query, limit, None).await {
            Ok(symbols) => CallToolResult::text(serde_json::to_string_pretty(&symbols).unwrap()),
            Err(e) => CallToolResult::error(format!("Symbol search failed: {e}")),
        }
    }

    async fn tool_symbol_definition(&self, args: &Value) -> CallToolResult {
        let symbol = match args.get("symbol").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return CallToolResult::error("Missing 'symbol' parameter"),
        };
        let file = args.get("file").and_then(|v| v.as_str());
        let line = args
            .get("line")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let col = args.get("col").and_then(|v| v.as_u64()).map(|n| n as usize);
        let reg = self.build_registry();
        match reg.find_definition(symbol, file, line, col, None).await {
            Ok(res) => CallToolResult::text(serde_json::to_string_pretty(&res).unwrap()),
            Err(e) => CallToolResult::error(format!("Symbol definition lookup failed: {e}")),
        }
    }

    async fn tool_symbol_references(&self, args: &Value) -> CallToolResult {
        let symbol = match args.get("symbol").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return CallToolResult::error("Missing 'symbol' parameter"),
        };
        let file = args.get("file").and_then(|v| v.as_str());
        let line = args
            .get("line")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let col = args.get("col").and_then(|v| v.as_u64()).map(|n| n as usize);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let reg = self.build_registry();
        match reg
            .find_references(symbol, file, line, col, limit, None)
            .await
        {
            Ok(res) => CallToolResult::text(serde_json::to_string_pretty(&res).unwrap()),
            Err(e) => CallToolResult::error(format!("Symbol references lookup failed: {e}")),
        }
    }

    async fn tool_structure_search(&self, args: &Value) -> CallToolResult {
        let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return CallToolResult::error("Missing 'pattern' parameter"),
        };
        let language = args
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("rust");
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);
        let reg = self.build_registry();
        match reg.structural_search(pattern, language, limit, None).await {
            Ok(matches) => CallToolResult::text(serde_json::to_string_pretty(&matches).unwrap()),
            Err(e) => CallToolResult::error(format!("Structural search failed: {e}")),
        }
    }

    async fn tool_package_query(&self, args: &Value) -> CallToolResult {
        let target = args.get("target").and_then(|v| v.as_str());
        let reg = self.build_registry();
        match reg.packages(None).await {
            Ok(mut pkgs) => {
                if let Some(t) = target {
                    pkgs.retain(|p| p.name == t || p.targets.iter().any(|tg| tg.name == t));
                }
                CallToolResult::text(serde_json::to_string_pretty(&pkgs).unwrap())
            }
            Err(e) => CallToolResult::error(format!("Package query failed: {e}")),
        }
    }

    async fn handle_resources_list(&self, id: Option<Value>) -> JsonRpcResponse {
        let resources = vec![
            ResourceDefinition {
                uri: "fact://".into(),
                name: "Omen Facts".into(),
                description: Some("Facts published in the Fact Registry".into()),
                mime_type: Some("application/json".into()),
            },
            ResourceDefinition {
                uri: "artifact://".into(),
                name: "CAS Artifacts".into(),
                description: Some(
                    "Content-addressed storage artifacts (stdout/stderr/evidence)".into(),
                ),
                mime_type: Some("text/plain".into()),
            },
            ResourceDefinition {
                uri: "proc://".into(),
                name: "Managed Services".into(),
                description: Some("Managed process supervisor resources".into()),
                mime_type: Some("application/json".into()),
            },
        ];

        JsonRpcResponse::success(id, json!({ "resources": resources }))
    }

    async fn handle_resources_read(
        &self,
        id: Option<Value>,
        params: Option<Value>,
    ) -> JsonRpcResponse {
        let uri = match params
            .and_then(|p| p.get("uri").and_then(|u| u.as_str()).map(|s| s.to_string()))
        {
            Some(u) => u,
            None => return JsonRpcResponse::error(id, -32602, "Missing 'uri' parameter"),
        };

        // Section 32: mcp_large_artifact_returns_reference_not_unbounded_payload
        if uri.starts_with("artifact://sha256/") {
            let digest = uri.trim_start_matches("artifact://sha256/");
            let state_dir = resolve_workspace_dir(&self.workspace_path);
            let cas = ContentAddressedStore::new(state_dir.join("cas"));
            let blob_path = cas.blob_path(digest);

            if !blob_path.exists() {
                return JsonRpcResponse::error(
                    id,
                    -32004,
                    format!("Artifact not found in CAS: {digest}"),
                );
            }

            match std::fs::read(&blob_path) {
                Ok(mut bytes) => {
                    // Bound payload to at most 64 KiB
                    if bytes.len() > 65536 {
                        bytes.truncate(65536);
                    }
                    let text = String::from_utf8_lossy(&bytes).to_string();
                    let contents = vec![ResourceContent {
                        uri: uri.clone(),
                        mime_type: Some("text/plain".into()),
                        text,
                    }];
                    return JsonRpcResponse::success(id, json!({ "contents": contents }));
                }
                Err(e) => {
                    return JsonRpcResponse::error(
                        id,
                        -32004,
                        format!("Failed to read artifact: {e}"),
                    );
                }
            }
        }

        JsonRpcResponse::error(
            id,
            -32004,
            format!("Resource not found or unsupported scheme: {uri}"),
        )
    }

    /// Dispatches a single JSON-RPC message string to a response string.
    pub async fn dispatch_message(&self, line: &str) -> Option<String> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        match serde_json::from_str::<JsonRpcRequest>(trimmed) {
            Ok(req) => {
                let resp = self.handle_request(req).await;
                serde_json::to_string(&resp).ok()
            }
            Err(e) => {
                let resp = JsonRpcResponse::error(None, -32700, format!("Parse error: {e}"));
                serde_json::to_string(&resp).ok()
            }
        }
    }

    /// Runs stream server loop over any async reader and writer.
    pub async fn run_stream<R, W>(
        &self,
        reader: R,
        mut writer: W,
    ) -> Result<(), Box<dyn std::error::Error>>
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        let mut lines = BufReader::new(reader).lines();
        while let Some(line) = lines.next_line().await? {
            if let Some(resp_str) = self.dispatch_message(&line).await {
                writer.write_all(resp_str.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await?;
            }
        }
        Ok(())
    }

    /// Runs stdio server loop for external agents.
    pub async fn run_stdio(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.run_stream(tokio::io::stdin(), tokio::io::stdout())
            .await
    }
}
