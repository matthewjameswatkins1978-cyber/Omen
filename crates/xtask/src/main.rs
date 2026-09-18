use clap::{Parser, Subcommand};
use omen_schema::{ExecutionContractWire, ExecutionResultWire};
use schemars::schema_for;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser)]
#[command(name = "xtask", about = "Omen repository automation")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate JSON schemas from Rust wire types
    GenerateSchemas,
    /// Verify committed JSON schemas match Rust wire types
    VerifySchemas,
    /// Run full repository verification suite
    Verify,
}

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn schemas_dir() -> PathBuf {
    project_root().join("schemas").join("v0.2")
}

fn generate_schemas_content() -> [(String, String); 2] {
    let contract_schema = schema_for!(ExecutionContractWire);
    let contract_json = serde_json::to_string_pretty(&contract_schema).unwrap();

    let result_schema = schema_for!(ExecutionResultWire);
    let result_json = serde_json::to_string_pretty(&result_schema).unwrap();

    [
        ("execution_contract.json".into(), contract_json),
        ("execution_result.json".into(), result_json),
    ]
}

fn run_cmd(cmd: &str, args: &[&str]) {
    println!("Running: {} {}", cmd, args.join(" "));
    let status = Command::new(cmd)
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("Failed to run {cmd}: {e}"));
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn main() {
    let cli = Cli::parse();
    let target_dir = schemas_dir();

    match cli.command {
        Commands::GenerateSchemas => {
            fs::create_dir_all(&target_dir).expect("Failed to create schemas dir");
            for (filename, content) in generate_schemas_content() {
                let file_path = target_dir.join(&filename);
                fs::write(&file_path, content.as_bytes()).expect("Failed to write schema");
                println!("Generated {}", file_path.display());
            }
        }
        Commands::VerifySchemas => {
            if !target_dir.exists() {
                eprintln!("Schemas directory does not exist: {}", target_dir.display());
                std::process::exit(1);
            }
            let mut diff_found = false;
            for (filename, expected) in generate_schemas_content() {
                let file_path = target_dir.join(&filename);
                if !file_path.exists() {
                    eprintln!("Missing schema file: {}", file_path.display());
                    diff_found = true;
                    continue;
                }
                let actual = fs::read_to_string(&file_path).expect("Failed to read schema");
                let actual_norm = actual.replace("\r\n", "\n");
                let expected_norm = expected.replace("\r\n", "\n");
                if actual_norm.trim() != expected_norm.trim() {
                    eprintln!("Schema drift detected in {}", file_path.display());
                    diff_found = true;
                }
            }
            if diff_found {
                eprintln!("Run 'cargo xtask generate-schemas' to update schemas.");
                std::process::exit(1);
            }
            println!("All JSON schemas are up to date.");
        }
        Commands::Verify => {
            println!("=== Omen Verification Suite ===");
            run_cmd("cargo", &["fmt", "--check"]);
            run_cmd(
                "cargo",
                &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--",
                    "-D",
                    "warnings",
                ],
            );
            run_cmd("cargo", &["test", "--workspace"]);
            run_cmd(
                "cargo",
                &["run", "--package", "xtask", "--", "verify-schemas"],
            );
            run_cmd("cargo-deny", &["check"]);
            println!("=== Verification Passed Successfully ===");
        }
    }
}
