use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use omen_adapters::{DefaultCapabilityExecutor, execute_plan, get_workspace_semantic_registry};
use omen_atlas::{RuntimeProfile, ToolValidator};
use omen_core::composition::{self, OmenWorkspaceConfig};
use omen_core::machine_contract::{self, CapabilityProjection};
use omen_core::{
    ActionId, CoreError, ErrorCode, ExecutionContract, OmenError, RequiredAssurance, ResourceUri,
    StdioMode,
};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use omen_knowledge::{
    ContentAddressedStore, DEFAULT_HISTORY_LIMIT, Database, FactRegistry, HistoryQuery,
    canonical_workspace_db_path_readonly, query_history, workspace_state_dir_path,
};
use omen_schema::{ExecutionContractWire, ExecutionResultWire};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const MAX_OMEN_TOML_BYTES: u64 = 256 * 1024;

#[derive(Parser, Debug)]
#[command(
    name = "omen",
    author,
    version = env!("OMEN_BUILD_IDENTITY"),
    about = "Agent-native developer runtime. Substrate, not sovereign."
)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,

    /// Emit bounded deterministic machine-readable output.
    #[arg(long, global = true)]
    machine: bool,

    #[arg(long, global = true)]
    workspace: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Bootstrap an unfamiliar client with the bounded machine contract.
    Orient(OrientArgs),
    /// Generate shell completion for the CLI command tree.
    Completion { shell: Shell },
    /// List the static capability catalogue, optionally filtered by group.
    Capabilities { group: Option<String> },
    /// Inspect environment and health
    Doctor,
    /// Describe Omen runtime capabilities
    Describe(DescribeArgs),
    /// Explain a bounded advisory recipe.
    How { recipe: String },
    /// Report bounded live context or an unavailable historical delta.
    Context {
        #[arg(long)]
        since: Option<u64>,
    },
    /// Inspect and deterministically plan inert Omen.toml actions.
    Action(ActionArgs),
    /// Tool atlas management and inspection
    Tool(ToolArgs),
    /// Fact query and provenance
    Fact(FactArgs),
    /// Query bounded durable execution history.
    History(HistoryArgs),
    /// Execute an argv or contract
    Exec(ExecArgs),
    /// CAS artifact inspection and retrieval
    Artifact(ArtifactArgs),
    /// Ephemeral storage garbage collection
    Gc(GcArgs),
    /// Manage Omen shared runtime daemon
    Daemon(DaemonArgs),
    /// Model Context Protocol (MCP) server over stdio
    Mcp(McpArgs),
}

#[derive(Args, Debug)]
struct DescribeArgs {
    /// Capability ID to describe; omit for the legacy runtime description.
    capability: Option<String>,
}

#[derive(Args, Debug)]
struct OrientArgs {
    /// Compare the cached static contract digest without returning the full map.
    #[arg(long)]
    since: Option<String>,
}

#[derive(Args, Debug)]
struct HistoryArgs {
    /// Return only entries for the supplied interactive session.
    #[arg(long)]
    session_id: Option<String>,
    /// Query the current session instead of all sessions.
    #[arg(long)]
    current_session: bool,
    /// Maximum number of entries to return (1-100).
    #[arg(long, default_value_t = DEFAULT_HISTORY_LIMIT)]
    limit: usize,
}

#[derive(Args, Debug)]
struct ActionArgs {
    #[command(subcommand)]
    subcommand: ActionSubcommands,
}

#[derive(Subcommand, Debug)]
enum ActionSubcommands {
    /// List configured actions without probing or executing anything.
    List,
    /// Show one stored action definition.
    Show { action_id: String },
    /// Validate and build a deterministic read-only action plan.
    Plan { action_id: String },
    /// Execute exactly the freshly recomputed plan identified by --expect-plan.
    Run {
        action_id: String,
        #[arg(long)]
        expect_plan: String,
        /// Repeatable step-id=ExecutionContract JSON path binding.
        #[arg(long = "execution-contract")]
        execution_contract: Vec<String>,
    },
}

#[derive(Debug)]
struct ConfigLoadError {
    code: &'static str,
    message: String,
}

impl std::fmt::Display for ConfigLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ConfigLoadError {}

fn omen_config_path(workspace: &Path) -> Result<PathBuf, ConfigLoadError> {
    let root = workspace.canonicalize().map_err(|e| ConfigLoadError {
        code: "OMEN_CONFIG_INVALID",
        message: format!("cannot resolve workspace root: {e}"),
    })?;
    let path = root.join("Omen.toml");
    if !path.exists() {
        return Ok(path);
    }
    let resolved = path.canonicalize().map_err(|e| ConfigLoadError {
        code: "OMEN_CONFIG_INVALID",
        message: format!("cannot resolve Omen.toml: {e}"),
    })?;
    if !resolved.starts_with(&root) {
        return Err(ConfigLoadError {
            code: "OMEN_CONFIG_PATH_ESCAPE",
            message: "Omen.toml resolves outside the selected workspace".into(),
        });
    }
    Ok(resolved)
}

fn load_omen_config(workspace: &Path) -> Result<Option<OmenWorkspaceConfig>, ConfigLoadError> {
    let path = omen_config_path(workspace)?;
    if !path.exists() {
        return Ok(None);
    }
    let size = std::fs::metadata(&path)
        .map_err(|e| ConfigLoadError {
            code: "OMEN_CONFIG_INVALID",
            message: e.to_string(),
        })?
        .len();
    if size > MAX_OMEN_TOML_BYTES {
        return Err(ConfigLoadError {
            code: "OMEN_CONFIG_TOO_LARGE",
            message: format!("Omen.toml is {size} bytes; maximum is {MAX_OMEN_TOML_BYTES}"),
        });
    }
    let text = std::fs::read_to_string(&path).map_err(|e| ConfigLoadError {
        code: "OMEN_CONFIG_INVALID",
        message: e.to_string(),
    })?;
    let config: OmenWorkspaceConfig = toml::from_str(&text).map_err(|e| ConfigLoadError {
        code: "OMEN_CONFIG_INVALID",
        message: e.to_string(),
    })?;
    composition::validate_config(&config).map_err(|e| ConfigLoadError {
        code: "OMEN_CONFIG_INVALID",
        message: e.to_string(),
    })?;
    Ok(Some(config))
}

