use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, RwLock, watch};
use tracing::{error, info};
use uuid::Uuid;

use omen_ipc::{
    ClientHello, DaemonHello, DaemonMessage, IpcRequest, IpcResponse, LocalIpcError,
    MAX_FRAME_SIZE, PlatformListener, RequestPayload, ResponsePayload, default_endpoint_address,
    negotiate_protocol_version, read_json_frame, write_json_frame,
};

use crate::registry::WorkspaceRegistry;
use crate::workspace::WorkspaceState;

pub struct DaemonServer {
    instance_id: String,
    epoch: u64,
    endpoint: String,
    registry: Arc<WorkspaceRegistry>,
    pty_manager: Arc<crate::pty_service::PtySessionManager>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
}

impl DaemonServer {
    pub fn new(endpoint: Option<String>) -> Self {
        let endpoint = endpoint.unwrap_or_else(default_endpoint_address);
        let epoch = chrono::Utc::now().timestamp_millis() as u64;
        let instance_id = format!("dmn_{}", Uuid::new_v4());
        let registry = Arc::new(WorkspaceRegistry::new(epoch));
        let pty_manager = Arc::new(crate::pty_service::PtySessionManager::new());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        Self {
            instance_id,
            epoch,
            endpoint,
            registry,
            pty_manager,
            shutdown_tx,
            shutdown_rx,
        }
    }

