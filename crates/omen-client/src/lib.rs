use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, oneshot};
use uuid::Uuid;

use omen_ipc::{
    ClientHello, DaemonHello, DaemonMessage, ExecutionResultSummary, ExecutionStatusRecord,
    FactInfo, IpcEvent, IpcRequest, LocalIpcError, ManagedServiceInfo, PlatformStream,
    RequestPayload, ResponsePayload, SharedIndexSnapshot, default_endpoint_address,
    read_json_frame, write_json_frame,
};

type PendingRequests =
    Arc<Mutex<HashMap<String, oneshot::Sender<Result<ResponsePayload, LocalIpcError>>>>>;

struct QueuedRequest {
    request: IpcRequest,
    reply_tx: oneshot::Sender<Result<ResponsePayload, LocalIpcError>>,
}

#[derive(Clone)]
pub struct OmenClient {
    endpoint: String,
    session_id: String,
    workspace_id: Arc<RwLock<Option<String>>>,
    request_tx: mpsc::Sender<QueuedRequest>,
    event_tx: broadcast::Sender<IpcEvent>,
    is_connected: Arc<AtomicBool>,
    last_sequence: Arc<AtomicU64>,
    last_epoch: Arc<AtomicU64>,
}

impl OmenClient {
    pub async fn connect_default(session_id: Option<String>) -> Result<Self, LocalIpcError> {
        let endpoint = default_endpoint_address();
        Self::connect_to(&endpoint, session_id).await
    }

    pub async fn connect_to(
        endpoint: &str,
        session_id: Option<String>,
    ) -> Result<Self, LocalIpcError> {
        let stream = PlatformStream::connect(endpoint).await.map_err(|e| {
            LocalIpcError::Io(format!("Failed to connect to daemon at {endpoint}: {e}"))
        })?;
        Self::from_stream(stream, Some(endpoint.to_string()), session_id).await
    }

    pub async fn from_stream(
        mut stream: PlatformStream,
        endpoint: Option<String>,
        session_id: Option<String>,
    ) -> Result<Self, LocalIpcError> {
        let _daemon_hello = Self::perform_handshake(&mut stream).await?;
        let session_id = session_id.unwrap_or_else(|| format!("sess_{}", Uuid::new_v4()));
        let endpoint = endpoint.unwrap_or_else(|| "memory://stream".to_string());

        let (mut read_half, mut write_half) = tokio::io::split(stream);
        let (request_tx, mut request_rx) = mpsc::channel::<QueuedRequest>(64);
        let (event_tx, _) = broadcast::channel::<IpcEvent>(256);
        let is_connected = Arc::new(AtomicBool::new(true));

        let pending_requests: PendingRequests = Arc::new(Mutex::new(HashMap::new()));

        // Background writer
        let pending_writer = pending_requests.clone();
        let is_connected_writer = is_connected.clone();
        tokio::spawn(async move {
            while let Some(QueuedRequest { request, reply_tx }) = request_rx.recv().await {
                let req_id = request.request_id.clone();
                {
                    let mut map = pending_writer.lock().await;
                    map.insert(req_id.clone(), reply_tx);
                }
                if let Err(e) = write_json_frame(&mut write_half, &request).await {
                    is_connected_writer.store(false, Ordering::SeqCst);
                    let mut map = pending_writer.lock().await;
                    if let Some(tx) = map.remove(&req_id) {
                        let _ = tx.send(Err(e));
                    }
                    break;
                }
            }
        });

        let last_sequence = Arc::new(AtomicU64::new(0));
        let last_epoch = Arc::new(AtomicU64::new(0));

        // Background reader
        let pending_reader = pending_requests.clone();
        let event_tx_reader = event_tx.clone();
        let is_connected_reader = is_connected.clone();
        let last_sequence_reader = last_sequence.clone();
        let last_epoch_reader = last_epoch.clone();

        tokio::spawn(async move {
            loop {
                let msg: Result<Option<DaemonMessage>, LocalIpcError> =
                    read_json_frame(&mut read_half).await;
                match msg {
                    Ok(Some(DaemonMessage::Response(response))) => {
                        let mut map = pending_reader.lock().await;
                        if let Some(tx) = map.remove(&response.request_id) {
                            let _ = tx.send(response.outcome);
                        }
                    }
                    Ok(Some(DaemonMessage::Event(event))) => {
                        let prev_seq = last_sequence_reader.swap(event.sequence, Ordering::SeqCst);
                        let prev_epoch = last_epoch_reader.swap(event.epoch, Ordering::SeqCst);

                        if prev_epoch != 0 && prev_epoch != event.epoch {
                            let _ = event_tx_reader.send(IpcEvent::new(
                                &event.workspace_id,
                                event.epoch,
                                event.sequence,
                                omen_ipc::EventPayload::ResyncRequired {
                                    reason: "Epoch changed; daemon restarted".to_string(),
                                },
                            ));
                        } else if prev_seq != 0 && event.sequence > prev_seq + 1 {
                            let _ = event_tx_reader.send(IpcEvent::new(
                                &event.workspace_id,
                                event.epoch,
                                event.sequence,
                                omen_ipc::EventPayload::ResyncRequired {
                                    reason: format!(
                                        "Event gap detected: expected {}, received {}",
                                        prev_seq + 1,
                                        event.sequence
                                    ),
                                },
                            ));
                        }

                        let _ = event_tx_reader.send(event);
                    }
                    Ok(None) => {
                        break;
                    }
                    Err(e) => {
                        tracing::error!("Daemon connection error: {e}");
                        break;
                    }
                }
            }
            is_connected_reader.store(false, Ordering::SeqCst);
            let mut map = pending_reader.lock().await;
            for (_, tx) in map.drain() {
                let _ = tx.send(Err(LocalIpcError::Io(
                    "Connection to daemon lost".to_string(),
                )));
            }
        });

        Ok(Self {
            endpoint,
            session_id,
            workspace_id: Arc::new(RwLock::new(None)),
            request_tx,
            event_tx,
            is_connected,
            last_sequence,
            last_epoch,
        })
    }

