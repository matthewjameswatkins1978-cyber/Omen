//! Isolated ConPTY teardown driver for M0-W hostile-close controls (Windows).
//!
//! Parent tests spawn this helper so a wedged `ClosePseudoConsole` cannot hang
//! the cargo test process. The helper runs bounded Compat teardown scenarios
//! and prints one JSON line per scenario; the parent enforces an outer
//! watchdog and terminates the helper if it wedges.

#[cfg(windows)]
fn main() {
    use omen_compat::{WIN_CLEANUP_BOUND, WindowsConPtySession, WindowsWriteOutcome};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let gremlin = root.join("target").join("debug").join("omen-gremlin.exe");
    if !gremlin.exists() {
        println!(
            "{{\"driver\":\"conpty-teardown\",\"error\":\"gremlin missing\",\"path\":\"{}\"}}",
            gremlin.display()
        );
        std::process::exit(2);
    }

    let scenarios: &[&str] = &["normal_exit", "live_client", "final_output"];
    let mut all_ok = true;

    for scenario in scenarios {
        let t0 = Instant::now();
        let args: Vec<String> = match *scenario {
            "normal_exit" => vec!["--windows-report".into()],
            "live_client" => vec!["--windows-child-hold".into()],
            _ => vec!["--windows-final-output-hold".into()],
        };

        let spawn = WindowsConPtySession::spawn(
            &gremlin,
            &args,
            Some(&root),
            24,
            80,
            Duration::from_secs(15),
        );
        let mut ok = false;
        let mut detail;
        match spawn {
            Ok(mut session) => {
                if *scenario == "normal_exit" {
                    let _ = session.wait_for_text("OMEN_COMPAT_READY", Duration::from_secs(6));
                    let _ = session.wait_exit(Duration::from_secs(5));
                } else {
                    let _ = session.wait_for_text("OMEN_COMPAT_READY", Duration::from_secs(6));
                    if *scenario == "final_output" {
                        // Do not wait for full burst; shutdown while drain is live.
                        let _ =
                            session.wait_for_text("OMEN_COMPAT_FINAL_LINE", Duration::from_secs(2));
                    }
                }
                let budget = if *scenario == "final_output" {
                    Duration::from_secs(8)
                } else {
                    WIN_CLEANUP_BOUND.max(Duration::from_secs(4))
                };
                let obs = session.shutdown_bounded(budget);
                ok = obs.harness_shutdown_pass(budget);
                detail = format!(
                    "close_returned={} input_stopped={} output_stopped={} handles={} \
                     pipe_broken={} elapsed_ms={} bounded_out={} close_timed_out={}",
                    obs.close_returned,
                    obs.input_worker_stopped,
                    obs.output_worker_stopped,
                    obs.handles_closed_once,
                    obs.output_pipe_broken,
                    obs.elapsed.as_millis(),
                    obs.bounded_out,
                    obs.close_timed_out
                );
                // Post-shutdown write must be rejected (no further writes).
                if session.write_input(b"x").is_ok() {
                    ok = false;
                    detail.push_str("; post_shutdown_write_accepted");
                }
            }
            Err(e) => {
                all_ok = false;
                detail = format!("spawn_error={e}");
            }
        }
        if !ok {
            all_ok = false;
        }
        println!(
            "{{\"driver\":\"conpty-teardown\",\"scenario\":\"{scenario}\",\"ok\":{ok},\
             \"elapsed_ms\":{},\"detail\":\"{}\"}}",
            t0.elapsed().as_millis(),
            detail.replace('"', "'")
        );
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }

    // Small write control inside driver (Completed, no cancel).
    {
        let mut session = match WindowsConPtySession::spawn(
            &gremlin,
            &["--windows-report".to_string()],
            Some(&root),
            24,
            80,
            Duration::from_secs(10),
        ) {
            Ok(s) => s,
            Err(e) => {
                println!(
                    "{{\"driver\":\"conpty-teardown\",\"scenario\":\"small_write\",\
                     \"ok\":false,\"detail\":\"spawn_error={e}\"}}"
                );
                std::process::exit(1);
            }
        };
        let _ = session.wait_for_text("OMEN_COMPAT_READY", Duration::from_secs(5));
        let write_ok = session
            .write_input(b"\r\n")
            .map(|o| {
                o.outcome == WindowsWriteOutcome::Completed
                    && o.written_bytes == 2
                    && !o.cancel_requested
            })
            .unwrap_or(false);
        if !write_ok {
            all_ok = false;
        }
        let obs = session.shutdown_bounded(Duration::from_secs(5));
        let shutdown_ok = obs.harness_shutdown_pass(Duration::from_secs(5));
        if !shutdown_ok {
            all_ok = false;
        }
        println!(
            "{{\"driver\":\"conpty-teardown\",\"scenario\":\"small_write\",\
             \"ok\":{write_ok},\"shutdown_ok\":{shutdown_ok}}}"
        );
    }

    std::process::exit(if all_ok { 0 } else { 1 });
}

#[cfg(not(windows))]
fn main() {
    println!("{{\"driver\":\"conpty-teardown\",\"unsupported\":true}}");
}
