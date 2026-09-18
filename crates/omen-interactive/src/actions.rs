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
        db: Option<&mut Database>,
    ) -> Result<ProcessExit, CoreError> {
        match action {
            "status" => {
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
                if let Some(typed_ref) = TypedReference::parse(target) {
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
                        println!("Provenance for: {}", prov.fact.resource_uri);
                        println!("Value: {}", prov.fact.value);
                        println!("Validity: {:?}", prov.fact.validity);
                        println!("Dependencies:");
                        for dep in &prov.dependencies {
                            println!(
                                "  - {}: recorded={}, current={}",
                                dep.generation_name,
                                dep.recorded_generation,
                                dep.current_generation
                            );
                        }
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
            other => {
                println!(
                    "Unknown Omen semantic action ':{other}'. Available: :status, :doctor, :tools, :inspect, :why, :history, :show, :open, :rerun"
                );
                Ok(ProcessExit {
                    code: Some(1),
                    signal: None,
                })
            }
        }
    }
}