fn print_composition_error(error: impl std::fmt::Display, code: &str) {
    let mut envelope = OmenError::from_code(
        ErrorCode::parse(code).unwrap_or(ErrorCode::Internal),
        error.to_string(),
    );
    if ErrorCode::parse(code).is_none() {
        envelope.details = serde_json::json!({"legacy_code": code});
    }
    println!("{}", serde_json::json!({"ok":false,"error":envelope}));
}

fn bundled_tool_profile(tool_id: &str) -> Result<RuntimeProfile, CoreError> {
    let content = match tool_id {
        "cargo" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../profiles/cargo.toml"
        )),
        "git" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../profiles/git.toml"
        )),
        "ripgrep" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../profiles/ripgrep.toml"
        )),
        "threadmoth" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../profiles/threadmoth.toml"
        )),
        other => {
            return Err(CoreError::NotFound(format!(
                "Unknown tool profile '{other}'"
            )));
        }
    };
    RuntimeProfile::from_toml_str(content)
}

fn load_execution_contracts(
    bindings: &[String],
) -> Result<std::collections::BTreeMap<String, ExecutionContract>, ConfigLoadError> {
    let mut contracts = std::collections::BTreeMap::new();
    for binding in bindings {
        let (step_id, path) = binding.split_once('=').ok_or_else(|| ConfigLoadError {
            code: "AUTHORITY_CONTRACT_INVALID",
            message: format!("expected <step-id>=<path>, got '{binding}'"),
        })?;
        if step_id.is_empty() || path.is_empty() {
            return Err(ConfigLoadError {
                code: "AUTHORITY_CONTRACT_INVALID",
                message: format!("invalid authority binding '{binding}'"),
            });
        }
        if contracts.contains_key(step_id) {
            return Err(ConfigLoadError {
                code: "AUTHORITY_CONTRACT_INVALID",
                message: format!("duplicate authority mapping for step '{step_id}'"),
            });
        }
        let content = std::fs::read_to_string(path).map_err(|e| ConfigLoadError {
            code: "AUTHORITY_CONTRACT_INVALID",
            message: format!("cannot read contract for '{step_id}': {e}"),
        })?;
        let wire: ExecutionContractWire =
            serde_json::from_str(&content).map_err(|e| ConfigLoadError {
                code: "AUTHORITY_CONTRACT_INVALID",
                message: format!("invalid contract for '{step_id}': {e}"),
            })?;
        let contract = ExecutionContract::try_from(wire).map_err(|e| ConfigLoadError {
            code: "AUTHORITY_CONTRACT_INVALID",
            message: e.to_string(),
        })?;
        contracts.insert(step_id.to_owned(), contract);
    }
    Ok(contracts)
}

#[derive(Args, Debug)]
struct McpArgs {
    /// Workspace root directory to serve
    #[arg(long)]
    workspace: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct DaemonArgs {
    #[command(subcommand)]
    subcommand: DaemonSubcommands,
}

#[derive(Subcommand, Debug)]
enum DaemonSubcommands {
    /// Start the omend daemon
    Start {
        /// Run daemon in foreground
        #[arg(long)]
        foreground: bool,
    },
    /// Stop the running omend daemon
    Stop,
    /// Inspect omend daemon status
    Status,
    /// Ping the omend daemon
    Ping,
}

#[derive(Args, Debug)]
struct ToolArgs {
    #[command(subcommand)]
    subcommand: ToolSubcommands,
}

#[derive(Subcommand, Debug)]
enum ToolSubcommands {
    /// List known tool profiles
    List,
    /// Inspect a specific tool profile
    Inspect { tool_id: String },
    /// Validate tool availability, version, and health
    Validate { tool_id: String },
}

#[derive(Args, Debug)]
struct FactArgs {
    #[command(subcommand)]
    subcommand: FactSubcommands,
}

#[derive(Subcommand, Debug)]
enum FactSubcommands {
    /// Get fact by logical URI
    Get {
        uri: String,
        #[arg(long)]
        require_current: bool,
    },
    /// Inspect fact provenance and dependency history
    Why { uri: String },
}

#[derive(Args, Debug)]
struct ExecArgs {
    /// Logical tool URI or binary name
    argv: Vec<String>,
    /// Execution timeout in milliseconds
    #[arg(long, default_value = "30000")]
    timeout_ms: u64,
    /// Inline output budget in bytes
    #[arg(long, default_value = "8192")]
    budget: usize,
    /// Path to Tethers execution contract JSON
    #[arg(long)]
    contract: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct ArtifactArgs {
    #[command(subcommand)]
    subcommand: ArtifactSubcommands,
}

#[derive(Subcommand, Debug)]
enum ArtifactSubcommands {
    /// Read slice of artifact
    Read {
        hash: String,
        #[arg(long, default_value = "0")]
        offset: u64,
        #[arg(long, default_value = "8192")]
        length: usize,
    },
    /// Inspect artifact metadata
    Inspect { hash: String },
}

#[derive(Args, Debug)]
struct GcArgs {
    /// Dry run without deleting artifacts
    #[arg(long)]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let json_mode = cli.json || cli.machine;

    let current_dir = std::env::current_dir()?;
    let ws_root = cli.workspace.unwrap_or(current_dir);
    let state_dir = workspace_state_dir_path(&ws_root);
    let db_path = canonical_workspace_db_path_readonly(&ws_root);
    let cas_dir = state_dir.join("cas");
    let context_generation = Database::open_read_only(&db_path)
        .ok()
        .and_then(|db| FactRegistry::get_generation_if_present(&db, "fs:workspace").ok())
        .flatten();
    let machine_context = machine_contract::context_with_generation(context_generation);

