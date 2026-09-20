use omen_core::{PtySessionId, PtyState};
use omen_engine::{
    ExecutionBackend, NativeExecutionBackend, PtyExecutionHandle, PtyExecutionRequest,
};
use omen_ipc::{LocalIpcError, PtySessionInfo};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

pub struct ManagedPtySession {
    pub session_id: PtySessionId,
    pub command: String,
    pub handle: Mutex<Box<dyn PtyExecutionHandle>>,
    pub state: RwLock<PtyState>,
    pub attached_clients: AtomicUsize,
    pub started_at: Instant,
}

pub struct PtySessionManager {
    sessions: RwLock<HashMap<String, Arc<ManagedPtySession>>>,
}

impl PtySessionManager {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub async fn create(
        &self,
        session_id_str: &str,
        argv: Vec<String>,
        cwd: PathBuf,
        env: Vec<(String, String)>,
        rows: u16,
        cols: u16,
    ) -> Result<Option<u32>, LocalIpcError> {
        let session_id = PtySessionId::new(session_id_str)
            .map_err(|e| LocalIpcError::InternalRuntimeError(format!("Invalid session ID: {e}")))?;

        let command_str = argv.join(" ");
        let req = PtyExecutionRequest {
            session_id: session_id.clone(),
            argv,
            cwd,
            env,
            rows,
            cols,
        };

        let backend = NativeExecutionBackend::new();
        let handle = backend.spawn_pty(&req).map_err(|e| {
            LocalIpcError::InternalRuntimeError(format!("Failed to spawn PTY: {e}"))
        })?;

        let pid = handle.pid();
        let managed = Arc::new(ManagedPtySession {
            session_id,
            command: command_str,
            handle: Mutex::new(handle),
            state: RwLock::new(PtyState::Running),
            attached_clients: AtomicUsize::new(1),
            started_at: Instant::now(),
        });

        self.sessions
            .write()
            .await
            .insert(session_id_str.to_string(), managed);
        Ok(pid)
    }

    pub async fn attach(&self, session_id_str: &str) -> Result<(Vec<u8>, PtyState), LocalIpcError> {
        let session = self.get(session_id_str).await?;
        session.attached_clients.fetch_add(1, Ordering::SeqCst);
        let mut state_guard = session.state.write().await;
        if *state_guard == PtyState::Detached {
            *state_guard = PtyState::Running;
        }
        let current_state = *state_guard;
        drop(state_guard);

        let mut handle = session.handle.lock().await;
        let output = handle
            .read_output()
            .map_err(|e| LocalIpcError::InternalRuntimeError(e.to_string()))?;
        Ok((output, current_state))
    }

    pub async fn detach(&self, session_id_str: &str) -> Result<(), LocalIpcError> {
        let session = self.get(session_id_str).await?;
        let prev = session.attached_clients.fetch_sub(1, Ordering::SeqCst);
        if prev <= 1 {
            let mut state = session.state.write().await;
            if *state == PtyState::Running {
                *state = PtyState::Detached;
            }
        }
        Ok(())
    }

    pub async fn write_input(
        &self,
        session_id_str: &str,
        data: &[u8],
    ) -> Result<(), LocalIpcError> {
        let session = self.get(session_id_str).await?;
        let mut handle = session.handle.lock().await;
        handle
            .write_input(data)
            .map_err(|e| LocalIpcError::InternalRuntimeError(e.to_string()))
    }

    pub async fn read_output(
        &self,
        session_id_str: &str,
        _offset: usize,
    ) -> Result<(Vec<u8>, usize, PtyState), LocalIpcError> {
        let session = self.get(session_id_str).await?;
        let mut handle = session.handle.lock().await;

        // Check if child process exited
        if let Ok(Some(_exit)) = handle.try_wait() {
            let mut state = session.state.write().await;
            *state = PtyState::Exited;
        }

        let output = handle
            .read_output()
            .map_err(|e| LocalIpcError::InternalRuntimeError(e.to_string()))?;
        let total = output.len();
        let current_state = *session.state.read().await;

        Ok((output, total, current_state))
    }

    pub async fn resize(
        &self,
        session_id_str: &str,
        rows: u16,
        cols: u16,
    ) -> Result<(), LocalIpcError> {
        let session = self.get(session_id_str).await?;
        let mut handle = session.handle.lock().await;
        handle
            .resize(rows, cols)
            .map_err(|e| LocalIpcError::InternalRuntimeError(e.to_string()))
    }

    pub async fn terminate(&self, session_id_str: &str) -> Result<(), LocalIpcError> {
        let session = self.get(session_id_str).await?;
        let mut handle = session.handle.lock().await;
        let _ = handle.terminate();
        let mut state = session.state.write().await;
        *state = PtyState::Exited;
        Ok(())
    }

    pub async fn list(&self) -> Vec<PtySessionInfo> {
        let sessions = self.sessions.read().await;
        let mut list = Vec::new();
        for (id, session) in sessions.iter() {
            let handle = session.handle.lock().await;
            let pid = handle.pid();
            let state = format!("{:?}", *session.state.read().await).to_uppercase();
            let attached_clients = session.attached_clients.load(Ordering::SeqCst);
            let uptime_secs = session.started_at.elapsed().as_secs();

            list.push(PtySessionInfo {
                session_id: id.clone(),
                pid,
                command: session.command.clone(),
                state,
                attached_clients,
                uptime_secs,
            });
        }
        list
    }

    async fn get(&self, session_id_str: &str) -> Result<Arc<ManagedPtySession>, LocalIpcError> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id_str).cloned().ok_or_else(|| {
            LocalIpcError::InternalRuntimeError(format!("PTY session '{session_id_str}' not found"))
        })
    }
}

impl Default for PtySessionManager {
    fn default() -> Self {
        Self::new()
    }
}
