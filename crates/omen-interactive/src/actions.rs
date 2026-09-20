use crate::grammar::TypedReference;
use omen_atlas::RuntimeProfile;
use omen_core::{CoreError, ProcessExit, ResourceUri, composition, machine_contract};
use omen_knowledge::{Database, FactRegistry};
use std::fs;
use std::path::Path;

pub struct SemanticDispatcher;

impl SemanticDispatcher {
    pub fn dispatch(
        action: &str,
        args: &[String],
        cwd: &Path,
        session_id: &omen_core::InteractiveSessionId,
        db: Option<&mut Database>,
        agent_registry: Option<&omen_agent::ProviderRegistry>,
        backend_registry: Option<&omen_engine::BackendRegistry>,
    ) -> Result<ProcessExit, CoreError> {
        match action {
            "status" => {
                if let Some(target) = args.first() {
                    let svc_name = target
                        .strip_prefix("@service.")
                        .or_else(|| target.strip_prefix("service."))
                        .unwrap_or(target);
                    if let Some(svc) = crate::services::ServiceRegistry::global().get(svc_name) {
                        println!("Service Status: {}", svc.name);
                        println!("URI:    {}", svc.resource_uri);
                        println!("State:  {:?}", svc.state);
                        println!("PID:    {:?}", svc.pid);
                        println!("Uptime: {}s", svc.uptime_secs);
                        println!("Cmd:    {}", svc.command);
                        return Ok(ProcessExit {
                            code: Some(0),
                            signal: None,
                        });
                    }
                }
                println!("Omen 0.3 Human Interface: Status Healthy");
                println!("Active Workspace: {}", cwd.display());
                if let Some(db_ref) = db
                    && let Ok(generation_val) = FactRegistry::get_generation(db_ref, "fs:workspace")
                {
                    println!("Workspace Generation: {generation_val}");
                }
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "doctor" => {
                println!("Omen Runtime Doctor: OK");
                println!("Platform Backend: {}", std::env::consts::OS);
                println!("Doctrine: substrate, not sovereign");
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "tools" => {
                println!("Known Runtime Profiles:");
                for tool in &["threadmoth", "cargo", "git", "ripgrep"] {
                    println!("  - {tool}");
                }
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "inspect" => {
                if args.is_empty() {
                    println!("Usage: :inspect <@reference | tool_id>");
                    return Ok(ProcessExit {
                        code: Some(1),
                        signal: None,
                    });
                }
                let target = &args[0];
                let roles = omen_ui::ColorRoles::plain();
                if target == "@last" {
                    if let Some(db_ref) = db
                        && let Ok(Some(exec)) =
                            omen_knowledge::ExecutionHistory::get_last_execution(db_ref, session_id)
                    {
                        println!(
                            "{}",
                            omen_ui::DiagnosticRenderer::render_execution(
                                &exec,
                                omen_ui::DiagnosticLevel::Level2,
                                &roles
                            )
                        );
                        return Ok(ProcessExit {
                            code: Some(0),
                            signal: None,
                        });
                    }
                    println!("No previous execution to inspect");
                } else if let Some(typed_ref) = TypedReference::parse(target) {
                    println!("Inspecting typed reference: {:?}", typed_ref);
                } else {
                    let prof_path = Path::new("profiles").join(format!("{target}.toml"));
                    if let Ok(prof) = RuntimeProfile::from_file(&prof_path) {
                        println!("Tool Profile: {}", prof.tool_id);
                        println!("Binary: {}", prof.binary_name);
                        println!("Description: {}", prof.description);
                    } else {
                        println!("Target '{}' not found for inspection", target);
                    }
                }
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "show" => {
                let target = args.first().map(|s| s.as_str()).unwrap_or("@last");
                let roles = omen_ui::ColorRoles::plain();
                if let Some(db_ref) = db {
                    let exec_opt = if target == "@failed" {
                        omen_knowledge::ExecutionHistory::get_last_failed_execution(
                            db_ref, session_id,
                        )
                    } else {
                        omen_knowledge::ExecutionHistory::get_last_execution(db_ref, session_id)
                    };
                    if let Ok(Some(exec)) = exec_opt {
                        println!(
                            "{}",
                            omen_ui::DiagnosticRenderer::render_execution(
                                &exec,
                                omen_ui::DiagnosticLevel::Level1,
                                &roles
                            )
                        );
                        return Ok(ProcessExit {
                            code: Some(0),
                            signal: None,
                        });
                    }
                }
                println!("No execution found for :show {target}");
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "why" => {
                if args.is_empty() {
                    println!("Usage: :why <@reference | fact_uri>");
                    return Ok(ProcessExit {
                        code: Some(1),
                        signal: None,
                    });
                }
                let target = &args[0];
                if let Some(db_ref) = db {
                    let uri_str = if target.starts_with('@') {
                        format!("fact://{}", target.trim_start_matches('@'))
                    } else {
                        target.to_string()
                    };

                    if let Ok(res_uri) = ResourceUri::parse(&uri_str)
                        && let Ok(prov) = FactRegistry::why_fact(db_ref, &res_uri)
                    {
                        let roles = omen_ui::ColorRoles::plain();
                        println!(
                            "{}",
                            omen_ui::DiagnosticRenderer::render_fact_provenance(
                                &prov,
                                omen_ui::DiagnosticLevel::Level2,
                                &roles
                            )
                        );
                        return Ok(ProcessExit {
                            code: Some(0),
                            signal: None,
                        });
                    }
                }
                println!("No provenance found for '{target}'");
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "history" => {
                if let Some(db_ref) = db
                    && let Ok(execs) = omen_knowledge::ExecutionHistory::list_session_executions(
                        db_ref, session_id, 20,
                    )
                {
                    println!("Execution History for session {}:", session_id.as_str());
                    for (i, exec) in execs.iter().enumerate() {
                        let status_str = match exec.exit_code {
                            Some(0) => "OK".to_string(),
                            Some(c) => format!("FAILED({c})"),
                            None => "RUNNING".to_string(),
                        };
                        println!(
                            "  [{}] {} (exit: {}, {} ms)",
                            i + 1,
                            exec.command,
                            status_str,
                            exec.duration_ms.unwrap_or(0)
                        );
                    }
                    return Ok(ProcessExit {
                        code: Some(0),
                        signal: None,
                    });
                }
                println!("No execution history recorded for this session");
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "rerun" => {
                let target = args.first().map(|s| s.as_str()).unwrap_or("@last");
                if let Some(db_ref) = db {
                    match crate::resolver::ReferenceResolver::resolve(target, session_id, db_ref) {
                        Ok(cmd) => {
                            println!("Reconstructing command: {cmd}");
                            Ok(ProcessExit {
                                code: Some(0),
                                signal: None,
                            })
                        }
                        Err(e) => {
                            eprintln!("Failed to rerun: {e}");
                            Ok(ProcessExit {
                                code: Some(1),
                                signal: None,
                            })
                        }
                    }
                } else {
                    println!("Database not available for :rerun");
                    Ok(ProcessExit {
                        code: Some(1),
                        signal: None,
                    })
                }
            }
            "services" => {
                let services = crate::services::ServiceRegistry::global().list();
                if services.is_empty() {
                    println!("No managed background services running.");
                } else {
                    println!("Managed Services (proc://):");
                    for svc in &services {
                        println!(
                            "  - {} [{:?}] pid: {:?}, uptime: {}s, uri: {}",
                            svc.name, svc.state, svc.pid, svc.uptime_secs, svc.resource_uri
                        );
                    }
                }
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "stop" => {
                if let Some(target) = args.first() {
                    let svc_name = target
                        .strip_prefix("@service.")
                        .or_else(|| target.strip_prefix("service."))
                        .unwrap_or(target);
                    match crate::services::ServiceRegistry::global().stop(svc_name) {
                        Ok(true) => {
                            println!("Service '{svc_name}' stopped.");
                            Ok(ProcessExit {
                                code: Some(0),
                                signal: None,
                            })
                        }
                        Ok(false) => {
                            println!("Service '{svc_name}' not found.");
                            Ok(ProcessExit {
                                code: Some(1),
                                signal: None,
                            })
                        }
                        Err(e) => {
                            eprintln!("Failed to stop service: {e}");
                            Ok(ProcessExit {
                                code: Some(1),
                                signal: None,
                            })
                        }
                    }
                } else {
                    println!("Usage: :stop <@service.<name> | name>");
                    Ok(ProcessExit {
                        code: Some(1),
                        signal: None,
                    })
                }
            }
            "agent" => {
                let sub = args.first().map(|s| s.as_str()).unwrap_or("status");
                match sub {
                    "providers" => {
                        println!("Available Agent Reasoning Providers:");
                        if let Some(reg) = agent_registry {
                            let active_id = reg.active_descriptor().id;
                            for p in reg.list_providers() {
                                let star = if p.id == active_id { "*" } else { " " };
                                let avail = if p.is_available {
                                    "available"
                                } else {
                                    "unavailable"
                                };
                                println!(
                                    "  {} {}: {} [{}] ({})",
                                    star,
                                    p.id,
                                    p.name,
                                    p.model.as_deref().unwrap_or("default"),
                                    avail
                                );
                            }
                        } else {
                            println!(
                                "  * diagnostic: Omen Built-in Diagnostic Agent [deterministic] (available)"
                            );
                        }
                        Ok(ProcessExit {
                            code: Some(0),
                            signal: None,
                        })
                    }
                    "use" => {
                        if let Some(target) = args.get(1) {
                            if let Some(reg) = agent_registry
                                && let Err(e) = reg.set_active_provider(target)
                            {
                                println!("Error switching agent provider: {e}");
                                return Ok(ProcessExit {
                                    code: Some(1),
                                    signal: None,
                                });
                            }
                            println!("Active agent provider switched to '{target}'.");
                            Ok(ProcessExit {
                                code: Some(0),
                                signal: None,
                            })
                        } else {
                            println!("Usage: :agent use <provider_id>");
                            Ok(ProcessExit {
                                code: Some(1),
                                signal: None,
                            })
                        }
                    }
                    "status" => {
                        if let Some(reg) = agent_registry {
                            println!("{}", reg.status_text());
                        } else {
                            println!("Provider: diagnostic (Omen Built-in Diagnostic Agent)");
                            println!("Model: deterministic");
                            println!("Auth source: none");
                            println!(
                                "Capabilities: orientation, failure-diagnosis, navigation, build-check, deterministic"
                            );
                            println!("Status: available");
                        }
                        Ok(ProcessExit {
                            code: Some(0),
                            signal: None,
                        })
                    }
                    other => {
                        println!(
                            "Unknown agent subcommand '{other}'. Available: providers, use, status"
                        );
                        Ok(ProcessExit {
                            code: Some(1),
                            signal: None,
                        })
                    }
                }
            }
            "backend" => {
                let sub = args.first().map(|s| s.as_str()).unwrap_or("status");
                match sub {
                    "list" => {
                        println!("Available Physical Execution Backends:");
                        if let Some(reg) = backend_registry {
                            for b in reg.list() {
                                println!(
                                    "  {:10} {:28} status={:?} fs={:?} net={:?} desc={:?}",
                                    b.id.as_str(),
                                    b.name,
                                    b.availability,
                                    b.capabilities.filesystem,
                                    b.capabilities.network,
                                    b.capabilities.descendants
                                );
                            }
                        } else {
                            println!("  native     Native Host                  Available");
                        }
                        Ok(ProcessExit {
                            code: Some(0),
                            signal: None,
                        })
                    }
                    "status" => {
                        if let Some(reg) = backend_registry {
                            let active = reg.active();
                            let desc = active.descriptor();
                            println!(
                                "Active Execution Backend: {} ({})",
                                desc.name,
                                desc.id.as_str()
                            );
                            println!("Kind:         {:?}", desc.kind);
                            println!("Availability: {:?}", desc.availability);
                            println!("Capabilities:");
                            println!("  Filesystem:   {:?}", desc.capabilities.filesystem);
                            println!("  Network:      {:?}", desc.capabilities.network);
                            println!("  Descendants:  {:?}", desc.capabilities.descendants);
                            println!("  Symlinks:     {:?}", desc.capabilities.symlink_escape);
                            println!("  PTY:          {}", desc.capabilities.pty);
                        } else {
                            println!("Active Execution Backend: Native Host (native)");
                        }
                        Ok(ProcessExit {
                            code: Some(0),
                            signal: None,
                        })
                    }
                    "use" => {
                        if let Some(target) = args.get(1) {
                            if let Some(reg) = backend_registry {
                                let bid = omen_core::BackendId::new(target).map_err(|e| {
                                    CoreError::ExecutionFailed(format!("Invalid backend ID: {e}"))
                                })?;
                                if let Err(e) = reg.set_active(&bid) {
                                    println!("Error switching backend: {e}");
                                    return Ok(ProcessExit {
                                        code: Some(1),
                                        signal: None,
                                    });
                                }
                                println!("Active execution backend switched to '{target}'.");
                            }
                            Ok(ProcessExit {
                                code: Some(0),
                                signal: None,
                            })
                        } else {
                            println!("Usage: :backend use <backend_id>");
                            Ok(ProcessExit {
                                code: Some(1),
                                signal: None,
                            })
                        }
                    }
                    other => {
                        println!(
                            "Unknown backend subcommand '{other}'. Available: list, status, use"
                        );
                        Ok(ProcessExit {
                            code: Some(1),
                            signal: None,
                        })
                    }
                }
            }
            "orient" => {
                let contract = machine_contract::contract();
                let groups: Vec<String> = contract
                    .capability_definitions
                    .iter()
                    .map(|definition| definition.group.clone())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                println!(
                    "Omen {} contract {}",
                    machine_contract::CONTRACT_VERSION,
                    machine_contract::contract_digest()
                );
                println!(
                    "Context: {:?}",
                    machine_context(db.as_deref()).context_generation
                );
                println!("Groups: {}", groups.join(", "));
                println!("Next: :capabilities; :describe <capability>; :how <recipe>; :actions");
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "capabilities" => {
                let group = args.first().map(String::as_str);
                let contract = machine_contract::contract();
                let context = machine_context(db.as_deref());
                for entry in machine_contract::project(&contract, &context)
                    .into_iter()
                    .filter(|entry| group.is_none_or(|wanted| entry.definition.group == wanted))
                {
                    println!(
                        "{} — {} [{:?}]",
                        entry.definition.id, entry.definition.summary, entry.status.availability
                    );
                }
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "describe" => {
                let Some(id) = args.first() else {
                    println!("Usage: :describe <capability>");
                    return Ok(ProcessExit {
                        code: Some(1),
                        signal: None,
                    });
                };
                match machine_contract::capability(id) {
                    Some(definition) => {
                        let context = machine_context(db.as_deref());
                        let status = context
                            .capability_statuses
                            .into_iter()
                            .find(|status| status.id == *id)
                            .unwrap();
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&machine_contract::CapabilityProjection {
                                definition,
                                status
                            })
                            .unwrap()
                        );
                        Ok(ProcessExit {
                            code: Some(0),
                            signal: None,
                        })
                    }
                    None => {
                        eprintln!("Capability not found: {id}");
                        Ok(ProcessExit {
                            code: Some(1),
                            signal: None,
                        })
                    }
                }
            }
            "how" => {
                let Some(id) = args.first() else {
                    println!("Usage: :how <recipe>");
                    return Ok(ProcessExit {
                        code: Some(1),
                        signal: None,
                    });
                };
                match machine_contract::recipe(id) {
                    Some(recipe) => {
                        println!("{} — {}", recipe.id, recipe.summary);
                        for (index, step) in recipe.steps.iter().enumerate() {
                            println!("{}. {} — {}", index + 1, step.capability_id, step.purpose);
                        }
                        println!("Advisory: {}", recipe.advisory);
                        Ok(ProcessExit {
                            code: Some(0),
                            signal: None,
                        })
                    }
                    None => {
                        eprintln!("Recipe not found: {id}");
                        Ok(ProcessExit {
                            code: Some(1),
                            signal: None,
                        })
                    }
                }
            }
            "actions" => match load_interactive_config(cwd) {
                Ok(Some(config)) => {
                    for (id, action) in config.actions {
                        println!("{} ({} steps)", id, action.steps.len());
                    }
                    Ok(ProcessExit {
                        code: Some(0),
                        signal: None,
                    })
                }
                Ok(None) => {
                    println!("No Omen.toml present.");
                    Ok(ProcessExit {
                        code: Some(0),
                        signal: None,
                    })
                }
                Err(error) => {
                    eprintln!("Failed to load Omen.toml: {error}");
                    Ok(ProcessExit {
                        code: Some(1),
                        signal: None,
                    })
                }
            },
            "plan" => {
                let Some(id) = args.first() else {
                    println!("Usage: :plan <action>");
                    return Ok(ProcessExit {
                        code: Some(1),
                        signal: None,
                    });
                };
                match load_interactive_config(cwd) {
                    Ok(Some(config)) => match composition::plan_action(
                        &config,
                        id,
                        &machine_contract::contract(),
                        &machine_context(db.as_deref()),
                    ) {
                        Ok(plan) => {
                            println!("{}", serde_json::to_string_pretty(&plan).unwrap());
                            Ok(ProcessExit {
                                code: Some(0),
                                signal: None,
                            })
                        }
                        Err(error) => {
                            eprintln!("{error}");
                            Ok(ProcessExit {
                                code: Some(1),
                                signal: None,
                            })
                        }
                    },
                    Ok(None) => {
                        eprintln!("Omen.toml is not present");
                        Ok(ProcessExit {
                            code: Some(1),
                            signal: None,
                        })
                    }
                    Err(error) => {
                        eprintln!("Failed to load Omen.toml: {error}");
                        Ok(ProcessExit {
                            code: Some(1),
                            signal: None,
                        })
                    }
                }
            }
            "symbol" => {
                if args.is_empty() {
                    println!("Usage: :symbol <query>");
                    return Ok(ProcessExit {
                        code: Some(1),
                        signal: None,
                    });
                }
                let query = &args[0];
                let reg = crate::semantic_service::build_semantic_registry(cwd);
                let results =
                    run_future_blocking(async { reg.symbol_search(query, Some(25), None).await })?;
                if results.is_empty() {
                    println!("No symbols found matching '{query}'.");
                } else {
                    println!("Found {} symbol(s) matching '{query}':", results.len());
                    for s in results {
                        println!(
                            "  @{}  ({})  {}:{}:{}",
                            s.uri.as_str(),
                            s.kind.as_str(),
                            s.location.file,
                            s.location.range.start_line + 1,
                            s.location.range.start_col + 1
                        );
                    }
                }
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "def" => {
                if args.is_empty() {
                    println!("Usage: :def <symbol_name>");
                    return Ok(ProcessExit {
                        code: Some(1),
                        signal: None,
                    });
                }
                let sym = &args[0];
                let reg = crate::semantic_service::build_semantic_registry(cwd);
                let res = run_future_blocking(async {
                    reg.find_definition(sym, None, None, None, None).await
                })?;
                match res {
                    omen_semantic::SemanticLookupResult::Resolved(loc) => {
                        println!("Definition for @symbol://{sym}:");
                        println!(
                            "  Location: {}:{}:{}",
                            loc.file,
                            loc.range.start_line + 1,
                            loc.range.start_col + 1
                        );
                    }
                    omen_semantic::SemanticLookupResult::Stale(loc) => {
                        println!("Definition for @symbol://{sym} (STALE index):");
                        println!(
                            "  Location: {}:{}:{}",
                            loc.file,
                            loc.range.start_line + 1,
                            loc.range.start_col + 1
                        );
                    }
                    omen_semantic::SemanticLookupResult::Ambiguous(candidates) => {
                        println!(
                            "Ambiguous symbol '{sym}' (found {} candidates):",
                            candidates.len()
                        );
                        for (i, c) in candidates.iter().enumerate() {
                            println!(
                                "  [{}] @{} at {}:{}:{}",
                                i + 1,
                                c.uri.as_str(),
                                c.location.file,
                                c.location.range.start_line + 1,
                                c.location.range.start_col + 1
                            );
                        }
                    }
                    omen_semantic::SemanticLookupResult::NotFound => {
                        println!("Symbol '{sym}' not found.");
                    }
                    omen_semantic::SemanticLookupResult::Unsupported => {
                        println!("Symbol definition lookup is unsupported by available providers.");
                    }
                }
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "refs" => {
                if args.is_empty() {
                    println!("Usage: :refs <symbol_name>");
                    return Ok(ProcessExit {
                        code: Some(1),
                        signal: None,
                    });
                }
                let sym = &args[0];
                let reg = crate::semantic_service::build_semantic_registry(cwd);
                let res = run_future_blocking(async {
                    reg.find_references(sym, None, None, None, Some(50), None)
                        .await
                })?;
                match res {
                    omen_semantic::SemanticLookupResult::Resolved(refs) => {
                        if refs.is_empty() {
                            println!("No references found for '{sym}'.");
                        } else {
                            println!("Found {} reference(s) for @symbol://{sym}:", refs.len());
                            for r in refs {
                                let tag = if r.is_definition { " (def)" } else { "" };
                                println!(
                                    "  {}:{}:{}{}",
                                    r.location.file,
                                    r.location.range.start_line + 1,
                                    r.location.range.start_col + 1,
                                    tag
                                );
                            }
                        }
                    }
                    omen_semantic::SemanticLookupResult::Stale(refs) => {
                        println!(
                            "Found {} reference(s) for @symbol://{sym} (STALE index):",
                            refs.len()
                        );
                        for r in refs {
                            let tag = if r.is_definition { " (def)" } else { "" };
                            println!(
                                "  {}:{}:{}{}",
                                r.location.file,
                                r.location.range.start_line + 1,
                                r.location.range.start_col + 1,
                                tag
                            );
                        }
                    }
                    omen_semantic::SemanticLookupResult::Ambiguous(candidates) => {
                        println!(
                            "Ambiguous symbol '{sym}' for references (found {} candidates):",
                            candidates.len()
                        );
                        for (i, c) in candidates.iter().enumerate() {
                            println!("  [{}] @{} at {}", i + 1, c.uri.as_str(), c.location.file);
                        }
                    }
                    omen_semantic::SemanticLookupResult::NotFound => {
                        println!("Symbol '{sym}' not found.");
                    }
                    omen_semantic::SemanticLookupResult::Unsupported => {
                        println!("Symbol references lookup is unsupported by available providers.");
                    }
                }
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "structure" => {
                if args.is_empty() {
                    println!("Usage: :structure <pattern> [language]");
                    return Ok(ProcessExit {
                        code: Some(1),
                        signal: None,
                    });
                }
                let pattern = &args[0];
                let lang = args.get(1).map(|s| s.as_str()).unwrap_or("rust");
                let reg = crate::semantic_service::build_semantic_registry(cwd);
                let matches = run_future_blocking(async {
                    reg.structural_search(pattern, lang, Some(25), None).await
                })?;
                if matches.is_empty() {
                    println!("No structural matches found for pattern '{pattern}'.");
                } else {
                    println!("Found {} structural match(es):", matches.len());
                    for m in matches {
                        println!(
                            "  {}:{}:{}: {}",
                            m.file,
                            m.range.start_line + 1,
                            m.range.start_col + 1,
                            m.matched_text.lines().next().unwrap_or("").trim()
                        );
                    }
                }
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "packages" => {
                let reg = crate::semantic_service::build_semantic_registry(cwd);
                let pkgs = run_future_blocking(async { reg.packages(None).await })?;
                if pkgs.is_empty() {
                    println!("No workspace packages detected.");
                } else {
                    println!("Workspace Packages ({}):", pkgs.len());
                    for p in pkgs {
                        println!(
                            "  @{} v{} [{}] (manifest: {})",
                            p.uri.as_str(),
                            p.version,
                            p.ecosystem,
                            p.manifest_path
                        );
                    }
                }
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            "tasks" => {
                let reg = crate::semantic_service::build_semantic_registry(cwd);
                let pkgs = run_future_blocking(async { reg.packages(None).await })?;
                let mut all_tasks = Vec::new();
                for p in pkgs {
                    for t in p.tasks {
                        all_tasks.push((p.name.clone(), t));
                    }
                }
                if all_tasks.is_empty() {
                    println!("No workspace tasks found.");
                } else {
                    println!("Workspace Tasks ({}):", all_tasks.len());
                    for (pkg, t) in all_tasks {
                        let desc = t.description.as_deref().unwrap_or("");
                        println!("  {:25} {:20} {}", t.name, pkg, desc);
                    }
                }
                Ok(ProcessExit {
                    code: Some(0),
                    signal: None,
                })
            }
            other => {
                println!(
                    "Unknown Omen semantic action ':{other}'. Available: :status, :doctor, :tools, :orient, :capabilities, :describe, :how, :actions, :plan, :inspect, :why, :history, :show, :rerun, :services, :stop, :agent, :backend, :symbol, :def, :refs, :structure, :packages, :tasks"
                );
                Ok(ProcessExit {
                    code: Some(1),
                    signal: None,
                })
            }
        }
    }
}

fn machine_context(db: Option<&Database>) -> machine_contract::MachineContext {
    let generation = db
        .and_then(|db| FactRegistry::get_generation_if_present(db, "fs:workspace").ok())
        .flatten();
    machine_contract::context_with_generation(generation)
}

fn load_interactive_config(
    cwd: &Path,
) -> Result<Option<omen_core::composition::OmenWorkspaceConfig>, String> {
    let path = cwd.join("Omen.toml");
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
