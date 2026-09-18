use crate::error::LocalIpcError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IpcRequest {
    pub request_id: String,
    pub session_id: Option<String>,
    pub workspace_id: Option<String>,
    pub payload: RequestPayload,
}

impl IpcRequest {
    pub fn new(
        request_id: impl Into<String>,
        session_id: Option<String>,
        workspace_id: Option<String>,
        payload: RequestPayload,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            session_id,
            workspace_id,
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum RequestPayload {
    Ping {
        timestamp_ms: u64,
    },
    AttachWorkspace {
        canonical_path: String,
    },
    GetSnapshot,
    SubmitExecution {
        tool: String,
        operation: String,
        args: Vec<String>,
        cwd: String,
        timeout_ms: u64,
    },
    QueryRequestStatus {
        consequential_request_id: String,
    },
    StartService {
        name: String,
        command: String,
        argv: Vec<String>,
    },
    StopService {
        name: String,
    },
    RestartService {
        name: String,
    },
    ListServices,
    ServiceLogs {
        name: String,
        tail_lines: usize,
    },
    QueryFact {
        resource_uri: String,
    },
    RecordHistory {
        session_id: String,
        command: String,
        exit_code: Option<i32>,
        duration_ms: u64,
        stdout_artifact: Option<String>,
        stderr_artifact: Option<String>,
    },
    QueryLastExecution {
        session_id: String,
    },
    ReportOfflineGap {
        modified_paths: Vec<String>,
    },
    Disconnect,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IpcResponse {
    pub request_id: String,
    pub outcome: Result<ResponsePayload, LocalIpcError>,
}

impl IpcResponse {
    pub fn ok(request_id: impl Into<String>, payload: ResponsePayload) -> Self {
        Self {
            request_id: request_id.into(),
            outcome: Ok(payload),
        }
    }

    pub fn err(request_id: impl Into<String>, error: LocalIpcError) -> Self {
        Self {
            request_id: request_id.into(),
            outcome: Err(error),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum ResponsePayload {
    Pong {
        timestamp_ms: u64,
    },
    WorkspaceAttached {
        workspace_id: String,
        epoch: u64,
    },
    Snapshot(SharedIndexSnapshot),
    ExecutionAccepted {
        execution_id: String,
    },
    ExecutionFinished(ExecutionResultSummary),
    RequestStatus(ExecutionStatusRecord),
    ServiceStarted(ManagedServiceInfo),
    ServiceStopped {
        name: String,
    },
    ServiceRestarted(ManagedServiceInfo),
    ServicesList {
        services: Vec<ManagedServiceInfo>,
    },
    ServiceLogsResponse {
        name: String,
        lines: Vec<String>,
        cas_uri: Option<String>,
    },
    FactResponse {
        fact: Option<FactInfo>,
    },
    HistoryRecorded,
    LastExecutionResponse {
        command: Option<String>,
        exit_code: Option<i32>,
        execution_id: Option<String>,
    },
    OfflineGapAcknowledged,
    Success,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IpcEvent {
    pub workspace_id: String,
    pub epoch: u64,
    pub sequence: u64,
    pub payload: EventPayload,
}

impl IpcEvent {
    pub fn new(
        workspace_id: impl Into<String>,
        epoch: u64,
        sequence: u64,
        payload: EventPayload,
    ) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            epoch,
            sequence,
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum EventPayload {
    FactInvalidated {
        fact_id: String,
        resource_uri: String,
        previous_validity: String,
        new_validity: String,
        cause: String,
    },
    FactPublished {
        fact_id: String,
        resource_uri: String,
        validity: String,
        assurance: String,
    },
    ServiceStateChanged {
        name: String,
        state: String,
        pid: Option<u32>,
    },
    ExecutionRecorded {
        execution_id: String,
        session_id: String,
        command: String,
        exit_code: Option<i32>,
    },
    ResyncRequired {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedIndexSnapshot {
    pub workspace_id: String,
    pub epoch: u64,
    pub sequence: u64,
    pub facts: Vec<FactInfo>,
    pub services: Vec<ManagedServiceInfo>,
    pub tools: Vec<String>,
    pub dirty_facts_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FactInfo {
    pub fact_id: String,
    pub resource_uri: String,
    pub value: String,
    pub validity: String,
    pub assurance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedServiceInfo {
    pub name: String,
    pub resource_uri: String,
    pub pid: Option<u32>,
    pub command: String,
    pub state: String,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionResultSummary {
    pub execution_id: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout_preview: String,
    pub stderr_preview: String,
    pub stdout_artifact: Option<String>,
    pub stderr_artifact: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecutionStatusCode {
    NotSeen,
    Accepted,
    Running,
    Completed,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionStatusRecord {
    pub consequential_request_id: String,
    pub execution_id: Option<String>,
    pub status: ExecutionStatusCode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum DaemonMessage {
    Response(IpcResponse),
    Event(IpcEvent),
}

impl DaemonMessage {
    pub fn is_response(&self) -> bool {
        matches!(self, DaemonMessage::Response(_))
    }

    pub fn is_event(&self) -> bool {
        matches!(self, DaemonMessage::Event(_))
    }

    pub fn as_response(&self) -> Option<&IpcResponse> {
        match self {
            DaemonMessage::Response(r) => Some(r),
            _ => None,
        }
    }

    pub fn as_event(&self) -> Option<&IpcEvent> {
        match self {
            DaemonMessage::Event(e) => Some(e),
            _ => None,
        }
    }
}

impl From<IpcResponse> for DaemonMessage {
    fn from(r: IpcResponse) -> Self {
        DaemonMessage::Response(r)
    }
}

impl From<IpcEvent> for DaemonMessage {
    fn from(e: IpcEvent) -> Self {
        DaemonMessage::Event(e)
    }
}
