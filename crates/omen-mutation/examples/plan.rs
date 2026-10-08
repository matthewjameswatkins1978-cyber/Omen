//! Dry-run mutation planner: parse, resolve, estimate, preflight — never execute.
//!
//! Usage: `plan cp -r sub sub-copy` (run from the intended cwd).
//! Prints the effect estimate and preflight as JSON and exits 0.
//! Physical execution needs `OMEN_MUTATION_TEST_ADMIT=1` in the environment
//! (explicit test bypass, exactly like the unit tests); otherwise the plan
//! is reported and refused, proving the default is closed.

use std::io::Write;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let op = match omen_mutation::parse(&argv) {
        Ok(op) => op,
        Err(error) => {
            eprintln!("plan: {error}");
            std::process::exit(2);
        }
    };
    let resolved = omen_mutation::resolve(op, &cwd);
    let estimate = omen_mutation::estimate(&resolved);
    let (severity, summary) = omen_mutation::preflight(&estimate, &resolved.op);
    let severity = format!("{severity:?}");
    let report = serde_json::json!({
        "severity": severity,
        "summary": summary,
        "affected": estimate.affected.len(),
        "truncated": estimate.truncated,
        "overwrites": estimate.overwrites.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
        "outside_cwd": estimate.outside_cwd.len(),
        "total_bytes": estimate.total_bytes,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
    std::io::stdout().flush().ok();

    if std::env::var_os("OMEN_MUTATION_TEST_ADMIT").is_some() {
        let admission = omen_mutation::Admission::Admitted {
            bundle: "example-test-admit".to_string(),
        };
        match omen_mutation::execute(&resolved, &admission) {
            Ok(done) => {
                println!("executed under explicit test admission: {}", done.op);
            }
            Err(error) => {
                eprintln!("execute failed: {error}");
                std::process::exit(1);
            }
        }
    } else {
        eprintln!("refused-closed: set OMEN_MUTATION_TEST_ADMIT=1 to execute (test bypass)");
    }
}
