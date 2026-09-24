//! Isolated product-terminate driver for M0-W (Windows only).
//!
//! Exercising `NativePtyHandle::terminate()` in-process can crash the test
//! host (product handle lifecycle). This driver runs the product path in a
//! child process and reports structured JSON; parent tests judge the result.

#[cfg(windows)]
fn main() {
    use omen_engine::backend::{PtyExecutionHandle, PtyExecutionRequest};
    use omen_engine::pty::NativePtyHandle;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let gremlin = root.join("target").join("debug").join("omen-gremlin.exe");
    let mut argv = vec![gremlin.to_string_lossy().into_owned()];
    argv.extend(std::env::args().skip(1).map(|s| s.to_string()));
    let req = PtyExecutionRequest {
        session_id: omen_core::PtySessionId::generate(),
        argv,
        cwd: root,
        env: vec![("NO_COLOR".into(), "1".into())],
        rows: 24,
        cols: 80,
    };
    let mut handle = match NativePtyHandle::spawn(&req) {
        Ok(h) => h,
        Err(e) => {
            println!(
                "{{\"driver\":\"engine-terminate\",\"spawn_error\":\"{}\"}}",
                e
            );
            std::process::exit(2);
        }
    };

    // Bounded read for tree live marker.
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut collected = String::new();
    while Instant::now() < deadline {
        if let Ok(bytes) = handle.read_output()
            && !bytes.is_empty()
        {
            collected.push_str(&String::from_utf8_lossy(&bytes));
            if collected.contains("OMEN_COMPAT_TREE_LIVE") {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let parent_pid = handle.pid().unwrap_or(0);
    let mut desc_pid = 0u32;
    if let Some(idx) = collected.rfind("\"child_pid\":") {
        let rest = &collected[idx + "\"child_pid\":".len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        desc_pid = digits.parse().unwrap_or(0);
    }

    let before = format!(
        "\"parent_alive_before\":{},\"desc_alive_before\":{},\"parent_pid\":{parent_pid},\"desc_pid\":{desc_pid}",
        process_alive(parent_pid),
        process_alive(desc_pid)
    );

    let t0 = Instant::now();
    // Flush before-state so the parent can observe liveness even if
    // product terminate() aborts this process (WIN-M0-004).
    println!(
        "{{\"driver\":\"engine-terminate-phase\",\"phase\":\"before_terminate\",\"parent_pid\":{parent_pid},\"desc_pid\":{desc_pid}}}"
    );
    use std::io::Write;
    let _ = std::io::stdout().flush();

    let term = handle.terminate().map(|_| true).unwrap_or(false);
    let elapsed = t0.elapsed();
    std::mem::forget(handle); // avoid Drop after terminate (double-close path)

    let parent_gone = wait_gone(parent_pid, Duration::from_secs(5));
    let desc_gone = desc_pid == 0 || wait_gone(desc_pid, Duration::from_secs(5));
    println!(
        "{{\"driver\":\"engine-terminate\",\"terminate_ok\":{term},\"elapsed_ms\":{},\"parent_gone\":{parent_gone},\"desc_gone\":{desc_gone},{before}}}",
        elapsed.as_millis()
    );
    std::process::exit(0);
}

#[cfg(not(windows))]
fn main() {
    println!("{{\"driver\":\"engine-terminate\",\"unsupported\":true}}");
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
    };
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let w = WaitForSingleObject(h, 0);
        CloseHandle(h);
        w != WAIT_OBJECT_0
    }
}

#[cfg(windows)]
fn wait_gone(pid: u32, bound: Duration) -> bool {
    let deadline = Instant::now() + bound;
    while Instant::now() < deadline {
        if !process_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    !process_alive(pid)
}

#[cfg(windows)]
use std::time::{Duration, Instant};