    match cli.command {
        Some(Commands::Completion { shell }) => {
            let mut command = Cli::command();
            generate(shell, &mut command, "omen", &mut std::io::stdout());
            return Ok(());
        }
        Some(Commands::Orient(args)) => {
            let digest = machine_contract::contract_digest();
            if let Some(previous) = args.since {
                let doc = if previous == digest {
                    serde_json::json!({"changed": false, "contract_digest": digest})
                } else {
                    serde_json::json!({
                        "changed": true,
                        "delta_available": false,
                        "previous_digest": previous,
                        "contract_digest": digest,
                        "next_actions": [{
                            "operation": "orient",
                            "cli": "orient --machine",
                            "mcp_tool": "omen_orient",
                            "purpose": "refresh the static discovery digest"
                        }]
                    })
                };
                println!("{}", serde_json::to_string_pretty(&doc)?);
                return Ok(());
            }
            let contract = machine_contract::contract();
            let capability_groups: Vec<String> = contract
                .capability_definitions
                .iter()
                .map(|definition| definition.group.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let doc = serde_json::json!({
                "contract_version": machine_contract::CONTRACT_VERSION,
                "omen_version": env!("CARGO_PKG_VERSION"),
                "contract_digest": digest,
                "context_generation": machine_context.context_generation,
                "generation_status": machine_context.generation_status,
                "workspace": {"name": ws_root.file_name().and_then(|s| s.to_str()).unwrap_or("workspace"), "root": "."},
                "platform": std::env::consts::OS,
                "backend": "native",
                "capability_groups": capability_groups,
                "references": ["@last", "@failed"],
                "recipes": machine_contract::contract().recipe_definitions.iter().map(|recipe| &recipe.id).collect::<Vec<_>>(),
                "next": ["capabilities", "history", "describe <capability>", "how <recipe>", "context --since <generation>"],
                "next_actions": [
                    {"operation":"capabilities","cli":"capabilities [group] --machine","mcp_tool":"omen_capabilities","purpose":"select a relevant capability from the compact catalogue"},
                    {"operation":"describe","cli":"describe <capability> --machine","mcp_tool":"omen_describe","purpose":"load one capability's operational schema and constraints"},
                    {"operation":"recipe","cli":"how <recipe> --machine","mcp_tool":"omen_recipe","purpose":"follow a short advisory multi-step path"},
                    {"operation":"context","cli":"context --machine","mcp_tool":"omen_context","purpose":"refresh dynamic workspace/session state"}
                ]
            });
            if json_mode {
                println!("{}", serde_json::to_string_pretty(&doc)?);
            } else {
                println!(
                    "Omen {} contract {}",
                    doc["omen_version"], doc["contract_digest"]
                );
                println!("Groups: {}", capability_groups.join(", "));
                println!("Next: omen capabilities; omen describe <capability>; omen how <recipe>");
            }
        }
        Some(Commands::Capabilities { group }) => {
            let contract = machine_contract::contract();
            let entries: Vec<_> = machine_contract::catalogue(&contract, &machine_context)
                .into_iter()
                .filter(|entry| group.as_deref().is_none_or(|g| entry.group == g))
                .collect();
            if json_mode {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "contract_version": machine_contract::CONTRACT_VERSION,
                        "contract_digest": machine_contract::contract_digest(),
                        "capabilities": entries
                    }))?
                );
            } else {
                for entry in entries {
                    println!(
                        "{} — {} ({:?})",
                        entry.id, entry.summary, entry.availability
                    );
                }
            }
        }
        Some(Commands::Doctor) => {
            let supervisor = ProcessSupervisor::new();
            let backend_caps = supervisor.backend().capabilities();
            if json_mode {
                let doc = serde_json::json!({
                    "status": "ok",
                    "version": env!("CARGO_PKG_VERSION"),
                    "doctrine": "substrate, not sovereign",
                    "platform": std::env::consts::OS,
                    "capabilities": {
                        "filesystem": format!("{:?}", backend_caps.filesystem),
                        "network": format!("{:?}", backend_caps.network),
                        "descendants": format!("{:?}", backend_caps.descendants),
                    }
                });
                println!("{}", serde_json::to_string_pretty(&doc)?);
            } else {
                println!("Omen {}: OK", env!("CARGO_PKG_VERSION"));
                println!("Doctrine: substrate, not sovereign");
                println!("Platform Backend: {}", std::env::consts::OS);
                println!("Descendant Containment: {:?}", backend_caps.descendants);
            }
        }
        Some(Commands::Describe(args)) => {
            if let Some(id) = args.capability {
                match machine_contract::capability(&id) {
                    Some(definition) => {
                        let status = machine_context
                            .capability_statuses
                            .iter()
                            .find(|status| status.id == definition.id)
                            .cloned()
                            .expect("every static capability has a runtime overlay");
                        let entry = CapabilityProjection { definition, status };
                        if json_mode {
                            let mut doc = serde_json::to_value(&entry)?;
                            if let Some(object) = doc.as_object_mut() {
                                object.insert(
                                    "contract_version".into(),
                                    serde_json::json!(machine_contract::CONTRACT_VERSION),
                                );
                                object.insert(
                                    "contract_digest".into(),
                                    serde_json::json!(machine_contract::contract_digest()),
                                );
                                object.insert("describe_ref".into(), serde_json::json!(id));
                                object.insert(
                                    "invocation".into(),
                                    serde_json::to_value(machine_contract::invocation_for(&id))?,
                                );
                            }
                            println!("{}", serde_json::to_string_pretty(&doc)?);
                        } else {
                            println!("{}: {}", entry.definition.id, entry.definition.summary);
                            println!("Effect: {:?}", entry.definition.effect_class);
                            println!(
                                "Availability: {:?}, Admission: {:?}",
                                entry.status.availability, entry.status.admission
                            );
                            let invocation = machine_contract::invocation_for(&id);
                            if let Some(cli) = invocation.cli {
                                println!("CLI: omen {cli}");
                            }
                            if let Some(mcp_tool) = invocation.mcp_tool {
                                println!("MCP: {mcp_tool}");
                            }
                            if invocation.cli.is_none() && invocation.mcp_tool.is_none() {
                                println!("Direct invocation: not exposed on CLI or MCP");
                            }
                        }
                    }
                    None => {
                        if json_mode {
                            let mut error = OmenError::from_code(
                                ErrorCode::CapabilityNotFound,
                                format!("capability '{id}' does not exist"),
                            );
                            error.details = serde_json::json!({
                                "capability_id": id,
                                "next_actions": ["capabilities"]
                            });
                            println!("{}", serde_json::to_string_pretty(&error)?);
                        } else {
                            eprintln!("Capability not found: {id}");
                        }
                        std::process::exit(2);
                    }
                }
                return Ok(());
            }
            let supervisor = ProcessSupervisor::new();
            let backend_caps = supervisor.backend().capabilities();
            let desc = serde_json::json!({
                "name": "omen",
                "version": env!("CARGO_PKG_VERSION"),
                "doctrine": "substrate, not sovereign",
                "boundaries": {
                    "lantern": "memory and provenance",
                    "resolve": "live guards and locks",
                    "tethers": "permissions, policy, approval, durable intent, and outcome truth",
                    "omen": "physical execution, containment, facts, and CAS artifacts",
                    "threadmoth": "deterministic bounded structural mutation",
                },
                "containment": {
                    "filesystem": format!("{:?}", backend_caps.filesystem),
                    "network": format!("{:?}", backend_caps.network),
                    "descendants": format!("{:?}", backend_caps.descendants),
                }
            });
            if json_mode {
                println!("{}", serde_json::to_string_pretty(&desc)?);
            } else {
                println!(
                    "Omen {}: Substrate, not sovereign",
                    env!("CARGO_PKG_VERSION")
                );
                println!("Enforcement: Descendants: {:?}", backend_caps.descendants);
            }
        }
        Some(Commands::How { recipe }) => match machine_contract::recipe(&recipe) {
            Some(doc) => {
                if json_mode {
                    println!("{}", serde_json::to_string_pretty(&doc)?);
                } else {
                    println!("{recipe}");
                    for (i, step) in doc.steps.iter().enumerate() {
                        println!("{}. {}", i + 1, step.capability_id);
                        println!("   {}", step.purpose);
                    }
                }
            }
            None => {
                if json_mode {
                    let mut error = OmenError::from_code(
                        ErrorCode::RecipeNotFound,
                        format!("recipe '{recipe}' does not exist"),
                    );
                    error.details = serde_json::json!({
                        "recipe_id": recipe,
                        "next_actions": ["orient"]
                    });
                    println!("{}", serde_json::to_string_pretty(&error)?);
                } else {
                    eprintln!("Recipe not found: {recipe}");
                }
                std::process::exit(2);
            }
        },
        Some(Commands::Context { since }) => {
            let doc = match since {
                None => {
                    serde_json::json!({"context_generation":machine_context.context_generation,"generation_status":machine_context.generation_status,"delta":"CURRENT_SNAPSHOT","workspace":{"root":"."},"provider_status":"not_probed"})
                }
                Some(generation)
                    if Some(generation as i64) == machine_context.context_generation =>
                {
                    serde_json::json!({"changed":false,"from_generation":generation,"context_generation":generation,"changes":[]})
                }
                Some(generation) => {
                    serde_json::json!({"error":"DELTA_UNAVAILABLE","from_generation":generation,"state_changed":false,"retryable":true,"context_generation":machine_context.context_generation,"reason":"Omen does not retain that historical generation","next_actions":["context"]})
                }
            };
            println!("{}", serde_json::to_string_pretty(&doc)?);
        }
        Some(Commands::History(args)) => {
            let session_id = match args.session_id {
                Some(value) => match omen_core::InteractiveSessionId::new(value) {
                    Ok(id) => Some(id),
                    Err(error) => {
                        let core = omen_core::CoreError::InvalidId(error.to_string());
                        if json_mode {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&OmenError::from_core(&core))?
                            );
                        } else {
                            eprintln!("History query failed: {core}");
                        }
                        std::process::exit(2);
                    }
                },
                None => None,
            };
            let all_sessions = !args.current_session && session_id.is_none();
            let query = HistoryQuery {
                all_sessions,
                session_id,
                limit: args.limit,
            };
            let result = match Database::open_read_only(&db_path)
                .map_err(|error| omen_core::CoreError::ExecutionFailedCode {
                    code: ErrorCode::PersistenceFailure,
                    message: format!("failed to open canonical history database: {error}"),
                })
                .and_then(|db| query_history(&db, &query))
            {
                Ok(result) => result,
                Err(error) => {
                    if json_mode {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&OmenError::from_core(&error))?
                        );
                    } else {
                        eprintln!("History query failed: {error}");
                    }
                    std::process::exit(2);
                }
            };
            let local_execution = if result.entries.is_empty() {
                std::fs::read_to_string(omen_knowledge::local_execution_status_path(&ws_root))
                    .ok()
                    .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
            } else {
                None
            };
            if json_mode {
                if let Some(marker) = local_execution {
                    let doc = serde_json::json!({
                        "history": result,
                        "history_status": "UNJOURNALED_LOCAL_EXECUTION",
                        "local_execution": marker
                    });
                    println!("{}", serde_json::to_string_pretty(&doc)?);
                } else {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
            } else if result.entries.is_empty() {
                println!("No durable execution history recorded.");
                if let Some(marker) = local_execution {
                    println!("Local execution status: UNJOURNALED_LOCAL_EXECUTION");
                    if let Some(execution_id) = marker.get("execution_id").and_then(|v| v.as_str()) {
                        println!("Last local execution: {execution_id}");
                    }
                    if let Some(command) = marker.get("command") {
                        println!("Command: {command}");
                    }
                }
            } else {
                println!(
                    "TIME                         STATUS      ID                 ACTION / COMMAND"
                );
                for entry in result.entries {
                    println!(
                        "{:<28} {:<11} {:<18} {}",
                        entry.recorded_at,
                        serde_json::to_string(&entry.status)?.trim_matches('"'),
                        entry.execution_id,
                        entry.command
                    );
                }
            }
        }
        Some(Commands::Action(action_args)) => {
            let loaded = match load_omen_config(&ws_root) {
                Ok(config) => config,
                Err(error) => {
                    if json_mode {
                        print_composition_error(&error, error.code);
                    } else {
                        eprintln!("{error}");
                    }
                    std::process::exit(2);
                }
            };
            match action_args.subcommand {
                ActionSubcommands::List => {
                    let doc = match loaded {
                        None => {
                            serde_json::json!({"config_present":false,"schema_version":null,"project":null,"actions":[]})
                        }
                        Some(config) => serde_json::json!({
                            "config_present":true,
                            "schema_version":config.schema_version,
                            "project":config.project.as_ref().and_then(|project| project.name.clone()),
                            "actions":config.actions.iter().map(|(id, action)| serde_json::json!({"action_id":id,"description":action.description,"step_count":action.steps.len()})).collect::<Vec<_>>()
                        }),
                    };
                    if json_mode {
                        println!("{}", serde_json::to_string_pretty(&doc)?);
                    } else {
                        if !doc["config_present"].as_bool().unwrap_or(false) {
                            println!("No Omen.toml present.");
                        } else if let Some(actions) = doc["actions"].as_array() {
                            for action in actions {
                                println!(
                                    "{} ({} steps)",
                                    action["action_id"].as_str().unwrap_or(""),
                                    action["step_count"]
                                );
                            }
                        }
                    }
                }
                ActionSubcommands::Show { action_id } => {
                    let config = match loaded {
                        Some(config) => config,
                        None => {
                            let error = "Omen.toml is not present";
                            if json_mode {
                                print_composition_error(error, "ACTION_NOT_FOUND");
                            } else {
                                eprintln!("{error}");
                            }
                            std::process::exit(2);
                        }
                    };
                    let action = match config.actions.get(&action_id) {
                        Some(action) => action,
                        None => {
                            let error = format!("action '{action_id}' does not exist");
                            if json_mode {
                                print_composition_error(&error, "ACTION_NOT_FOUND");
                            } else {
                                eprintln!("{error}");
                            }
                            std::process::exit(2);
                        }
                    };
                    if json_mode {
                        println!("{}", serde_json::to_string_pretty(action)?);
                    } else {
                        println!(
                            "{}",
                            action.description.as_deref().unwrap_or("(no description)")
                        );
                        for step in &action.steps {
                            println!("{}: {}", step.id, step.capability);
                        }
                    }
                }
                ActionSubcommands::Plan { action_id } => {
                    let config = match loaded {
                        Some(config) => config,
                        None => {
                            print_composition_error("Omen.toml is not present", "ACTION_NOT_FOUND");
                            std::process::exit(2);
                        }
                    };
                    match composition::plan_action(
                        &config,
                        &action_id,
                        &machine_contract::contract(),
                        &machine_context,
                    ) {
                        Ok(plan) => {
                            if json_mode {
                                println!("{}", serde_json::to_string_pretty(&plan)?);
                            } else {
                                println!("Plan {} ({})", plan.action_id, plan.plan_digest);
                                for step in &plan.steps {
                                    println!(
                                        "{}. {} -> {}",
                                        step.order + 1,
                                        step.step_id,
                                        step.capability_id
                                    );
                                }
                            }
                        }
                        Err(error) => {
                            print_composition_error(&error, &error.code);
                            std::process::exit(2);
                        }
                    }
                }
                ActionSubcommands::Run {
                    action_id,
                    expect_plan,
                    execution_contract,
                } => {
                    let config = match loaded {
                        Some(config) => config,
                        None => {
                            print_composition_error("Omen.toml is not present", "ACTION_NOT_FOUND");
                            std::process::exit(2);
                        }
                    };
                    let plan = match composition::plan_action(
                        &config,
                        &action_id,
                        &machine_contract::contract(),
                        &machine_context,
                    ) {
                        Ok(plan) => plan,
                        Err(error) => {
                            print_composition_error(&error, &error.code);
                            std::process::exit(2);
                        }
                    };
                    if plan.plan_digest != expect_plan {
                        let mut error = OmenError::from_code(
                            ErrorCode::PlanChanged,
                            "the current plan differs from --expect-plan; run action plan again",
                        );
                        error.details = serde_json::json!({
                            "expected_plan_digest": expect_plan,
                            "actual_plan_digest": plan.plan_digest,
                        });
                        println!("{}", serde_json::json!({"ok":false,"error":error}));
                        std::process::exit(2);
                    }
                    let authorities = match load_execution_contracts(&execution_contract) {
                        Ok(contracts) => contracts,
                        Err(error) => {
                            print_composition_error(&error, error.code);
                            std::process::exit(2);
                        }
                    };
                    let supervisor = ProcessSupervisor::new();
                    let registry = get_workspace_semantic_registry(&ws_root);
                    let executor =
                        DefaultCapabilityExecutor::new(ws_root.clone(), registry, supervisor);
                    let mut db = match Database::open(&db_path) {
                        Ok(db) => db,
                        Err(error) => {
                            print_composition_error(&error, "EXECUTION_STATE_UNAVAILABLE");
                            std::process::exit(2);
                        }
                    };
                    let cas = ContentAddressedStore::new(cas_dir);
                    let report = match execute_plan(
                        &plan,
                        &executor,
                        &authorities,
                        &mut db,
                        &cas,
                        machine_context.context_generation,
                    )
                    .await
                    {
                        Ok(report) => report,
                        Err(error) => {
                            print_composition_error(&error.message, &error.code);
                            std::process::exit(2);
                        }
                    };
                    if json_mode {
                        println!("{}", serde_json::to_string_pretty(&report)?);
                    } else {
                        println!("Action {}", report.action_id);
                        println!("Plan {}", report.plan_digest);
                        for (index, step) in report.steps.iter().enumerate() {
                            println!(
                                "{}. {} {} {:?}",
                                index + 1,
                                step.step_id,
                                step.capability_id,
                                step.status
                            );
                        }
                        println!("{:?} in {}ms", report.status, report.duration_ms);
                        if let Some(evidence) = report.evidence_artifact {
                            println!("Evidence: {evidence}");
                        }
                    }
                    if report.status != omen_core::composition::ActionRunStatus::Completed {
                        std::process::exit(1);
                    }
                }
            }
        }
        Some(Commands::Tool(tool_args)) => match tool_args.subcommand {
            ToolSubcommands::List => {
                let profiles = vec!["threadmoth", "cargo", "git", "ripgrep"];
                if json_mode {
                    println!("{}", serde_json::to_string_pretty(&profiles)?);
                } else {
                    for p in profiles {
                        println!("- {p}");
                    }
                }
            }
            ToolSubcommands::Inspect { tool_id } => {
                let prof = bundled_tool_profile(&tool_id);
                match prof {
                    Ok(p) => {
                        if json_mode {
                            println!("{}", serde_json::to_string_pretty(&p)?);
                        } else {
                            println!("Tool: {}", p.tool_id);
                            println!("Binary: {}", p.binary_name);
                            println!("Description: {}", p.description);
                        }
                    }
                    Err(e) => {
                        eprintln!("Error inspecting tool {tool_id}: {e}");
                        std::process::exit(1);
                    }
                }
            }
            ToolSubcommands::Validate { tool_id } => {
                let prof = bundled_tool_profile(&tool_id)?;
                let binary_path =
                    omen_atlas::find_binary_on_path(&prof.binary_name).ok_or_else(|| {
                        CoreError::NotFound(format!(
                            "Binary '{}' not found on PATH",
                            prof.binary_name
                        ))
                    })?;
                let fingerprint = omen_atlas::compute_binary_fingerprint(&binary_path)?;
                let mut instance = omen_atlas::ToolInstance {
                    tool_id: omen_core::ToolId::new(&prof.tool_id)?,
                    binary_path,
                    binary_fingerprint: fingerprint,
                    version: None,
                    platform: std::env::consts::OS.into(),
                    validation_state: omen_atlas::ValidationState::Unvalidated,
                    understanding: omen_atlas::ToolUnderstanding::Adapted,
                    profile: Some(prof),
                    last_validated_at: None,
                };

                let validator = ToolValidator::new();
                validator.validate(&mut instance).await?;
                if json_mode {
                    println!("{}", serde_json::to_string_pretty(&instance)?);
                } else {
                    println!("Tool: {}", instance.tool_id);
                    println!("Status: {:?}", instance.validation_state);
                    println!("Fingerprint: {}", instance.binary_fingerprint);
                }
            }
        },
        Some(Commands::Fact(fact_args)) => match fact_args.subcommand {
            FactSubcommands::Get {
                uri,
                require_current,
            } => {
                let res_uri = ResourceUri::parse(&uri)?;
                let db = Database::open(&db_path)?;
                let fact = FactRegistry::get_fact(&db, &res_uri, require_current);
                match fact {
                    Ok(f) => {
                        if json_mode {
                            println!("{}", serde_json::to_string_pretty(&f)?);
                        } else {
                            println!("Fact: {}", f.fact_id);
                            println!("Resource: {}", f.resource_uri);
                            println!("Value: {}", f.value);
                            println!("Validity: {:?}", f.validity);
                        }
                    }
                    Err(e) => {
                        if json_mode {
                            let err_json = serde_json::json!({
                                "error": e.to_string(),
                                "code": format!("{:?}", e.code()),
                            });
                            println!("{}", serde_json::to_string_pretty(&err_json)?);
                        } else {
                            eprintln!("Refusal: {e}");
                        }
                        std::process::exit(1);
                    }
                }
            }
            FactSubcommands::Why { uri } => {
                let res_uri = ResourceUri::parse(&uri)?;
                let db = Database::open(&db_path)?;
                let prov = FactRegistry::why_fact(&db, &res_uri)?;
                if json_mode {
                    println!("{}", serde_json::to_string_pretty(&prov)?);
                } else {
                    println!("Provenance for: {}", prov.fact.resource_uri);
                    println!("Current Value: {}", prov.fact.value);
                    println!("Validity: {:?}", prov.fact.validity);
                    println!("Dependencies: {}", prov.dependencies.len());
                    for dep in &prov.dependencies {
                        println!(
                            "  - {}: recorded={}, current={}",
                            dep.generation_name, dep.recorded_generation, dep.current_generation
                        );
                    }
                    println!("History Count: {}", prov.history.len());
                }
            }
        },
        Some(Commands::Exec(exec_args)) => {
            let supervisor = ProcessSupervisor::new();
            let mut db = Database::open(&db_path)?;
            let cas = ContentAddressedStore::new(cas_dir);

            if let Some(contract_file) = exec_args.contract {
                let content = std::fs::read_to_string(contract_file)?;
                let wire: ExecutionContractWire = serde_json::from_str(&content)?;
                let contract = ExecutionContract::try_from(wire)?;

                let output = supervisor
                    .execute_contract(&contract, ws_root.clone(), exec_args.budget, false)
                    .await?;
                let artifact = cas.store(
                    &mut db,
                    &output.stdout_all,
                    "text/plain",
                    "omen://execution/contract",
                    omen_core::RetentionClass::Referenced,
                )?;

                let result = omen_core::ExecutionResult {
                    execution_id: omen_core::ExecutionId::generate(),
                    external_reference: Some(contract.execution_id.to_string()),
                    action_id: ActionId::new("act-execution-result").unwrap(),
                    runtime_status: output.runtime_status,
                    process_exit: output.process_exit.clone(),
                    adapter_classification: output.adapter_classification,
                    enforcement: output.enforcement.clone(),
                    observations: vec![],
                    fact_updates: vec![],
                    artifacts: vec![artifact.uri],
                    reduced_summary: String::from_utf8_lossy(&output.stdout_bounded).to_string(),
                };
                let res_wire = ExecutionResultWire::from(&result);

                println!("{}", serde_json::to_string_pretty(&res_wire)?);
            } else {
                if exec_args.argv.is_empty() {
                    eprintln!("Error: argv cannot be empty");
                    std::process::exit(1);
                }

                let command = exec_args.argv.clone();
                let req = ExecutionRequest {
                    argv: command.clone(),
                    cwd: ws_root.clone(),
                    env: vec![],
                    stdin_mode: StdioMode::Closed,
                    stdin_payload: None,
                    timeout_ms: exec_args.timeout_ms,
                    inline_budget: exec_args.budget,
                    required_assurance: RequiredAssurance::default(),
                    secrets: vec![],
                };

                let execution_id = omen_core::ExecutionId::generate();
                let output = supervisor.execute(req).await?;
                let stdout_artifact = cas.store(
                    &mut db,
                    &output.stdout_all,
                    "text/plain",
                    "omen://execution/direct/stdout",
                    omen_core::RetentionClass::Referenced,
                )?;
                let stderr_artifact = cas.store(
                    &mut db,
                    &output.stderr_all,
                    "text/plain",
                    "omen://execution/direct/stderr",
                    omen_core::RetentionClass::Referenced,
                )?;

                std::fs::write(
                    omen_knowledge::local_execution_status_path(&ws_root),
                    serde_json::to_vec_pretty(&serde_json::json!({
                        "history_status": "UNJOURNALED_LOCAL_EXECUTION",
                        "execution_id": execution_id.to_string(),
                        "command": command,
                        "exit_code": output.process_exit.code,
                        "runtime_status": format!("{:?}", output.runtime_status),
                        "stdout_artifact_uri": stdout_artifact.uri.to_string(),
                        "stderr_artifact_uri": stderr_artifact.uri.to_string(),
                        "recorded_at_unix_seconds": std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|duration| duration.as_secs())
                            .unwrap_or_default()
                    }))?,
                )?;

                if json_mode {
                    let out_json = serde_json::json!({
                        "execution_id": execution_id.to_string(),
                        "runtime_status": format!("{:?}", output.runtime_status),
                        "exit_code": output.process_exit.code,
                        "artifact_uri": stdout_artifact.uri.to_string(),
                        "stdout_artifact_uri": stdout_artifact.uri.to_string(),
                        "stderr_artifact_uri": stderr_artifact.uri.to_string(),
                        "stdout_bounded": String::from_utf8_lossy(&output.stdout_bounded),
                        "stderr_bounded": String::from_utf8_lossy(&output.stderr_bounded),
                    });
                    println!("{}", serde_json::to_string_pretty(&out_json)?);
                } else {
                    use std::io::Write as _;

                    print!("{}", String::from_utf8_lossy(&output.stdout_bounded));
                    eprint!("{}", String::from_utf8_lossy(&output.stderr_bounded));
                    if !output.stderr_bounded.is_empty() && !output.stderr_bounded.ends_with(b"\n") {
                        eprintln!();
                    }
                    let exit_display = output
                        .process_exit
                        .code
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "none".to_string());
                    eprintln!(
                        "\\O/ {} | {:?} | exit {}",
                        execution_id, output.runtime_status, exit_display
                    );
                    if output.process_exit.code != Some(0)
                        || output.runtime_status != omen_core::RuntimeStatus::Completed
                        || output.stdout_all.len() > output.stdout_bounded.len()
                        || output.stderr_all.len() > output.stderr_bounded.len()
                    {
                        eprintln!("stdout evidence: {}", stdout_artifact.uri);
                        eprintln!("stderr evidence: {}", stderr_artifact.uri);
                    }
                    let _ = std::io::stdout().flush();
                    let _ = std::io::stderr().flush();

                    if let Some(code) = output.process_exit.code {
                        if code != 0 {
                            std::process::exit(code);
                        }
                    } else if output.runtime_status == omen_core::RuntimeStatus::TimedOut {
                        std::process::exit(124);
                    } else if output.runtime_status != omen_core::RuntimeStatus::Completed {
                        std::process::exit(1);
                    }
                }
            }
        }
        Some(Commands::Artifact(art_args)) => match art_args.subcommand {
            ArtifactSubcommands::Read {
                hash,
                offset,
                length,
            } => {
                let mut db = Database::open(&db_path)?;
                let cas = ContentAddressedStore::new(cas_dir);
                let slice = cas.read_slice(&mut db, &hash, offset, length as u64)?;
                if json_mode {
                    let out_json = serde_json::json!({
                        "hash": hash,
                        "offset": offset,
                        "length": slice.len(),
                        "content": String::from_utf8_lossy(&slice),
                    });
                    println!("{}", serde_json::to_string_pretty(&out_json)?);
                } else {
                    print!("{}", String::from_utf8_lossy(&slice));
                }
            }
            ArtifactSubcommands::Inspect { hash } => {
                let db = Database::open(&db_path)?;
                let cas = ContentAddressedStore::new(cas_dir);
                let meta = cas.inspect(&db, &hash)?;
                if json_mode {
                    println!("{}", serde_json::to_string_pretty(&meta)?);
                } else {
                    println!("Artifact: {}", meta.digest);
                    println!("Size: {} bytes", meta.size);
                    println!("MIME: {}", meta.media_type);
                    println!("Created: {}", meta.created_at);
                }
            }
        },
        Some(Commands::Gc(gc_args)) => {
            let mut db = Database::open(&db_path)?;
            let cas = ContentAddressedStore::new(cas_dir);
            let report = cas.gc(&mut db, gc_args.dry_run)?;
            if json_mode {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("GC Report (dry_run={}):", report.dry_run);
                println!("  Reclaimed count: {}", report.reclaimed_count);
                println!("  Reclaimed bytes: {}", report.reclaimed_bytes);
            }
        }
        Some(Commands::Daemon(daemon_args)) => match daemon_args.subcommand {
            DaemonSubcommands::Start { foreground } => {
                if foreground {
                    let server = omen_daemon::DaemonServer::new(None);
                    if !json_mode {
                        println!("Starting omend in foreground on {}...", server.endpoint());
                    }
                    tokio::select! {
                        res = server.run() => {
                            if let Err(e) = res {
                                eprintln!("omend error: {e}");
                                std::process::exit(1);
                            }
                        }
                        _ = tokio::signal::ctrl_c() => {
                            server.shutdown();
                            if !json_mode {
                                println!("omend shut down cleanly.");
                            }
                        }
                    }
                } else {
                    if let Ok(client) = omen_client::OmenClient::connect_default(None).await
                        && let Ok(ts) = client.ping().await
                    {
                        if json_mode {
                            let doc = serde_json::json!({
                                "status": "already_running",
                                "endpoint": client.endpoint(),
                                "timestamp_ms": ts
                            });
                            println!("{}", serde_json::to_string_pretty(&doc)?);
                        } else {
                            println!("Daemon is already running on {}", client.endpoint());
                        }
                        return Ok(());
                    }

                    let exe = std::env::current_exe()?;
                    let omend_bin = exe
                        .parent()
                        .map(|d| d.join(if cfg!(windows) { "omend.exe" } else { "omend" }))
                        .filter(|p| p.exists())
                        .unwrap_or_else(|| {
                            PathBuf::from(if cfg!(windows) { "omend.exe" } else { "omend" })
                        });

                    let mut cmd = std::process::Command::new(&omend_bin);
                    cmd.arg("--foreground")
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null());

                    #[cfg(windows)]
                    {
                        use std::os::windows::process::CommandExt;
                        const CREATE_NO_WINDOW: u32 = 0x08000000;
                        const DETACHED_PROCESS: u32 = 0x00000008;
                        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
                    }

                    let _child = cmd.spawn()?;

                    let start_poll = std::time::Instant::now();
                    let mut running = false;
                    while start_poll.elapsed().as_millis() < 3000 {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        if let Ok(client) = omen_client::OmenClient::connect_default(None).await
                            && client.ping().await.is_ok()
                        {
                            running = true;
                            break;
                        }
                    }

                    if running {
                        if json_mode {
                            let doc = serde_json::json!({
                                "status": "started",
                                "endpoint": omen_ipc::default_endpoint_address()
                            });
                            println!("{}", serde_json::to_string_pretty(&doc)?);
                        } else {
                            println!(
                                "Daemon started successfully on {}",
                                omen_ipc::default_endpoint_address()
                            );
                        }
                    } else {
                        if json_mode {
                            let doc = serde_json::json!({
                                "status": "error",
                                "message": "Timed out waiting for daemon to start"
                            });
                            println!("{}", serde_json::to_string_pretty(&doc)?);
                        } else {
                            eprintln!("Error: Timed out waiting for daemon to start");
                        }
                        std::process::exit(1);
                    }
                }
            }
            DaemonSubcommands::Stop => match omen_client::OmenClient::connect_default(None).await {
                Ok(client) => match client.shutdown_daemon().await {
                    Ok(_) => {
                        if json_mode {
                            let doc = serde_json::json!({ "status": "stopped" });
                            println!("{}", serde_json::to_string_pretty(&doc)?);
                        } else {
                            println!("Daemon stopped successfully.");
                        }
                    }
                    Err(e) => {
                        if json_mode {
                            let doc = serde_json::json!({
                                "status": "error",
                                "message": e.to_string()
                            });
                            println!("{}", serde_json::to_string_pretty(&doc)?);
                        } else {
                            eprintln!("Error stopping daemon: {e}");
                        }
                        std::process::exit(1);
                    }
                },
                Err(_) => {
                    if json_mode {
                        let doc = serde_json::json!({ "status": "not_running" });
                        println!("{}", serde_json::to_string_pretty(&doc)?);
                    } else {
                        println!("Daemon is not running.");
                    }
                }
            },
            DaemonSubcommands::Status => {
                let endpoint = omen_ipc::default_endpoint_address();
                match omen_client::OmenClient::connect_default(None).await {
                    Ok(client) => match client.ping().await {
                        Ok(ts) => {
                            if json_mode {
                                let doc = serde_json::json!({
                                    "status": "running",
                                    "endpoint": client.endpoint(),
                                    "timestamp_ms": ts
                                });
                                println!("{}", serde_json::to_string_pretty(&doc)?);
                            } else {
                                println!("Daemon Status: running");
                                println!("Endpoint: {}", client.endpoint());
                                println!("Timestamp: {ts}ms");
                            }
                        }
                        Err(e) => {
                            if json_mode {
                                let doc = serde_json::json!({
                                    "status": "unresponsive",
                                    "endpoint": endpoint,
                                    "error": e.to_string()
                                });
                                println!("{}", serde_json::to_string_pretty(&doc)?);
                            } else {
                                println!("Daemon Status: unresponsive ({e})");
                            }
                        }
                    },
                    Err(_) => {
                        if json_mode {
                            let doc = serde_json::json!({
                                "status": "offline",
                                "endpoint": endpoint
                            });
                            println!("{}", serde_json::to_string_pretty(&doc)?);
                        } else {
                            println!("Daemon Status: offline");
                            println!("Endpoint: {endpoint}");
                        }
                    }
                }
            }
            DaemonSubcommands::Ping => match omen_client::OmenClient::connect_default(None).await {
                Ok(client) => match client.ping().await {
                    Ok(ts) => {
                        if json_mode {
                            let doc = serde_json::json!({
                                "status": "pong",
                                "timestamp_ms": ts
                            });
                            println!("{}", serde_json::to_string_pretty(&doc)?);
                        } else {
                            println!("pong ({ts}ms)");
                        }
                    }
                    Err(e) => {
                        if json_mode {
                            let doc = serde_json::json!({
                                "status": "offline",
                                "error": e.to_string()
                            });
                            println!("{}", serde_json::to_string_pretty(&doc)?);
                        } else {
                            eprintln!("Daemon is offline: {e}");
                        }
                        std::process::exit(1);
                    }
                },
                Err(e) => {
                    if json_mode {
                        let doc = serde_json::json!({
                            "status": "offline",
                            "error": e.to_string()
                        });
                        println!("{}", serde_json::to_string_pretty(&doc)?);
                    } else {
                        eprintln!("Daemon is offline: {e}");
                    }
                    std::process::exit(1);
                }
            },
        },
        Some(Commands::Mcp(mcp_args)) => {
            let ws = mcp_args.workspace.unwrap_or(ws_root);
            let ws = omen_mcp::validate_workspace(&ws)
                .map_err(|message| std::io::Error::new(std::io::ErrorKind::NotFound, message))?;
            let client = omen_client::OmenClient::connect_default(None).await.ok();
            if let Some(ref c) = client {
                let path_str = ws.clone();
                let _ = c
                    .attach_workspace(path_str.to_string_lossy().to_string())
                    .await;
            }

            let server = omen_mcp::McpServer::new(ws, client);
            server.run_stdio().await?;
        }
        None => {
            if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                let db = Database::open(&db_path).ok();
                let mut session = omen_interactive::InteractiveSession::new(ws_root, db)?;
                session.run_loop()?;
            } else {
                println!("Omen: substrate, not sovereign. Run 'omen --help' for usage.");
            }
        }
    }

    Ok(())
}
