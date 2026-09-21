use clap::Args;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

#[derive(Args, Debug)]
pub struct PerfArgs {
    /// Omen executable to measure. Defaults to the debug workspace binary.
    #[arg(long)]
    pub binary: Option<PathBuf>,
    /// Workspace supplied to Omen.
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    /// Samples per operation and state.
    #[arg(long, default_value_t = 5)]
    pub samples: usize,
    /// JSON result path. stdout is used when omitted.
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct Observation {
    duration_ms: u128,
    stdout_bytes: usize,
    stderr_bytes: usize,
    success: bool,
    process_names_before: BTreeMap<String, usize>,
    process_names_after: BTreeMap<String, usize>,
}

fn percentile(values: &[u128], numerator: usize, denominator: usize) -> u128 {
    if values.is_empty() {
        return 0;
    }
    let index = ((values.len() - 1) * numerator) / denominator;
    values[index]
}

fn metric(operation: &str, state: &str, observations: &[Observation]) -> Value {
    let mut durations: Vec<u128> = observations.iter().map(|o| o.duration_ms).collect();
    durations.sort_unstable();
    let process_deltas: Vec<Value> = observations
        .iter()
        .map(|o| {
            let names = o
                .process_names_after
                .iter()
                .filter_map(|(name, after)| {
                    let before = o.process_names_before.get(name).copied().unwrap_or(0);
                    (after > &before).then(|| json!({"name": name, "delta": after - before}))
                })
                .collect::<Vec<_>>();
            json!(names)
        })
        .collect();
    json!({
        "operation": operation,
        "state": state,
        "samples": observations.len(),
        "successes": observations.iter().filter(|o| o.success).count(),
        "p50_ms": percentile(&durations, 50, 100),
        "p95_ms": percentile(&durations, 95, 100),
        "max_ms": durations.last().copied().unwrap_or(0),
        "stdout_bytes_total": observations.iter().map(|o| o.stdout_bytes).sum::<usize>(),
        "stderr_bytes_total": observations.iter().map(|o| o.stderr_bytes).sum::<usize>(),
        "process_delta_observations": process_deltas,
    })
}

fn process_snapshot() -> BTreeMap<String, usize> {
    #[cfg(windows)]
    let output = Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output();
    #[cfg(not(windows))]
    let output = Command::new("ps").args(["-eo", "comm="]).output();
    let Ok(output) = output else {
        return BTreeMap::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut counts = BTreeMap::new();
    for line in text.lines() {
        #[cfg(windows)]
        let name = line.split(',').next().unwrap_or(line).trim_matches('"');
        #[cfg(not(windows))]
        let name = line.trim();
        if !name.is_empty() {
            *counts.entry(name.to_ascii_lowercase()).or_insert(0) += 1;
        }
    }
    counts
}

fn run_omen(binary: &Path, workspace: &Path, state_home: &Path, args: &[&str]) -> Observation {
    let before = process_snapshot();
    let started = Instant::now();
    let output = Command::new(binary)
        .args(args)
        .current_dir(workspace)
        .env("OMEN_STATE_HOME", state_home)
        .output();
    let duration_ms = started.elapsed().as_millis();
    let after = process_snapshot();
    match output {
        Ok(output) => Observation {
            duration_ms,
            stdout_bytes: output.stdout.len(),
            stderr_bytes: output.stderr.len(),
            success: output.status.success(),
            process_names_before: before,
            process_names_after: after,
        },
        Err(error) => Observation {
            duration_ms,
            stdout_bytes: 0,
            stderr_bytes: error.to_string().len(),
            success: false,
            process_names_before: before,
            process_names_after: after,
        },
    }
}

fn send_line(input: &mut ChildStdin, value: Value) -> std::io::Result<()> {
    writeln!(input, "{}", serde_json::to_string(&value).unwrap())?;
    input.flush()
}

fn read_line(output: &mut BufReader<ChildStdout>) -> std::io::Result<Value> {
    let mut line = String::new();
    output.read_line(&mut line)?;
    if line.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "MCP response EOF",
        ));
    }
    serde_json::from_str(line.trim())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn spawn_mcp(
    binary: &Path,
    workspace: &Path,
    state_home: &Path,
) -> std::io::Result<(Child, ChildStdin, BufReader<ChildStdout>)> {
    let mut child = Command::new(binary)
        .args(["mcp", "--workspace"])
        .arg(workspace)
        .current_dir(workspace)
        .env("OMEN_STATE_HOME", state_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let input = child.stdin.take().unwrap();
    let output = BufReader::new(child.stdout.take().unwrap());
    Ok((child, input, output))
}

fn semantic_observations(
    binary: &Path,
    workspace: &Path,
    state_home: &Path,
    samples: usize,
) -> (Vec<Observation>, Vec<Observation>) {
    let mut cold = Vec::with_capacity(samples);
    let mut warm = Vec::with_capacity(samples);
    for _ in 0..samples {
        let before = process_snapshot();
        let started = Instant::now();
        let result = (|| -> std::io::Result<(usize, usize)> {
            let (mut child, mut input, mut output) = spawn_mcp(binary, workspace, state_home)?;
            send_line(
                &mut input,
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"omen-b1-perf","version":"0.1"}}}),
            )?;
            let _ = read_line(&mut output)?;
            send_line(
                &mut input,
                json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
            )?;
            send_line(
                &mut input,
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"omen_symbol_search","arguments":{"query":"refresh_token","limit":20}}}),
            )?;
            let response = read_line(&mut output)?;
            let bytes = serde_json::to_vec(&response).unwrap().len();
            let _ = child.kill();
            let _ = child.wait();
            Ok((bytes, 0))
        })();
        let (stdout_bytes, stderr_bytes, success) = match result {
            Ok((stdout, stderr)) => (stdout, stderr, true),
            Err(error) => (0, error.to_string().len(), false),
        };
        cold.push(Observation {
            duration_ms: started.elapsed().as_millis(),
            stdout_bytes,
            stderr_bytes,
            success,
            process_names_before: before,
            process_names_after: process_snapshot(),
        });
    }

    let result = (|| -> std::io::Result<(Child, ChildStdin, BufReader<ChildStdout>, usize)> {
        let (child, mut input, mut output) = spawn_mcp(binary, workspace, state_home)?;
        send_line(
            &mut input,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"omen-b1-perf","version":"0.1"}}}),
        )?;
        let _ = read_line(&mut output)?;
        send_line(
            &mut input,
            json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
        )?;
        send_line(
            &mut input,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"omen_symbol_search","arguments":{"query":"refresh_token","limit":20}}}),
        )?;
        let _ = read_line(&mut output)?;
        Ok((child, input, output, 0))
    })();
    if let Ok((mut child, mut input, mut output, _)) = result {
        // The initialization and first query above deliberately warm the session. It
        // is not included in the warm-query distribution.
        let mut observations = Vec::with_capacity(samples);
        for _ in 0..samples {
            let before = process_snapshot();
            let started = Instant::now();
            let response = send_line(&mut input, json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"omen_symbol_search","arguments":{"query":"refresh_token","limit":20}}})).and_then(|_| read_line(&mut output));
            let (bytes, success) = match response {
                Ok(value) => (serde_json::to_vec(&value).unwrap().len(), true),
                Err(_) => (0, false),
            };
            observations.push(Observation {
                duration_ms: started.elapsed().as_millis(),
                stdout_bytes: bytes,
                stderr_bytes: 0,
                success,
                process_names_before: before,
                process_names_after: process_snapshot(),
            });
        }
        let _ = child.kill();
        let _ = child.wait();
        warm = observations;
    } else {
        warm.push(Observation {
            duration_ms: 0,
            stdout_bytes: 0,
            stderr_bytes: 0,
            success: false,
            process_names_before: process_snapshot(),
            process_names_after: process_snapshot(),
        });
    }
    (cold, warm)
}