    pub fn pty_manager(&self) -> Arc<crate::pty_service::PtySessionManager> {
        self.pty_manager.clone()
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn registry(&self) -> Arc<WorkspaceRegistry> {
        self.registry.clone()
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    pub fn subscribe_shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }

    pub async fn run(&self) -> Result<(), LocalIpcError> {
        let mut listener = PlatformListener::bind(&self.endpoint)
            .await
            .map_err(|e| LocalIpcError::Io(format!("Failed to bind to {}: {e}", self.endpoint)))?;

        info!(
            "omend listening on {}, instance_id: {}, epoch: {}",
            self.endpoint, self.instance_id, self.epoch
        );

        let mut shutdown_rx = self.shutdown_rx.clone();

        loop {
            tokio::select! {
                accept_res = listener.accept() => {
                    match accept_res {
                        Ok(stream) => {
                            let instance_id = self.instance_id.clone();
                            let registry = self.registry.clone();
                            let pty_manager = self.pty_manager.clone();
                            let shutdown_rx = self.shutdown_rx.clone();
                            let shutdown_tx = self.shutdown_tx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_connection_internal(stream, instance_id, registry, pty_manager, shutdown_rx, shutdown_tx).await {
                                    error!("Client connection handler finished with error: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            error!("Error accepting client connection: {e}");
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    info!("omend received shutdown signal, shutting down listener");
                    break;
                }
            }
        }

        Ok(())
    }

    pub async fn handle_connection<S>(
        stream: S,
        daemon_instance_id: String,
        registry: Arc<WorkspaceRegistry>,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Result<(), LocalIpcError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (shutdown_tx, _) = watch::channel(false);
        let pty_manager = Arc::new(crate::pty_service::PtySessionManager::new());
        Self::handle_connection_internal(
            stream,
            daemon_instance_id,
            registry,
            pty_manager,
            shutdown_rx,
            shutdown_tx,
        )
        .await
    }

    pub async fn handle_connection_internal<S>(
        stream: S,
        daemon_instance_id: String,
        registry: Arc<WorkspaceRegistry>,
        pty_manager: Arc<crate::pty_service::PtySessionManager>,
        mut shutdown_rx: watch::Receiver<bool>,
        shutdown_tx: watch::Sender<bool>,
    ) -> Result<(), LocalIpcError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut read_half, write_half) = tokio::io::split(stream);
        let write_mutex = Arc::new(Mutex::new(write_half));

        // 1. Handshake
        let client_hello: ClientHello = read_json_frame(&mut read_half)
            .await?
            .ok_or_else(|| LocalIpcError::Io("Client closed stream during handshake".into()))?;

        let selected_version = match negotiate_protocol_version(
            &client_hello.protocol_version_family,
            &client_hello.supported_versions,
        ) {
            Ok(v) => v,
            Err(e) => return Err(e),
        };

        let daemon_hello = DaemonHello {
            selected_protocol_version: selected_version,
            daemon_instance_id,
            product_version: env!("CARGO_PKG_VERSION").to_string(),
            supported_features: vec![
                "events".to_string(),
                "services".to_string(),
                "execution".to_string(),
            ],
            max_frame_size: MAX_FRAME_SIZE,
        };

        {
            let mut writer = write_mutex.lock().await;
            write_json_frame(&mut *writer, &daemon_hello).await?;
        }

        // 2. Client Session Loop
        let attached_workspace: Arc<RwLock<Option<Arc<WorkspaceState>>>> =
            Arc::new(RwLock::new(None));
        let event_pump_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>> =
            Arc::new(Mutex::new(None));

        loop {
            tokio::select! {
                req_res = read_json_frame::<IpcRequest, _>(&mut read_half) => {
                    match req_res {
                        Ok(Some(request)) => {
                            let should_stop = matches!(request.payload, RequestPayload::Disconnect | RequestPayload::Shutdown);
                            let response = Self::process_request(
                                request,
                                registry.clone(),
                                attached_workspace.clone(),
                                pty_manager.clone(),
                                write_mutex.clone(),
                                event_pump_handle.clone(),
                                shutdown_tx.clone(),
                            ).await;

                            let mut writer = write_mutex.lock().await;
                            let msg = DaemonMessage::Response(response);
                            if let Err(e) = write_json_frame(&mut *writer, &msg).await {
                                error!("Failed to write response: {e}");
                                break;
                            }

                            if should_stop {
                                break;
                            }
                        }
                        Ok(None) => {
                            break;
                        }
                        Err(e) => {
                            error!("Error reading client request frame: {e}");
                            break;
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    break;
                }
            }
        }

        // Abort active event pump task on client disconnect
        let mut pump = event_pump_handle.lock().await;
        if let Some(handle) = pump.take() {
            handle.abort();
        }

        Ok(())
    }

    async fn process_request<W>(
        request: IpcRequest,
        registry: Arc<WorkspaceRegistry>,
        attached_workspace: Arc<RwLock<Option<Arc<WorkspaceState>>>>,
        pty_manager: Arc<crate::pty_service::PtySessionManager>,
        write_mutex: Arc<Mutex<W>>,
        event_pump_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
        shutdown_tx: watch::Sender<bool>,
    ) -> IpcResponse
    where
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let req_id = request.request_id.clone();
        let session_id = request
            .session_id
            .clone()
            .unwrap_or_else(|| "sess_anonymous".to_string());
        let outcome = match request.payload {
            RequestPayload::Ping { timestamp_ms } => Ok(ResponsePayload::Pong { timestamp_ms }),

            RequestPayload::AttachWorkspace { canonical_path } => {
                match registry.get_or_attach(Path::new(&canonical_path)).await {
                    Ok(ws) => {
                        let ws_id = ws.workspace_id().to_string();
                        let epoch = ws.epoch();

                        // Save attached workspace
                        {
                            let mut current = attached_workspace.write().await;
                            *current = Some(ws.clone());
                        }

                        // Setup event forwarding pump for this workspace
                        let mut rx = ws.subscribe_events();
                        let writer_clone = write_mutex.clone();

                        let mut pump = event_pump_handle.lock().await;
                        if let Some(old) = pump.take() {
                            old.abort();
                        }

                        let new_handle = tokio::spawn(async move {
                            while let Ok(event) = rx.recv().await {
                                let msg = DaemonMessage::Event(event);
                                let mut writer = writer_clone.lock().await;
                                if write_json_frame(&mut *writer, &msg).await.is_err() {
                                    break;
                                }
                            }
                        });
                        *pump = Some(new_handle);

                        Ok(ResponsePayload::WorkspaceAttached {
                            workspace_id: ws_id,
                            epoch,
                        })
                    }
                    Err(e) => Err(e),
                }
            }

            RequestPayload::GetSnapshot => {
                let current = attached_workspace.read().await;
                match &*current {
                    Some(ws) => {
                        let snapshot = ws.get_snapshot().await;
                        Ok(ResponsePayload::Snapshot(snapshot))
                    }
                    None => Err(LocalIpcError::WorkspaceNotAttached(
                        "No workspace attached for this session".into(),
                    )),
                }
            }

            RequestPayload::QueryFact { resource_uri } => {
                let current = attached_workspace.read().await;
                match &*current {
                    Some(ws) => {
                        let fact = ws.query_fact(&resource_uri).await;
                        Ok(ResponsePayload::FactResponse { fact })
                    }
                    None => Err(LocalIpcError::WorkspaceNotAttached(
                        "No workspace attached for this session".into(),
                    )),
                }
            }

            RequestPayload::StartService {
                name,
                command,
                argv,
            } => {
                let current = attached_workspace.read().await;
                match &*current {
                    Some(ws) => ws
                        .start_managed_service(&name, &command, &argv)
                        .await
                        .map(ResponsePayload::ServiceStarted),
                    None => Err(LocalIpcError::WorkspaceNotAttached(
                        "No workspace attached for this session".into(),
                    )),
                }
            }

            RequestPayload::StopService { name } => {
                let current = attached_workspace.read().await;
                match &*current {
                    Some(ws) => ws
                        .stop_managed_service(&name)
                        .await
                        .map(|name| ResponsePayload::ServiceStopped { name }),
                    None => Err(LocalIpcError::WorkspaceNotAttached(
                        "No workspace attached for this session".into(),
                    )),
                }
            }

            RequestPayload::RestartService { name } => {
                let current = attached_workspace.read().await;
                match &*current {
                    Some(ws) => ws
                        .restart_managed_service(&name)
                        .await
                        .map(ResponsePayload::ServiceRestarted),
                    None => Err(LocalIpcError::WorkspaceNotAttached(
                        "No workspace attached for this session".into(),
                    )),
                }
            }

            RequestPayload::ListServices => {
                let current = attached_workspace.read().await;
                match &*current {
                    Some(ws) => {
                        let services = ws.list_services().await;
                        Ok(ResponsePayload::ServicesList { services })
                    }
                    None => Err(LocalIpcError::WorkspaceNotAttached(
                        "No workspace attached for this session".into(),
                    )),
                }
            }

            RequestPayload::ServiceLogs { name, tail_lines } => {
                let current = attached_workspace.read().await;
                match &*current {
                    Some(ws) => ws
                        .service_logs(&name, tail_lines)
                        .await
                        .map(|(lines, cas_uri)| ResponsePayload::ServiceLogsResponse {
                            name,
                            lines,
                            cas_uri,
                        }),
                    None => Err(LocalIpcError::WorkspaceNotAttached(
                        "No workspace attached for this session".into(),
                    )),
                }
            }

            RequestPayload::SubmitExecution {
                tool,
                operation,
                args,
                cwd,
                timeout_ms,
                consequential_request_id,
            } => {
                if let Some(ref request_id) = consequential_request_id
                    && let Err(message) = validate_consequential_request_id(request_id)
                {
                    return IpcResponse::err(req_id, LocalIpcError::MalformedRequest(message));
                }
                let current = attached_workspace.read().await;
                match &*current {
                    Some(ws) => {
                        let dedup_id = consequential_request_id.unwrap_or_else(|| req_id.clone());
                        let params = crate::workspace::BrokerExecutionParams {
                            dedup_id: &dedup_id,
                            session_id: &session_id,
                            tool: &tool,
                            operation: &operation,
                            args: &args,
                            cwd: &cwd,
                            timeout_ms,
                        };
                        let res = ws.execute_broker(params).await;
                        res.map(ResponsePayload::ExecutionFinished)
                    }
                    None => Err(LocalIpcError::WorkspaceNotAttached(
                        "No workspace attached for this session".into(),
                    )),
                }
            }

            RequestPayload::QueryRequestStatus {
                consequential_request_id,
            } => {
                let current = attached_workspace.read().await;
                match &*current {
                    Some(ws) => {
                        if let Some(rec) = ws.query_request_receipt(&consequential_request_id).await
                        {
                            let status = match rec.status.as_str() {
                                "Completed" => omen_ipc::ExecutionStatusCode::Completed,
                                "Running" => omen_ipc::ExecutionStatusCode::Running,
                                "Accepted" => omen_ipc::ExecutionStatusCode::Accepted,
                                "Failed" => omen_ipc::ExecutionStatusCode::Failed,
                                _ => omen_ipc::ExecutionStatusCode::Unknown,
                            };
                            Ok(ResponsePayload::RequestStatus(
                                omen_ipc::ExecutionStatusRecord {
                                    consequential_request_id,
                                    execution_id: rec.execution_id,
                                    status,
                                },
                            ))
                        } else {
                            Ok(ResponsePayload::RequestStatus(
                                omen_ipc::ExecutionStatusRecord {
                                    consequential_request_id,
                                    execution_id: None,
                                    status: omen_ipc::ExecutionStatusCode::NotSeen,
                                },
                            ))
                        }
                    }
                    None => Err(LocalIpcError::WorkspaceNotAttached(
                        "No workspace attached for this session".into(),
                    )),
                }
            }

            RequestPayload::RecordHistory {
                session_id,
                command,
                exit_code,
                duration_ms,
                stdout_artifact,
                stderr_artifact,
            } => {
                let current = attached_workspace.read().await;
                match &*current {
                    Some(ws) => {
                        match ws
                            .record_history(
                                &session_id,
                                &command,
                                exit_code,
                                duration_ms,
                                stdout_artifact,
                                stderr_artifact,
                            )
                            .await
                        {
                            Ok(_) => Ok(ResponsePayload::HistoryRecorded),
                            Err(e) => Err(LocalIpcError::InternalRuntimeError(e.to_string())),
                        }
                    }
                    None => Err(LocalIpcError::WorkspaceNotAttached(
                        "No workspace attached for this session".into(),
                    )),
                }
            }

            RequestPayload::QueryLastExecution { session_id } => {
                let current = attached_workspace.read().await;
                match &*current {
                    Some(ws) => match ws.get_last_execution(&session_id).await {
                        Ok(Some(rec)) => Ok(ResponsePayload::LastExecutionResponse {
                            command: Some(rec.command),
                            exit_code: rec.exit_code,
                            execution_id: Some(rec.execution_id.to_string()),
                        }),
                        Ok(None) => Ok(ResponsePayload::LastExecutionResponse {
                            command: None,
                            exit_code: None,
                            execution_id: None,
                        }),
                        Err(e) => Err(LocalIpcError::InternalRuntimeError(e.to_string())),
                    },
                    None => Err(LocalIpcError::WorkspaceNotAttached(
                        "No workspace attached for this session".into(),
                    )),
                }
            }

            RequestPayload::CreatePtySession {
                session_id,
                argv,
                cwd,
                env,
                rows,
                cols,
            } => pty_manager
                .create(
                    &session_id,
                    argv,
                    Path::new(&cwd).to_path_buf(),
                    env,
                    rows,
                    cols,
                )
                .await
                .map(|pid| ResponsePayload::PtySessionCreated { session_id, pid }),

            RequestPayload::AttachPtySession { session_id } => pty_manager
                .attach(&session_id)
                .await
                .map(|(output, state)| ResponsePayload::PtySessionAttached {
                    session_id,
                    output,
                    state: format!("{:?}", state).to_uppercase(),
                }),

            RequestPayload::DetachPtySession { session_id } => pty_manager
                .detach(&session_id)
                .await
                .map(|_| ResponsePayload::PtySessionDetached { session_id }),

            RequestPayload::WritePtyInput { session_id, data } => pty_manager
                .write_input(&session_id, &data)
                .await
                .map(|_| ResponsePayload::PtyInputWritten),

            RequestPayload::ReadPtyOutput { session_id, offset } => {
                pty_manager.read_output(&session_id, offset).await.map(
                    |(output, total_written, state)| ResponsePayload::PtyOutputRead {
                        session_id,
                        output,
                        total_written,
                        state: format!("{:?}", state).to_uppercase(),
                    },
                )
            }

            RequestPayload::ResizePty {
                session_id,
                rows,
                cols,
            } => pty_manager
                .resize(&session_id, rows, cols)
                .await
                .map(|_| ResponsePayload::PtyResized),

            RequestPayload::TerminatePty { session_id } => pty_manager
                .terminate(&session_id)
                .await
                .map(|_| ResponsePayload::PtyTerminated { session_id }),

            RequestPayload::ListPtySessions => {
                let sessions = pty_manager.list().await;
                Ok(ResponsePayload::PtySessionsList { sessions })
            }

            RequestPayload::ReportOfflineGap { .. } => Ok(ResponsePayload::OfflineGapAcknowledged),

            RequestPayload::Shutdown => {
                let _ = shutdown_tx.send(true);
                Ok(ResponsePayload::DaemonShuttingDown)
            }

            RequestPayload::Disconnect => Ok(ResponsePayload::Success),
        };

        match outcome {
            Ok(payload) => IpcResponse::ok(req_id, payload),
            Err(error) => IpcResponse::err(req_id, error),
        }
    }
}

const MAX_CONSEQUENTIAL_REQUEST_ID_BYTES: usize = 256;

fn validate_consequential_request_id(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("consequential_request_id cannot be empty".into());
    }
    if value.len() > MAX_CONSEQUENTIAL_REQUEST_ID_BYTES {
        return Err(format!(
            "consequential_request_id exceeds {MAX_CONSEQUENTIAL_REQUEST_ID_BYTES} bytes"
        ));
    }
    if value.chars().any(char::is_control) {
        return Err("consequential_request_id contains control characters".into());
    }
    Ok(())
}

#[cfg(test)]
mod identity_tests {
    use super::validate_consequential_request_id;

    #[test]
    fn request_id_validation_is_bounded_and_machine_safe() {
        assert!(validate_consequential_request_id("req-1").is_ok());
        assert!(validate_consequential_request_id("").is_err());
        assert!(validate_consequential_request_id("req\n1").is_err());
        assert!(validate_consequential_request_id(&"x".repeat(257)).is_err());
    }
}
