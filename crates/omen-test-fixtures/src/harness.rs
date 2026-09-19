use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// Standard timeout classes according to Omen test doctrine
pub const UNIT_TIMEOUT: Duration = Duration::from_secs(5);
pub const INTEGRATION_TIMEOUT: Duration = Duration::from_secs(30);
pub const PROCESS_TIMEOUT: Duration = Duration::from_secs(30);
pub const DAEMON_TIMEOUT: Duration = Duration::from_secs(30);
pub const LONG_INTEGRATION_TIMEOUT: Duration = Duration::from_secs(60);

/// Tracks test execution phases and state to provide rich diagnostics upon timeout.
#[derive(Debug, Clone)]
pub struct TestContext {
    test_name: String,
    current_phase: Arc<Mutex<String>>,
    daemon_status: Arc<Mutex<String>>,
    client_status: Arc<Mutex<String>>,
    child_processes: Arc<Mutex<usize>>,
    last_event: Arc<Mutex<Option<String>>>,
    extra_evidence: Arc<Mutex<Vec<String>>>,
}

impl TestContext {
    pub fn new(test_name: impl Into<String>) -> Self {
        Self {
            test_name: test_name.into(),
            current_phase: Arc::new(Mutex::new("INIT".to_string())),
            daemon_status: Arc::new(Mutex::new("not started".to_string())),
            client_status: Arc::new(Mutex::new("not connected".to_string())),
            child_processes: Arc::new(Mutex::new(0)),
            last_event: Arc::new(Mutex::new(None)),
            extra_evidence: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn phase(&self, phase_name: impl Into<String>) {
        let p = phase_name.into();
        let mut guard = self.current_phase.lock().unwrap();
        *guard = p;
    }

    pub fn set_daemon_status(&self, status: impl Into<String>) {
        let mut guard = self.daemon_status.lock().unwrap();
        *guard = status.into();
    }

    pub fn set_client_status(&self, status: impl Into<String>) {
        let mut guard = self.client_status.lock().unwrap();
        *guard = status.into();
    }

    pub fn set_child_processes(&self, count: usize) {
        let mut guard = self.child_processes.lock().unwrap();
        *guard = count;
    }

    pub fn record_event(&self, event: impl Into<String>) {
        let mut guard = self.last_event.lock().unwrap();
        *guard = Some(event.into());
    }

    pub fn add_evidence(&self, msg: impl Into<String>) {
        let mut guard = self.extra_evidence.lock().unwrap();
        guard.push(msg.into());
    }

    pub fn format_diagnostics(&self, elapsed: Duration) -> String {
        let phase = self.current_phase.lock().unwrap().clone();
        let daemon = self.daemon_status.lock().unwrap().clone();
        let client = self.client_status.lock().unwrap().clone();
        let children = *self.child_processes.lock().unwrap();
        let last_evt = self
            .last_event
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "none".to_string());
        let evidence = self.extra_evidence.lock().unwrap().clone();

        let mut out = format!(
            "TEST TIMEOUT after {}s\n\ntest:\n  {}\n\nlast phase:\n  {}\n\ndaemon:\n  {}\n\nclient:\n  {}\n\nchild processes:\n  {}\n\nlast event:\n  {}",
            elapsed.as_secs(),
            self.test_name,
            phase,
            daemon,
            client,
            children,
            last_evt,
        );

        if !evidence.is_empty() {
            out.push_str("\n\nevidence:\n");
            for item in evidence {
                out.push_str(&format!("  - {item}\n"));
            }
        }

        out
    }
}

/// Executes a test future with an explicit outer deadline and structured diagnostic failure reporting.
pub async fn run_with_test_timeout<F, Fut, T>(
    test_name: &str,
    timeout_duration: Duration,
    test_fn: F,
) -> T
where
    F: FnOnce(TestContext) -> Fut,
    Fut: Future<Output = T>,
{
    let ctx = TestContext::new(test_name);
    let ctx_clone = ctx.clone();
    let fut = test_fn(ctx);

    match tokio::time::timeout(timeout_duration, fut).await {
        Ok(result) => result,
        Err(_) => {
            let diag = ctx_clone.format_diagnostics(timeout_duration);
            panic!(
                "\n========================================\n{}\n========================================\n",
                diag
            );
        }
    }
}

/// Executes a generic async operation with a bounded deadline.
pub async fn run_with_timeout<Fut, T>(
    operation_name: &str,
    timeout_duration: Duration,
    fut: Fut,
) -> Result<T, String>
where
    Fut: Future<Output = T>,
{
    tokio::time::timeout(timeout_duration, fut)
        .await
        .map_err(|_| format!("Operation '{operation_name}' timed out after {timeout_duration:?}"))
}

/// Polls a condition at regular intervals until it becomes true or the deadline is exceeded.
pub async fn wait_for_condition<F, Fut>(
    timeout: Duration,
    poll_interval: Duration,
    description: &str,
    mut condition: F,
) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if condition().await {
            return Ok(());
        }
        tokio::time::sleep(poll_interval).await;
    }
    Err(format!(
        "Condition timed out after {timeout:?}: {description}"
    ))
}