    async fn perform_handshake(stream: &mut PlatformStream) -> Result<DaemonHello, LocalIpcError> {
        let client_hello = ClientHello::new(
            format!("cli_{}", Uuid::new_v4()),
            vec![
                "events".to_string(),
                "services".to_string(),
                "execution".to_string(),
            ],
        );
        write_json_frame(stream, &client_hello).await?;

        let daemon_hello: Option<DaemonHello> = read_json_frame(stream).await?;
        let daemon_hello = daemon_hello.ok_or_else(|| {
            LocalIpcError::Io("Daemon closed stream during handshake".to_string())
        })?;

        if !client_hello
            .supported_versions
            .contains(&daemon_hello.selected_protocol_version)
        {
            return Err(LocalIpcError::ProtocolVersionUnsupported(format!(
                "Daemon selected unsupported version: {}",
                daemon_hello.selected_protocol_version
            )));
        }

        Ok(daemon_hello)
    }

    pub fn is_connected(&self) -> bool {
        self.is_connected.load(Ordering::SeqCst)
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn active_workspace_id(&self) -> Option<String> {
        self.workspace_id.read().await.clone()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<IpcEvent> {
        self.event_tx.subscribe()
    }

    pub fn last_event_sequence(&self) -> u64 {
        self.last_sequence.load(Ordering::SeqCst)
    }

    pub fn last_event_epoch(&self) -> u64 {
        self.last_epoch.load(Ordering::SeqCst)
    }

    pub async fn send_request_with_id(
        &self,
        request_id: impl Into<String>,
        payload: RequestPayload,
    ) -> Result<ResponsePayload, LocalIpcError> {
        if !self.is_connected() {
            return Err(LocalIpcError::Io(
                "Client is not connected to daemon".to_string(),
            ));
        }
        let request_id = request_id.into();
        let ws_id = self.workspace_id.read().await.clone();
        let request = IpcRequest::new(
            request_id.clone(),
            Some(self.session_id.clone()),
            ws_id,
            payload,
        );

        let (reply_tx, reply_rx) = oneshot::channel();
        self.request_tx
            .send(QueuedRequest { request, reply_tx })
            .await
            .map_err(|_| LocalIpcError::Io("Failed to dispatch request to writer".to_string()))?;

        match reply_rx.await {
            Ok(outcome) => outcome,
            Err(_) => Err(LocalIpcError::Io(
                "Daemon closed connection before responding".to_string(),
            )),
        }
    }

    pub async fn send_request(
        &self,
        payload: RequestPayload,
    ) -> Result<ResponsePayload, LocalIpcError> {
        let request_id = format!("req_{}", Uuid::new_v4());
        self.send_request_with_id(request_id, payload).await
    }

    pub async fn ping(&self) -> Result<u64, LocalIpcError> {
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let resp = self
            .send_request(RequestPayload::Ping { timestamp_ms: now })
            .await?;
        match resp {
            ResponsePayload::Pong { timestamp_ms } => Ok(timestamp_ms),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected Pong, got {other:?}"
            ))),
        }
    }

    pub async fn attach_workspace(
        &self,
        canonical_path: impl Into<String>,
    ) -> Result<(String, u64), LocalIpcError> {
        let resp = self
            .send_request(RequestPayload::AttachWorkspace {
                canonical_path: canonical_path.into(),
            })
            .await?;
        match resp {
            ResponsePayload::WorkspaceAttached {
                workspace_id,
                epoch,
            } => {
                let mut ws = self.workspace_id.write().await;
                *ws = Some(workspace_id.clone());
                Ok((workspace_id, epoch))
            }
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected WorkspaceAttached, got {other:?}"
            ))),
        }
    }

    pub async fn get_snapshot(&self) -> Result<SharedIndexSnapshot, LocalIpcError> {
        let resp = self.send_request(RequestPayload::GetSnapshot).await?;
        match resp {
            ResponsePayload::Snapshot(snapshot) => Ok(snapshot),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected Snapshot, got {other:?}"
            ))),
        }
    }

    pub async fn submit_execution(
        &self,
        tool: impl Into<String>,
        operation: impl Into<String>,
        args: Vec<String>,
        cwd: impl Into<String>,
        timeout_ms: u64,
    ) -> Result<ExecutionResultSummary, LocalIpcError> {
        let resp = self
            .send_request(RequestPayload::SubmitExecution {
                tool: tool.into(),
                operation: operation.into(),
                args,
                cwd: cwd.into(),
                timeout_ms,
                consequential_request_id: None,
            })
            .await?;
        match resp {
            ResponsePayload::ExecutionFinished(summary) => Ok(summary),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected ExecutionFinished, got {other:?}"
            ))),
        }
    }

    pub async fn submit_consequential_execution(
        &self,
        consequential_request_id: impl Into<String>,
        tool: impl Into<String>,
        operation: impl Into<String>,
        args: Vec<String>,
        cwd: impl Into<String>,
        timeout_ms: u64,
    ) -> Result<ExecutionResultSummary, LocalIpcError> {
        let cid = consequential_request_id.into();
        let resp = self
            .send_request_with_id(
                cid.clone(),
                RequestPayload::SubmitExecution {
                    tool: tool.into(),
                    operation: operation.into(),
                    args,
                    cwd: cwd.into(),
                    timeout_ms,
                    consequential_request_id: Some(cid),
                },
            )
            .await?;
        match resp {
            ResponsePayload::ExecutionFinished(summary) => Ok(summary),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected ExecutionFinished, got {other:?}"
            ))),
        }
    }

    pub async fn query_request_status(
        &self,
        consequential_request_id: impl Into<String>,
    ) -> Result<ExecutionStatusRecord, LocalIpcError> {
        let resp = self
            .send_request(RequestPayload::QueryRequestStatus {
                consequential_request_id: consequential_request_id.into(),
            })
            .await?;
        match resp {
            ResponsePayload::RequestStatus(record) => Ok(record),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected RequestStatus, got {other:?}"
            ))),
        }
    }

    /// E2 stop truth: request cancellation of a live brokered execution by
    /// canonical execution ID. Returns intent-vs-proof distinguished truth
    /// (`TerminationConfirmed` only when physical death was observed;
    /// `DispatchPrevented` when the stop arrived before any spawn).
    pub async fn cancel_execution(
        &self,
        execution_id: impl Into<String>,
    ) -> Result<omen_ipc::CancelRecord, LocalIpcError> {
        let resp = self
            .send_request(RequestPayload::CancelExecution {
                execution_id: execution_id.into(),
            })
            .await?;
        match resp {
            ResponsePayload::CancelResult(record) => Ok(record),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected CancelResult, got {other:?}"
            ))),
        }
    }

    pub async fn start_service(
        &self,
        name: impl Into<String>,
        command: impl Into<String>,
        argv: Vec<String>,
    ) -> Result<ManagedServiceInfo, LocalIpcError> {
        let resp = self
            .send_request(RequestPayload::StartService {
                name: name.into(),
                command: command.into(),
                argv,
            })
            .await?;
        match resp {
            ResponsePayload::ServiceStarted(info) => Ok(info),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected ServiceStarted, got {other:?}"
            ))),
        }
    }

    pub async fn stop_service(&self, name: impl Into<String>) -> Result<String, LocalIpcError> {
        let resp = self
            .send_request(RequestPayload::StopService { name: name.into() })
            .await?;
        match resp {
            ResponsePayload::ServiceStopped { name } => Ok(name),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected ServiceStopped, got {other:?}"
            ))),
        }
    }

    pub async fn restart_service(
        &self,
        name: impl Into<String>,
    ) -> Result<ManagedServiceInfo, LocalIpcError> {
        let resp = self
            .send_request(RequestPayload::RestartService { name: name.into() })
            .await?;
        match resp {
            ResponsePayload::ServiceRestarted(info) => Ok(info),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected ServiceRestarted, got {other:?}"
            ))),
        }
    }

    pub async fn list_services(&self) -> Result<Vec<ManagedServiceInfo>, LocalIpcError> {
        let resp = self.send_request(RequestPayload::ListServices).await?;
        match resp {
            ResponsePayload::ServicesList { services } => Ok(services),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected ServicesList, got {other:?}"
            ))),
        }
    }

    pub async fn service_logs(
        &self,
        name: impl Into<String>,
        tail_lines: usize,
    ) -> Result<(Vec<String>, Option<String>), LocalIpcError> {
        let resp = self
            .send_request(RequestPayload::ServiceLogs {
                name: name.into(),
                tail_lines,
            })
            .await?;
        match resp {
            ResponsePayload::ServiceLogsResponse { lines, cas_uri, .. } => Ok((lines, cas_uri)),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected ServiceLogsResponse, got {other:?}"
            ))),
        }
    }

    pub async fn query_fact(
        &self,
        resource_uri: impl Into<String>,
    ) -> Result<Option<FactInfo>, LocalIpcError> {
        let resp = self
            .send_request(RequestPayload::QueryFact {
                resource_uri: resource_uri.into(),
            })
            .await?;
        match resp {
            ResponsePayload::FactResponse { fact } => Ok(fact),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected FactResponse, got {other:?}"
            ))),
        }
    }

    pub async fn record_history(
        &self,
        command: impl Into<String>,
        exit_code: Option<i32>,
        duration_ms: u64,
        stdout_artifact: Option<String>,
        stderr_artifact: Option<String>,
    ) -> Result<(), LocalIpcError> {
        let resp = self
            .send_request(RequestPayload::RecordHistory {
                session_id: self.session_id.clone(),
                command: command.into(),
                exit_code,
                duration_ms,
                stdout_artifact,
                stderr_artifact,
            })
            .await?;
        match resp {
            ResponsePayload::HistoryRecorded => Ok(()),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected HistoryRecorded, got {other:?}"
            ))),
        }
    }

    pub async fn query_last_execution(
        &self,
        session_id: Option<&str>,
    ) -> Result<Option<String>, LocalIpcError> {
        let sid = session_id.unwrap_or(&self.session_id).to_string();
        let resp = self
            .send_request(RequestPayload::QueryLastExecution { session_id: sid })
            .await?;
        match resp {
            ResponsePayload::LastExecutionResponse { command, .. } => Ok(command),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected LastExecutionResponse, got {other:?}"
            ))),
        }
    }

    pub async fn report_offline_gap(
        &self,
        modified_paths: Vec<String>,
    ) -> Result<(), LocalIpcError> {
        let resp = self
            .send_request(RequestPayload::ReportOfflineGap { modified_paths })
            .await?;
        match resp {
            ResponsePayload::OfflineGapAcknowledged => Ok(()),
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected OfflineGapAcknowledged, got {other:?}"
            ))),
        }
    }

    pub async fn shutdown_daemon(&self) -> Result<(), LocalIpcError> {
        let resp = self.send_request(RequestPayload::Shutdown).await?;
        match resp {
            ResponsePayload::DaemonShuttingDown => {
                self.is_connected.store(false, Ordering::SeqCst);
                Ok(())
            }
            other => Err(LocalIpcError::MalformedRequest(format!(
                "Expected DaemonShuttingDown, got {other:?}"
            ))),
        }
    }

    pub async fn disconnect(&self) -> Result<(), LocalIpcError> {
        let _ = self.send_request(RequestPayload::Disconnect).await;
        self.is_connected.store(false, Ordering::SeqCst);
        Ok(())
    }
}
