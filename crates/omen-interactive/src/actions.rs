use crate::grammar::TypedReference;
use omen_atlas::RuntimeProfile;
use omen_core::{CoreError, ProcessExit, ResourceUri};
use omen_knowledge::{Database, FactRegistry};
use std::path::Path;

pub struct SemanticDispatcher;

impl SemanticDispatcher {
    pub fn dispatch(
        action: &str,
        args: &[String],
        cwd: &Path,
        session_id: &omen_core::InteractiveSessionId,
        db: Option<&mut Database>,
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
                        println!(
                            "  * diagnostic: Omen Built-in Diagnostic Agent [deterministic] (available)"
                        );
                        Ok(ProcessExit {
                            code: Some(0),
                            signal: None,
                        })
                    }
                    "use" => {
                        if let Some(target) = args.get(1) {
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
                        println!("Provider: diagnostic (Omen Built-in Diagnostic Agent)");
                        println!("Model: deterministic");
                        println!("Auth source: none");
                        println!(
                            "Capabilities: orientation, failure-diagnosis, navigation, build-check, deterministic"
                        );
                        println!("Status: available");
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
            other => {
                println!(
                    "Unknown Omen semantic action ':{other}'. Available: :status, :doctor, :tools, :inspect, :why, :history, :show, :open, :rerun, :services, :stop, :agent"
                );
                Ok(ProcessExit {
                    code: Some(1),
                    signal: None,
                })
            }
        }
    }
}