/// Awaits an event on a broadcast receiver with a deadline and logs observed events on failure.
pub async fn wait_for_event<T, P>(
    rx: &mut tokio::sync::broadcast::Receiver<T>,
    timeout: Duration,
    description: &str,
    predicate: P,
) -> Result<T, String>
where
    T: Clone + std::fmt::Debug,
    P: Fn(&T) -> bool,
{
    let deadline = tokio::time::Instant::now() + timeout;
    let mut seen_events = Vec::new();

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(format!(
                "Event timeout after {timeout:?}: waiting for {description}. Observed events: {seen_events:?}"
            ));
        }

        let remaining = deadline - now;
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(event)) => {
                if predicate(&event) {
                    return Ok(event);
                }
                seen_events.push(format!("{event:?}"));
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(n))) => {
                seen_events.push(format!("<lagged {n} messages>"));
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                return Err(format!(
                    "Event channel closed while waiting for {description}. Observed events: {seen_events:?}"
                ));
            }
            Err(_) => {
                return Err(format!(
                    "Event timeout after {timeout:?}: waiting for {description}. Observed events: {seen_events:?}"
                ));
            }
        }
    }
}

/// Waits for a child process to exit within a deadline, falling back to killing and reaping it.
pub async fn wait_for_process_exit(
    child: &mut tokio::process::Child,
    timeout: Duration,
    description: &str,
) -> Result<std::process::ExitStatus, String> {
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => Ok(status),
        Ok(Err(e)) => Err(format!("Process {description} wait error: {e}")),
        Err(_) => {
            let _ = child.start_kill();
            let reap_timeout = Duration::from_secs(5);
            let reap_res = tokio::time::timeout(reap_timeout, child.wait()).await;
            Err(format!(
                "Process {description} timed out after {timeout:?}. Killed child (reap: {reap_res:?})"
            ))
        }
    }
}

/// Ensures a child process is forcefully killed and reaped within a deadline.
pub async fn kill_and_reap(
    child: &mut tokio::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, String> {
    let _ = child.start_kill();
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => Ok(status),
        Ok(Err(e)) => Err(format!("Failed to reap killed child: {e}")),
        Err(_) => Err(format!(
            "Timed out after {timeout:?} waiting to reap killed child"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_test_context_diagnostics_formatting() {
        let ctx = TestContext::new("real_interactive_shell_routes_execution_through_daemon_once");
        ctx.phase("waiting for execution-completed event");
        ctx.set_daemon_status("running");
        ctx.set_client_status("connected");
        ctx.set_child_processes(1);
        ctx.record_event("ExecutionAccepted(exec_123)");

        let diag = ctx.format_diagnostics(Duration::from_secs(30));
        assert!(diag.contains("TEST TIMEOUT after 30s"));
        assert!(
            diag.contains("test:\n  real_interactive_shell_routes_execution_through_daemon_once")
        );
        assert!(diag.contains("last phase:\n  waiting for execution-completed event"));
        assert!(diag.contains("daemon:\n  running"));
        assert!(diag.contains("client:\n  connected"));
        assert!(diag.contains("child processes:\n  1"));
        assert!(diag.contains("last event:\n  ExecutionAccepted(exec_123)"));
    }

    #[tokio::test]
    async fn test_run_with_test_timeout_success() {
        let res = run_with_test_timeout("test_success", Duration::from_secs(1), |ctx| async move {
            ctx.phase("COMPUTING");
            42
        })
        .await;
        assert_eq!(res, 42);
    }

    #[tokio::test]
    async fn test_wait_for_condition() {
        let mut count = 0;
        let res = wait_for_condition(
            Duration::from_millis(500),
            Duration::from_millis(10),
            "incrementing count",
            || {
                count += 1;
                async move { count >= 3 }
            },
        )
        .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_wait_for_event() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = tx.send("hello".to_string());
        });

        let res = wait_for_event(
            &mut rx,
            Duration::from_millis(500),
            "hello string event",
            |evt| evt == "hello",
        )
        .await;
        assert_eq!(res.unwrap(), "hello");
    }
}
