use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, RwLock, watch};
use tracing::{error, info};
use uuid::Uuid;

use omen_ipc::{
    ClientHello, DaemonHello, DaemonMessage, IpcRequest, IpcResponse, LocalIpcError,
    MAX_FRAME_SIZE, ManagedServiceInfo, PlatformListener, RequestPayload, ResponsePayload,
    default_endpoint_address, negotiate_protocol_version, read_json_frame, write_json_frame,
};

use crate::registry::WorkspaceRegistry;
use crate::workspace::WorkspaceState;

pub struct DaemonServer {
    instance_id: String,
    epoch: u64,
    endpoint: String,
    registry: Arc<WorkspaceRegistry>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
}

impl DaemonServer {
    pub fn new(endpoint: Option<String>) -> Self {
        let endpoint = endpoint.unwrap_or_else(default_endpoint_address);
        let epoch = chrono::Utc::now().timestamp_millis() as u64;
        let instance_id = format!("dmn_{}", Uuid::new_v4());
        let registry = Arc::new(WorkspaceRegistry::new(epoch));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        Self {
            instance_id,
            epoch,
            endpoint,
            registry,
            shutdown_tx,
            shutdown_rx,
        }
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
                            let shutdown_rx = self.shutdown_rx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_connection(stream, instance_id, registry, shutdown_rx).await {
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
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<(), LocalIpcError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut read_half, write_half) = tokio::io::split(stream);
        let write_mutex = Arc::new(Mutex::new(write_half));

        // 1. Handshake
        let client_hello: Option<ClientHello> = read_json_frame(&mut read_half).await?;
        let client_hello = client_hello
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
                            let should_disconnect = matches!(request.payload, RequestPayload::Disconnect);
                            let response = Self::process_request(
                                request,
                                registry.clone(),
                                attached_workspace.clone(),
                                write_mutex.clone(),
                                event_pump_handle.clone(),
                            ).await;

                            let mut writer = write_mutex.lock().await;
                            let msg = DaemonMessage::Response(response);
                            if let Err(e) = write_json_frame(&mut *writer, &msg).await {
                                error!("Failed to write response: {e}");
                                break;
                            }

                            if should_disconnect {
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
        write_mutex: Arc<Mutex<W>>,
        event_pump_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
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
                let ws = registry.get_or_attach(Path::new(&canonical_path)).await;
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
                    Some(ws) => {
                        if let Some(existing) = ws.get_service(&name).await
                            && existing.state == "running"
                        {
                            return IpcResponse::err(
                                req_id,
                                LocalIpcError::ServiceAlreadyRunning(format!(
                                    "Service '{name}' is already running with PID {:?}",
                                    existing.pid
                                )),
                            );
                        }

                        let info = ManagedServiceInfo {
                            name: name.clone(),
                            resource_uri: format!("proc://workspace/{name}"),
                            pid: Some(std::process::id()),
                            command: format!("{} {}", command, argv.join(" ")),
                            state: "running".into(),
                            uptime_secs: 0,
                        };
                        ws.register_service(info.clone()).await;
                        Ok(ResponsePayload::ServiceStarted(info))
                    }
                    None => Err(LocalIpcError::WorkspaceNotAttached(
                        "No workspace attached for this session".into(),
                    )),
                }
            }

            RequestPayload::StopService { name } => {
                let current = attached_workspace.read().await;
                match &*current {
                    Some(ws) => {
                        if ws.update_service_state(&name, "stopped", None).await {
                            Ok(ResponsePayload::ServiceStopped { name })
                        } else {
                            Err(LocalIpcError::ServiceNotFound(format!(
                                "Service '{name}' not found"
                            )))
                        }
                    }
                    None => Err(LocalIpcError::WorkspaceNotAttached(
                        "No workspace attached for this session".into(),
                    )),
                }
            }

            RequestPayload::RestartService { name } => {
                let current = attached_workspace.read().await;
                match &*current {
                    Some(ws) => {
                        if let Some(mut existing) = ws.get_service(&name).await {
                            existing.state = "running".into();
                            existing.uptime_secs = 0;
                            ws.register_service(existing.clone()).await;
                            Ok(ResponsePayload::ServiceRestarted(existing))
                        } else {
                            Err(LocalIpcError::ServiceNotFound(format!(
                                "Service '{name}' not found"
                            )))
                        }
                    }
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

            RequestPayload::ServiceLogs { name, .. } => {
                let current = attached_workspace.read().await;
                match &*current {
                    Some(ws) => {
                        if ws.get_service(&name).await.is_some() {
                            Ok(ResponsePayload::ServiceLogsResponse {
                                name,
                                lines: vec!["Service log initialized".to_string()],
                                cas_uri: None,
                            })
                        } else {
                            Err(LocalIpcError::ServiceNotFound(format!(
                                "Service '{name}' not found"
                            )))
                        }
                    }
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

            RequestPayload::ReportOfflineGap { .. } => Ok(ResponsePayload::OfflineGapAcknowledged),

            RequestPayload::Disconnect => Ok(ResponsePayload::Success),
        };

        match outcome {
            Ok(payload) => IpcResponse::ok(req_id, payload),
            Err(error) => IpcResponse::err(req_id, error),
        }
    }
}
