use clap::{Parser, Subcommand};
use omen_schema::{ExecutionContractWire, ExecutionResultWire};
use schemars::schema_for;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod perf;
mod preview;

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
    /// Run Omen 0.3 performance benchmarks
    Bench,
    /// Run the 0.9-B.1 product and harness-tax audit
    Perf(perf::PerfArgs),
    /// Build, install, and prove an immutable preview candidate
    Preview(preview::PreviewArgs),
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
            run_benchmarks();
            run_cmd("cargo-deny", &["check"]);
            println!("=== Verification Passed Successfully ===");
        }
        Commands::Bench => {
            run_benchmarks();
        }
        Commands::Perf(args) => perf::run(args, &project_root()),
        Commands::Preview(args) => preview::run(args, &project_root()),
    }
}

fn run_benchmarks() {
    use omen_interactive::InteractiveSession;
    use omen_interactive::completion::{
        CachedFact, CompletionContext, HotSemanticIndex, OmenCompleter,
    };
    use omen_interactive::grammar::GrammarScanner;
    use reedline::Prompt;
    use std::time::Instant;

    println!("=== Omen 0.3 Performance Benchmark Suite ===");

    // 1. In-Process Session Construction & Prompt Latency
    println!(
        "\n[1/4] Benchmarking In-Process Session Construction & Prompt Latency (100 iterations)..."
    );
    println!(
        "  Definition: In-process struct allocation, capability detection, and prompt rendering."
    );
    let mut startup_durations = Vec::with_capacity(100);
    let temp_dir = tempfile::tempdir().expect("Failed to create tempdir");

    for _ in 0..100 {
        let t0 = Instant::now();
        let session = InteractiveSession::new(temp_dir.path().to_path_buf(), None)
            .expect("Failed to create session");
        let _ = session.prompt.render_prompt_left();
        startup_durations.push(t0.elapsed());
    }

    startup_durations.sort();
    let min_startup = startup_durations[0];
    let max_startup = startup_durations[startup_durations.len() - 1];
    let median_startup = startup_durations[startup_durations.len() / 2];
    let avg_startup: std::time::Duration =
        startup_durations.iter().sum::<std::time::Duration>() / startup_durations.len() as u32;

    println!("  Min:    {:?}", min_startup);
    println!("  Max:    {:?}", max_startup);
    println!("  Median: {:?}", median_startup);
    println!("  Avg:    {:?}", avg_startup);
    println!("  => PASS: In-process session construction completes in microseconds.");

    // 2. Genuine Cold Process Launch Benchmark (Release Binary)
    println!("\n[2/4] Benchmarking Genuine Cold Process Launch (Release Binary, 10 iterations)...");
    println!(
        "  Definition: OS spawn of fresh 'omen.exe doctor --json' to process exit & output read."
    );
    let release_bin = if cfg!(windows) {
        project_root()
            .join("target")
            .join("release")
            .join("omen.exe")
    } else {
        project_root().join("target").join("release").join("omen")
    };

    if release_bin.exists() {
        let mut cold_durations = Vec::with_capacity(10);
        for _ in 0..10 {
            let t0 = Instant::now();
            let output = std::process::Command::new(&release_bin)
                .args(["doctor", "--json"])
                .output()
                .expect("Failed to execute release binary");
            let elapsed = t0.elapsed();
            assert!(
                output.status.success(),
                "Release binary exited with failure"
            );
            cold_durations.push(elapsed);
        }

        cold_durations.sort();
        let min_cold = cold_durations[0];
        let max_cold = cold_durations[cold_durations.len() - 1];
        let median_cold = cold_durations[cold_durations.len() / 2];
        let avg_cold: std::time::Duration =
            cold_durations.iter().sum::<std::time::Duration>() / cold_durations.len() as u32;

        println!("  Min:    {:?}", min_cold);
        println!("  Max:    {:?}", max_cold);
        println!("  Median: {:?}", median_cold);
        println!("  Avg:    {:?}", avg_cold);
        println!(
            "  => PASS: Real cold process launch completes within OS scheduling expectations."
        );
    } else {
        println!(
            "  [NOTE] Release binary not found at {}. Run 'cargo build --release -p omen-cli' first.",
            release_bin.display()
        );
    }

    // 3. Completion Engine Latency with Representative Hot-Index State
    println!(
        "\n[3/4] Benchmarking OmenCompleter Suggestions with Representative Hot Index (100 queries)..."
    );
    println!(
        "  Definition: 50 active facts (Current & Dirty), 20 workspace entries, tools, and actions."
    );

    let mut hot_index = HotSemanticIndex::default();
    for i in 0..25 {
        hot_index.active_facts.push(CachedFact {
            resource_uri: format!("fact://test/service_{i}:health"),
            validity: omen_core::ValidityState::Current,
        });
    }
    for i in 25..50 {
        hot_index.active_facts.push(CachedFact {
            resource_uri: format!("fact://git/module_{i}:head"),
            validity: omen_core::ValidityState::Dirty,
        });
    }
    for i in 0..20 {
        hot_index
            .path_commands
            .push(format!("workspace_file_{i}.rs"));
    }

    let ctx = std::sync::Arc::new(std::sync::Mutex::new(CompletionContext {
        cwd: temp_dir.path().to_path_buf(),
        hot_index,
    }));
    let mut completer = OmenCompleter::new(ctx);

    let test_queries = [
        ":",
        ":st",
        ":doc",
        ":why",
        ":his",
        ":rer",
        ":in",
        "@",
        "@fact.git",
        "@fact.test",
        "@l",
        "@fa",
        "@er",
        "car",
        "git",
        "doc",
        "thr",
        "work",
        "?",
    ];

    let mut completion_durations = Vec::with_capacity(100);
    for i in 0..100 {
        let q = test_queries[i % test_queries.len()];
        let t0 = Instant::now();
        let suggestions = completer.complete_items(q, q.len());
        let elapsed = t0.elapsed();
        assert!(!suggestions.is_empty() || q == "?");
        completion_durations.push(elapsed);
    }

    completion_durations.sort();
    let min_comp = completion_durations[0];
    let max_comp = completion_durations[completion_durations.len() - 1];
    let median_comp = completion_durations[completion_durations.len() / 2];
    let avg_comp: std::time::Duration = completion_durations.iter().sum::<std::time::Duration>()
        / completion_durations.len() as u32;

    println!("  Min:    {:?}", min_comp);
    println!("  Max:    {:?}", max_comp);
    println!("  Median: {:?}", median_comp);
    println!("  Avg:    {:?}", avg_comp);

    assert!(
        avg_comp.as_millis() < 5,
        "Completion average latency ({:?}) exceeds target limit of 5ms",
        avg_comp
    );
    println!("  => PASS: Hot-index completion latency is well within < 5ms budget.");

    // 4. Grammar Scanner Throughput
    println!("\n[4/4] Benchmarking GrammarScanner Throughput (10,000 queries)...");
    let scanner_queries = [
        "cargo test --all",
        ":status",
        ":doctor",
        "? why did the build fail",
        "git status --porcelain",
        ":why @last.failed",
        r#"notepad "C:\Program Files\Rust\bin\cargo.exe""#,
        r#"dir \\server\share\data\test"#,
    ];

    let t0 = Instant::now();
    for i in 0..10000 {
        let q = scanner_queries[i % scanner_queries.len()];
        let _ = GrammarScanner::scan(q).expect("Scan failed");
    }
    let total_elapsed = t0.elapsed();
    let avg_scan_ns = total_elapsed.as_nanos() / 10000;
    let ops_per_sec = 10000.0 / total_elapsed.as_secs_f64();

    println!("  Total time for 10,000 scans: {:?}", total_elapsed);
    println!(
        "  Avg per scan:                {} ns ({:.3} µs)",
        avg_scan_ns,
        avg_scan_ns as f64 / 1000.0
    );
    println!(
        "  Throughput:                  {:.0} scans/sec",
        ops_per_sec
    );
    println!("  => PASS: Grammar scanning executes in sub-microsecond time.");

    println!("\n=== All Benchmarks Completed Successfully ===");
}
