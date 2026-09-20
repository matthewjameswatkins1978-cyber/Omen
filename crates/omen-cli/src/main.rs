use clap::{Args, Parser, Subcommand};
use omen_atlas::{RuntimeProfile, ToolValidator};
use omen_core::machine_contract::{self, CapabilityEntry};
use omen_core::{
    ActionId, CoreError, ExecutionContract, RequiredAssurance, ResourceUri, StdioMode,
};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use omen_knowledge::{
    ContentAddressedStore, Database, FactRegistry, canonical_workspace_db_path,
    resolve_workspace_dir,
};
use omen_schema::{
    ExecutionContractWire, ExecutionResultWire, ProcessExitWire, SCHEMA_VERSION_RESULT,
};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "omen",
    author,
    version,
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
    /// Tool atlas management and inspection
    Tool(ToolArgs),
    /// Fact query and provenance
    Fact(FactArgs),
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
    let state_dir = resolve_workspace_dir(&ws_root);
    let db_path = canonical_workspace_db_path(&ws_root);
    let cas_dir = state_dir.join("cas");

    let supervisor = ProcessSupervisor::new();

    match cli.command {
        Some(Commands::Orient(args)) => {
            let digest = machine_contract::contract_digest();
            if let Some(previous) = args.since {
                let doc = if previous == digest {
                    serde_json::json!({"changed": false, "contract_digest": digest})
                } else {
                    serde_json::json!({
                        "changed": true,
                        "previous_digest": previous,
                        "new_digest": digest,
                        "capabilities_added": [],
                        "capabilities_removed": [],
                        "schemas_changed": [],
                        "recipes_changed": machine_contract::contract().recipes
                    })
                };
                if json_mode {
                    println!("{}", serde_json::to_string_pretty(&doc)?);
                } else {
                    println!("{}", serde_json::to_string_pretty(&doc)?);
                }
                return Ok(());
            }
            let doc = serde_json::json!({
                "contract_version": machine_contract::CONTRACT_VERSION,
                "omen_version": env!("CARGO_PKG_VERSION"),
                "contract_digest": digest,
                "context_generation": 0,
                "workspace": {"name": ws_root.file_name().and_then(|s| s.to_str()).unwrap_or("workspace"), "root": "."},
                "platform": std::env::consts::OS,
                "backend": "native",
                "capability_groups": ["execution", "semantic", "structure", "mutation", "filesystem"],
                "references": ["@last", "@failed"],
                "recipes": machine_contract::contract().recipes,
                "next": ["capabilities", "describe <capability>", "how <recipe>", "context --since <generation>"]
            });
            if json_mode {
                println!("{}", serde_json::to_string_pretty(&doc)?);
            } else {
                println!(
                    "Omen {} contract {}",
                    doc["omen_version"], doc["contract_digest"]
                );
                println!("Groups: execution, semantic, structure, mutation, filesystem");
                println!("Next: omen capabilities; omen describe <capability>; omen how <recipe>");
            }
        }
        Some(Commands::Capabilities { group }) => {
            let entries: Vec<CapabilityEntry> = machine_contract::contract()
                .capabilities
                .into_iter()
                .filter(|entry| group.as_deref().is_none_or(|g| entry.definition.group == g))
                .collect();
            if json_mode {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({"capabilities": entries}))?
                );
            } else {
                for entry in entries {
                    println!("{} — {}", entry.definition.id, entry.definition.summary);
                }
            }
        }
        Some(Commands::Doctor) => {
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
                    Some(entry) => {
                        if json_mode {
                            println!("{}", serde_json::to_string_pretty(&entry)?);
                        } else {
                            println!("{}: {}", entry.definition.id, entry.definition.summary);
                            println!("Effect: {}", entry.definition.effect);
                            println!(
                                "Available: {}, Admitted: {}",
                                entry.status.available, entry.status.admitted
                            );
                        }
                    }
                    None => {
                        let err = serde_json::json!({"error":"CAPABILITY_NOT_FOUND","operation":id,"state_changed":false,"retryable":false,"next_actions":["capabilities"]});
                        if json_mode {
                            println!("{}", serde_json::to_string_pretty(&err)?);
                        } else {
                            eprintln!("Capability not found: {id}");
                        }
                        std::process::exit(2);
                    }
                }
                return Ok(());
            }
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
                    for (i, step) in doc["steps"].as_array().unwrap().iter().enumerate() {
                        println!("{}. {}", i + 1, step.as_str().unwrap_or(""));
                    }
                }
            }
            None => {
                let err = serde_json::json!({"error":"RECIPE_NOT_FOUND","operation":recipe,"state_changed":false,"retryable":false,"next_actions":["orient"]});
                if json_mode {
                    println!("{}", serde_json::to_string_pretty(&err)?);
                } else {
                    eprintln!("Recipe not found: {recipe}");
                }
                std::process::exit(2);
            }
        },
        Some(Commands::Context { since }) => {
            let doc = match since {
                None => {
                    serde_json::json!({"context_generation":0,"delta":"CURRENT_SNAPSHOT","workspace":{"root":"."},"provider_status":"not_probed"})
                }
                Some(0) => {
                    serde_json::json!({"changed":false,"from_generation":0,"context_generation":0,"changes":[]})
                }
                Some(generation) => {
                    serde_json::json!({"error":"DELTA_UNAVAILABLE","from_generation":generation,"state_changed":false,"retryable":true,"context_generation":0,"reason":"Omen does not retain that historical generation","next_actions":["context"]})
                }
            };
            if json_mode {
                println!("{}", serde_json::to_string_pretty(&doc)?);
            } else {
                println!("{}", serde_json::to_string_pretty(&doc)?);
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
                let profile_path = Path::new("profiles").join(format!("{tool_id}.toml"));
                let prof = RuntimeProfile::from_file(&profile_path);
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
                let profile_path = Path::new("profiles").join(format!("{tool_id}.toml"));
                let prof = RuntimeProfile::from_file(&profile_path)?;
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
            let mut db = Database::open(&db_path)?;
            let cas = ContentAddressedStore::new(cas_dir);

            if let Some(contract_file) = exec_args.contract {
                let content = std::fs::read_to_string(contract_file)?;
                let wire: ExecutionContractWire = serde_json::from_str(&content)?;
                let contract = ExecutionContract::try_from(wire)?;

                let req = ExecutionRequest {
                    argv: contract.intent.args,
                    cwd: ws_root,
                    env: vec![],
                    stdin_mode: contract.stdio.stdin,
                    stdin_payload: None,
                    timeout_ms: contract.constraints.timeout_ms,
                    inline_budget: exec_args.budget,
                    required_assurance: contract.required_assurance,
                    secrets: vec![],
                };

                let output = supervisor.execute(req).await?;
                let artifact = cas.store(
                    &mut db,
                    &output.stdout_all,
                    "text/plain",
                    "omen://execution/contract",
                    omen_core::RetentionClass::Referenced,
                )?;

                let backend_caps = supervisor.backend().capabilities();
                let res_wire = ExecutionResultWire {
                    schema_version: SCHEMA_VERSION_RESULT.to_string(),
                    execution_id: contract.execution_id.to_string(),
                    action_id: ActionId::new("act-execution-result").unwrap().to_string(),
                    runtime_status: format!("{:?}", output.runtime_status).to_uppercase(),
                    process_exit: ProcessExitWire {
                        code: output.process_exit.code,
                        signal: None,
                    },
                    adapter_classification: "EXECUTION_COMPLETE".to_string(),
                    enforcement: omen_schema::EnforcementReportWire {
                        filesystem: format!("{:?}", backend_caps.filesystem).to_uppercase(),
                        network: format!("{:?}", backend_caps.network).to_uppercase(),
                        descendant_processes: format!("{:?}", backend_caps.descendants)
                            .to_uppercase(),
                        symlink_escape: "OBSERVED".to_string(),
                    },
                    observations: vec![],
                    fact_updates: vec![],
                    artifacts: vec![artifact.uri.to_string()],
                    reduced_summary: String::from_utf8_lossy(&output.stdout_bounded).to_string(),
                };

                println!("{}", serde_json::to_string_pretty(&res_wire)?);
            } else {
                if exec_args.argv.is_empty() {
                    eprintln!("Error: argv cannot be empty");
                    std::process::exit(1);
                }

                let req = ExecutionRequest {
                    argv: exec_args.argv,
                    cwd: ws_root,
                    env: vec![],
                    stdin_mode: StdioMode::Closed,
                    stdin_payload: None,
                    timeout_ms: exec_args.timeout_ms,
                    inline_budget: exec_args.budget,
                    required_assurance: RequiredAssurance::default(),
                    secrets: vec![],
                };

                let output = supervisor.execute(req).await?;
                let artifact = cas.store(
                    &mut db,
                    &output.stdout_all,
                    "text/plain",
                    "omen://execution/direct",
                    omen_core::RetentionClass::Referenced,
                )?;

                if json_mode {
                    let out_json = serde_json::json!({
                        "runtime_status": format!("{:?}", output.runtime_status),
                        "exit_code": output.process_exit.code,
                        "artifact_uri": artifact.uri.to_string(),
                        "stdout_bounded": String::from_utf8_lossy(&output.stdout_bounded),
                        "stderr_bounded": String::from_utf8_lossy(&output.stderr_bounded),
                    });
                    println!("{}", serde_json::to_string_pretty(&out_json)?);
                } else {
                    print!("{}", String::from_utf8_lossy(&output.stdout_bounded));
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
            let client = omen_client::OmenClient::connect_default(None).await.ok();
            if let Some(ref c) = client {
                let path_str = ws.canonicalize().unwrap_or_else(|_| ws.clone());
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