fn system_memory_bytes() -> Option<u64> {
    #[cfg(windows)]
    {
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
            ])
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
    }
    #[cfg(target_os = "linux")]
    {
        let text = fs::read_to_string("/proc/meminfo").ok()?;
        let kib = text
            .lines()
            .find(|line| line.starts_with("MemTotal:"))?
            .split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()?;
        Some(kib * 1024)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

fn machine_metadata(binary: &Path, workspace: &Path, samples: usize) -> Value {
    let version = Command::new(binary)
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unavailable".into());
    let git_sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unavailable".into());
    json!({
        "machine": std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "unknown".into()),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "cpu_logical": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        "omen_version": version,
        "git_sha": git_sha,
        "workspace_fixture": workspace.display().to_string(),
        "samples_per_operation": samples,
        "harness": "omen-0.9-b1",
        "memory_bytes": system_memory_bytes(),
        "memory_note": "This is total physical RAM; per-process working-set capture remains outside the portable harness. Process deltas are recorded where the OS exposes them."
    })
}

pub fn run(args: PerfArgs, root: &Path) {
    let binary = args.binary.unwrap_or_else(|| {
        root.join("target")
            .join("debug")
            .join(if cfg!(windows) { "omen.exe" } else { "omen" })
    });
    let workspace = args.workspace.unwrap_or_else(|| root.to_path_buf());
    let samples = args.samples.max(1);
    let state = tempfile::tempdir().expect("create isolated B.1 state directory");
    let mut metrics = Vec::new();
    let commands: &[(&str, &[&str])] = &[
        ("startup_version", &["--version"]),
        ("startup_context", &["--machine", "context"]),
        ("startup_orient", &["--machine", "orient"]),
        ("discovery_capabilities", &["--machine", "capabilities"]),
        (
            "discovery_describe",
            &["--machine", "describe", "execution.run"],
        ),
        (
            "discovery_how",
            &["--machine", "how", "investigate-failure"],
        ),
        ("discovery_context", &["--machine", "context"]),
        (
            "execution_tiny",
            &["--machine", "exec", "rustc", "--", "--version"],
        ),
        (
            "history_0_to_100",
            &["--machine", "history", "--limit", "100"],
        ),
    ];
    for (operation, command) in commands {
        let observations = (0..samples)
            .map(|_| run_omen(&binary, &workspace, state.path(), command))
            .collect::<Vec<_>>();
        metrics.push(metric(operation, "cold_process", &observations));
    }
    let (semantic_cold, semantic_warm) =
        semantic_observations(&binary, &workspace, state.path(), samples);
    metrics.push(metric(
        "semantic_search",
        "cold_mcp_session",
        &semantic_cold,
    ));
    metrics.push(metric(
        "semantic_search",
        "warm_mcp_session",
        &semantic_warm,
    ));
    let result = json!({
        "schema_version": 1,
        "metadata": machine_metadata(&binary, &workspace, samples),
        "metrics": metrics,
        "coverage": {
            "measured": ["startup", "discovery", "execution", "history", "semantic_search", "cold_vs_warm", "process_snapshot"],
            "not_measured": ["large_stdout_stderr_split", "CAS_write_phase", "history_population_10_100", "semantic_definition", "semantic_references", "semantic_type_info", "workspace_scaling", "multi_session_2_5_10", "stage-level_provider_timeline", "agent_time_to_useful_answer"],
            "reason": "B.1 harness remains measurement-only and does not invent fixture or provider instrumentation absent from the packet's complete acceptance details."
        }
    });
    let text = serde_json::to_string_pretty(&result).unwrap();
    if let Some(path) = args.output {
        fs::write(&path, format!("{text}\n"))
            .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        println!("Wrote {}", path.display());
    } else {
        println!("{text}");
    }
}
