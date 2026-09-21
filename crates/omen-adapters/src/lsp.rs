use omen_atlas::find_binary_on_path;
use omen_core::{CoreError, ResourceUri, SemanticProviderId, SymbolId};
use omen_semantic::provider::{
    BoxFuture, ProviderCapabilities, ProviderKind, SemanticLookupResult, SemanticProvider,
};
use omen_semantic::types::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, watch};

pub const DEFAULT_LSP_TIMEOUT: Duration = Duration::from_millis(5000);
/// Cold rust-analyzer startup can exceed the ordinary request budget on a
/// clean Windows process. Keep readiness finite, but leave enough headroom
/// for initialization before semantic requests are attempted.
pub const DEFAULT_LSP_READINESS_TIMEOUT: Duration = Duration::from_secs(25);
pub const DEFAULT_MAX_LSP_MESSAGE_BYTES: usize = 16 * 1024 * 1024; // 16 MiB hard-cap

type PendingRequestMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, CoreError>>>>>;

const MAX_SERVER_STATUS_TEXT_CHARS: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspServerStatus {
    pub health: Option<String>,
    pub quiescent: Option<bool>,
    pub message: Option<String>,
}

fn bounded_status_text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(|text| text.chars().take(MAX_SERVER_STATUS_TEXT_CHARS).collect())
}

fn parse_server_status(params: &Value) -> LspServerStatus {
    LspServerStatus {
        health: bounded_status_text(params.get("health")),
        quiescent: params.get("quiescent").and_then(Value::as_bool),
        message: bounded_status_text(params.get("message")),
    }
}

fn format_readiness_evidence(
    status: Option<&LspServerStatus>,
    deadline: Duration,
    reason: &str,
) -> String {
    let health = status
        .and_then(|s| s.health.as_deref())
        .unwrap_or("unknown");
    let quiescent = status
        .and_then(|s| s.quiescent)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".into());
    let message = status
        .and_then(|s| s.message.as_deref())
        .unwrap_or("unknown");
    format!(
        "provider=rust-analyzer phase=readiness.wait health={health} quiescent={quiescent} message={message:?} deadline_ms={} reason={reason}",
        deadline.as_millis()
    )
}

fn initialize_supports_workspace_symbol_scope_kind_filtering(result: &Value) -> bool {
    result
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("experimental"))
        .and_then(|experimental| experimental.get("workspaceSymbolScopeKindFiltering"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn workspace_symbol_all_params(query: &str, filtered_search_supported: bool) -> Value {
    if filtered_search_supported {
        json!({
            "query": query,
            "searchScope": "workspace",
            "searchKind": "allSymbols"
        })
    } else {
        json!({ "query": format!("{query}#") })
    }
}

/// Real JSON-RPC stdio LSP client with request correlation, bounded timeouts, and cancellation.
pub struct LspClient {
    child: Option<Child>,
    stdin: tokio::process::ChildStdin,
    next_id: AtomicU64,
    pending: PendingRequestMap,
    diagnostics_by_file: Arc<Mutex<HashMap<String, Vec<SemanticDiagnostic>>>>,
    reader_task: Option<tokio::task::JoinHandle<()>>,
    workspace_root: PathBuf,
    provider_id: SemanticProviderId,
    generation: SemanticGeneration,
    workspace_symbol_scope_kind_filtering: bool,
    server_status: watch::Receiver<Option<LspServerStatus>>,
}

impl LspClient {
    pub async fn spawn(
        exe_path: &Path,
        args: &[String],
        workspace_root: PathBuf,
        provider_id: SemanticProviderId,
    ) -> Result<Self, CoreError> {
        let mut cmd = Command::new(exe_path);
        cmd.args(args);
        cmd.current_dir(&workspace_root);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::null());

        let mut child = cmd.spawn().map_err(|e| {
            CoreError::ExecutionFailed(format!(
                "Failed to spawn LSP server '{}': {e}",
                exe_path.display()
            ))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            CoreError::ExecutionFailed("Failed to capture LSP server stdin".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            CoreError::ExecutionFailed("Failed to capture LSP server stdout".into())
        })?;

        let pending: PendingRequestMap = Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = pending.clone();

        let diagnostics_by_file: Arc<Mutex<HashMap<String, Vec<SemanticDiagnostic>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let diagnostics_clone = diagnostics_by_file.clone();
        let root_clone = workspace_root.clone();
        let prov_id_clone = provider_id.clone();
        let (server_status_tx, server_status_rx) = watch::channel(None);

        // Background reader reading Content-Length framed JSON-RPC messages
        let reader_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut header_line = String::new();

            loop {
                header_line.clear();
                let mut content_length: Option<usize> = None;

                // Read headers until empty line "\r\n"
                loop {
                    header_line.clear();
                    match reader.read_line(&mut header_line).await {
                        Ok(0) => return, // EOF
                        Ok(_) => {
                            let trimmed = header_line.trim();
                            if trimmed.is_empty() {
                                break;
                            }
                            if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                                content_length = rest.trim().parse::<usize>().ok();
                            }
                        }
                        Err(_) => return,
                    }
                }

                if let Some(len) = content_length {
                    // Hard-cap message size to 16 MiB before allocating
                    if len > DEFAULT_MAX_LSP_MESSAGE_BYTES {
                        tracing::error!(
                            "LSP message size {} bytes exceeds hard-cap of {} bytes",
                            len,
                            DEFAULT_MAX_LSP_MESSAGE_BYTES
                        );
                        let mut map = pending_clone.lock().unwrap();
                        for (_, tx) in map.drain() {
                            let _ = tx.send(Err(CoreError::ExecutionFailed(format!(
                                "LSP message size {len} bytes exceeded maximum allowed {DEFAULT_MAX_LSP_MESSAGE_BYTES} bytes"
                            ))));
                        }
                        return;
                    }

                    let mut body = vec![0u8; len];
                    if reader.read_exact(&mut body).await.is_err() {
                        return;
                    }

                    if let Ok(val) = serde_json::from_slice::<Value>(&body) {
                        if let Some(id_val) = val.get("id").and_then(|id| id.as_u64()) {
                            let mut map = pending_clone.lock().unwrap();
                            if let Some(tx) = map.remove(&id_val) {
                                let _ = tx.send(Ok(val));
                            }
                            // If not in map: safely discarded as a late or canceled response!
                        } else if let Some(method) = val.get("method").and_then(|m| m.as_str()) {
                            // Handle asynchronous server notifications
                            if method == "textDocument/publishDiagnostics"
                                && let Some(params) = val.get("params")
                            {
                                parse_and_store_diagnostics(
                                    params,
                                    &root_clone,
                                    &prov_id_clone,
                                    &diagnostics_clone,
                                );
                            } else if method == "experimental/serverStatus"
                                && let Some(params) = val.get("params")
                            {
                                let _ = server_status_tx.send(Some(parse_server_status(params)));
                            }
                        }
                    }
                }
            }
        });

        Ok(Self {
            child: Some(child),
            stdin,
            next_id: AtomicU64::new(1),
            pending,
            diagnostics_by_file,
            reader_task: Some(reader_task),
            workspace_root,
            provider_id,
            generation: SemanticGeneration::new(1, 1),
            workspace_symbol_scope_kind_filtering: false,
            server_status: server_status_rx,
        })
    }

    pub fn get_diagnostics(&self, file: Option<&str>) -> Vec<SemanticDiagnostic> {
        let guard = self.diagnostics_by_file.lock().unwrap();
        if let Some(f) = file {
            guard.get(f).cloned().unwrap_or_default()
        } else {
            guard.values().flatten().cloned().collect()
        }
    }

    pub async fn send_request(
        &mut self,
        method: &str,
        params: Value,
        timeout_duration: Duration,
    ) -> Result<Value, CoreError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            self.pending.lock().unwrap().insert(id, tx);
        }

        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let body_str = serde_json::to_string(&body).map_err(|e| {
            CoreError::Internal(format!("Failed to serialize JSON-RPC request: {e}"))
        })?;

        let header = format!("Content-Length: {}\r\n\r\n", body_str.len());
        self.stdin
            .write_all(header.as_bytes())
            .await
            .map_err(|e| CoreError::ExecutionFailed(format!("Failed to write LSP header: {e}")))?;
        self.stdin
            .write_all(body_str.as_bytes())
            .await
            .map_err(|e| CoreError::ExecutionFailed(format!("Failed to write LSP body: {e}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| CoreError::ExecutionFailed(format!("Failed to flush LSP request: {e}")))?;

        // Await with bounded timeout
        match tokio::time::timeout(timeout_duration, rx).await {
            Ok(Ok(Ok(response))) => {
                if let Some(error) = response.get("error") {
                    return Err(CoreError::ExecutionFailed(format!(
                        "LSP error response: {error}"
                    )));
                }
                Ok(response.get("result").cloned().unwrap_or(Value::Null))
            }
            Ok(Ok(Err(err))) => Err(err),
            Ok(Err(_)) => {
                self.pending.lock().unwrap().remove(&id);
                Err(CoreError::ExecutionFailed(
                    "LSP server closed channel prematurely".into(),
                ))
            }
            Err(_) => {
                // Timeout! Remove from pending map so any late response is discarded
                self.pending.lock().unwrap().remove(&id);

                // Send $/cancelRequest
                let _ = self
                    .send_notification("$/cancelRequest", json!({ "id": id }))
                    .await;

                Err(CoreError::ExecutionFailed(format!(
                    "LSP request '{method}' (id: {id}) timed out after {:?}",
                    timeout_duration
                )))
            }
        }
    }

    pub async fn send_notification(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(), CoreError> {
        let body = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let body_str = serde_json::to_string(&body).map_err(|e| {
            CoreError::Internal(format!("Failed to serialize JSON-RPC notification: {e}"))
        })?;
        let header = format!("Content-Length: {}\r\n\r\n", body_str.len());
        let _ = self.stdin.write_all(header.as_bytes()).await;
        let _ = self.stdin.write_all(body_str.as_bytes()).await;
        let _ = self.stdin.flush().await;
        Ok(())
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }

    pub async fn initialize(&mut self, timeout_duration: Duration) -> Result<(), CoreError> {
        self.initialize_with_readiness_timeout(timeout_duration, DEFAULT_LSP_READINESS_TIMEOUT)
            .await
    }

    pub async fn initialize_with_readiness_timeout(
        &mut self,
        timeout_duration: Duration,
        readiness_timeout: Duration,
    ) -> Result<(), CoreError> {
        let root_uri = format!(
            "file:///{}",
            self.workspace_root
                .to_string_lossy()
                .replace('\\', "/")
                .trim_start_matches('/')
        );

        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "workspace": {
                    "symbol": {
                        "symbolKind": {
                            "valueSet": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26]
                        }
                    }
                },
                "textDocument": {
                    "definition": { "dynamicRegistration": false },
                    "references": { "dynamicRegistration": false },
                    "publishDiagnostics": { "relatedInformation": true }
                },
                "experimental": {
                    "serverStatusNotification": true
                }
            }
        });

        let initialize_result = self
            .send_request("initialize", params, timeout_duration)
            .await?;
        self.workspace_symbol_scope_kind_filtering =
            initialize_supports_workspace_symbol_scope_kind_filtering(&initialize_result);
        self.send_notification("initialized", json!({})).await?;
        self.wait_for_readiness(readiness_timeout).await
    }

    fn latest_server_status(&self) -> Option<LspServerStatus> {
        self.server_status.borrow().clone()
    }

    async fn wait_for_readiness(&mut self, deadline: Duration) -> Result<(), CoreError> {
        let deadline_at = tokio::time::Instant::now() + deadline;
        loop {
            if self
                .server_status
                .borrow()
                .as_ref()
                .and_then(|status| status.quiescent)
                == Some(true)
            {
                return Ok(());
            }

            let remaining = deadline_at.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                let status = self.latest_server_status();
                return Err(CoreError::ExecutionFailed(format_readiness_evidence(
                    status.as_ref(),
                    deadline,
                    "deadline_exceeded",
                )));
            }

            match tokio::time::timeout(remaining, self.server_status.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    let status = self.latest_server_status();
                    return Err(CoreError::ExecutionFailed(format_readiness_evidence(
                        status.as_ref(),
                        deadline,
                        "server_status_channel_closed",
                    )));
                }
                Err(_) => {
                    let status = self.latest_server_status();
                    return Err(CoreError::ExecutionFailed(format_readiness_evidence(
                        status.as_ref(),
                        deadline,
                        "deadline_exceeded",
                    )));
                }
            }
        }
    }

    async fn workspace_symbol_with_params(
        &mut self,
        params: Value,
        timeout_duration: Duration,
    ) -> Result<Vec<SymbolRecord>, CoreError> {
        let res = self
            .send_request("workspace/symbol", params, timeout_duration)
            .await?;

        let mut symbols = Vec::new();
        if let Some(arr) = res.as_array() {
            for item in arr {
                if let Some(sym) = self.parse_symbol_information(item) {
                    symbols.push(sym);
                }
            }
        }
        Ok(symbols)
    }

    pub async fn workspace_symbol(
        &mut self,
        query: &str,
        timeout_duration: Duration,
    ) -> Result<Vec<SymbolRecord>, CoreError> {
        self.workspace_symbol_with_params(json!({ "query": query }), timeout_duration)
            .await
    }

    pub async fn workspace_symbol_all(
        &mut self,
        query: &str,
        timeout_duration: Duration,
    ) -> Result<Vec<SymbolRecord>, CoreError> {
        let params = workspace_symbol_all_params(query, self.workspace_symbol_scope_kind_filtering);
        self.workspace_symbol_with_params(params, timeout_duration)
            .await
    }

    pub async fn did_open_file(&mut self, file: &str) -> Result<(), CoreError> {
        let path = self.workspace_root.join(file);
        if let Ok(content) = std::fs::read_to_string(&path) {
            let uri = self.to_file_uri(file);
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            let lang = match ext {
                "rs" => "rust",
                "ts" | "tsx" => "typescript",
                "js" | "jsx" => "javascript",
                "py" => "python",
                "go" => "go",
                _ => "plaintext",
            };
            let params = json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": lang,
                    "version": 1,
                    "text": content,
                }
            });
            let _ = self.send_notification("textDocument/didOpen", params).await;
        }
        Ok(())
    }

    pub async fn definition(
        &mut self,
        file: &str,
        line: usize,
        col: usize,
        timeout_duration: Duration,
    ) -> Result<SemanticLookupResult<SourceLocation>, CoreError> {
        let _ = self.did_open_file(file).await;
        let uri = self.to_file_uri(file);

        // Convert canonical UTF-8 byte column to LSP UTF-16 code units
        let full_path = self.workspace_root.join(file);
        let utf16_col = if let Ok(content) = std::fs::read_to_string(&full_path) {
            content
                .lines()
                .nth(line)
                .map(|l| utf8_to_lsp_utf16_col(l, col))
                .unwrap_or(col)
        } else {
            col
        };

        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": utf16_col }
        });

        let res = self
            .send_request("textDocument/definition", params, timeout_duration)
            .await?;

        if res.is_null() {
            return Ok(SemanticLookupResult::NotFound);
        }

        if let Some(arr) = res.as_array() {
            if arr.is_empty() {
                return Ok(SemanticLookupResult::NotFound);
            }
            if arr.len() == 1 {
                if let Some(loc) = self.parse_location(&arr[0]) {
                    return Ok(SemanticLookupResult::Resolved(loc));
                }
            } else {
                let mut candidates = Vec::new();
                for (idx, item) in arr.iter().enumerate() {
                    if let Some(loc) = self.parse_location(item) {
                        let id = SymbolId::new(format!("candidate_{idx}")).unwrap();
                        let uri = ResourceUri::parse(&format!("symbol://candidate/{idx}")).unwrap();
                        candidates.push(SymbolRecord {
                            id,
                            name: format!("candidate_{idx}"),
                            kind: SymbolKind::Other("candidate".into()),
                            location: loc,
                            container_name: None,
                            signature: None,
                            uri,
                            documentation: None,
                        });
                    }
                }
                return Ok(SemanticLookupResult::Ambiguous(candidates));
            }
        } else if let Some(loc) = self.parse_location(&res) {
            return Ok(SemanticLookupResult::Resolved(loc));
        }

        Ok(SemanticLookupResult::NotFound)
    }

    pub async fn references(
        &mut self,
        file: &str,
        line: usize,
        col: usize,
        limit: usize,
        timeout_duration: Duration,
    ) -> Result<SemanticLookupResult<Vec<ReferenceRecord>>, CoreError> {
        let _ = self.did_open_file(file).await;
        let uri = self.to_file_uri(file);

        // Convert canonical UTF-8 byte column to LSP UTF-16 code units
        let full_path = self.workspace_root.join(file);
        let utf16_col = if let Ok(content) = std::fs::read_to_string(&full_path) {
            content
                .lines()
                .nth(line)
                .map(|l| utf8_to_lsp_utf16_col(l, col))
                .unwrap_or(col)
        } else {
            col
        };

        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": utf16_col },
            "context": { "includeDeclaration": true }
        });

        let res = self
            .send_request("textDocument/references", params, timeout_duration)
            .await?;

        if let Some(arr) = res.as_array() {
            let mut refs = Vec::new();
            for item in arr.iter().take(limit) {
                if let Some(loc) = self.parse_location(item) {
                    let sym_id = SymbolId::new("ref").unwrap();
                    refs.push(ReferenceRecord {
                        symbol_id: sym_id,
                        location: loc,
                        is_definition: false,
                        is_write: false,
                        snippet: None,
                    });
                }
            }
            Ok(SemanticLookupResult::Resolved(refs))
        } else {
            Ok(SemanticLookupResult::NotFound)
        }
    }

    pub async fn shutdown(&mut self) -> Result<(), CoreError> {
        let _ = self
            .send_request("shutdown", json!(null), Duration::from_millis(2000))
            .await;
        let _ = self.send_notification("exit", json!(null)).await;
        if let Some(task) = self.reader_task.take() {
            task.abort();
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
        }
        Ok(())
    }

    fn to_file_uri(&self, rel_path: &str) -> String {
        let full = self.workspace_root.join(rel_path);
        let path_str = full.to_string_lossy().replace('\\', "/");
        let path_str = path_str.trim_start_matches('/');
        format!("file:///{path_str}")
    }

    fn parse_location(&self, val: &Value) -> Option<SourceLocation> {
        let uri_str = val.get("uri").or_else(|| val.get("targetUri"))?.as_str()?;
        let range_val = val.get("range").or_else(|| val.get("targetRange"))?;

        let start = range_val.get("start")?;
        let end = range_val.get("end")?;

        let start_line = start.get("line")?.as_u64()? as usize;
        let start_col_raw = start.get("character")?.as_u64()? as usize;
        let end_line = end.get("line")?.as_u64()? as usize;
        let end_col_raw = end.get("character")?.as_u64()? as usize;

        let file = self.uri_to_relative_file(uri_str);
        let full_path = self.workspace_root.join(&file);
        let file_text = std::fs::read_to_string(&full_path).ok();

        // Convert LSP UTF-16 code units to canonical UTF-8 byte column
        let start_col = if let Some(ref text) = file_text {
            text.lines()
                .nth(start_line)
                .map(|l| lsp_utf16_to_utf8_col(l, start_col_raw))
                .unwrap_or(start_col_raw)
        } else {
            start_col_raw
        };

        let end_col = if let Some(ref text) = file_text {
            text.lines()
                .nth(end_line)
                .map(|l| lsp_utf16_to_utf8_col(l, end_col_raw))
                .unwrap_or(end_col_raw)
        } else {
            end_col_raw
        };

        Some(SourceLocation::new(
            file,
            SourceRange::new(start_line, start_col, end_line, end_col),
            self.provider_id.clone(),
            self.generation.clone(),
        ))
    }

    fn parse_symbol_information(&self, val: &Value) -> Option<SymbolRecord> {
        let name = val.get("name")?.as_str()?.to_string();
        let kind_num = val.get("kind")?.as_u64()? as usize;
        let kind = match kind_num {
            1 => SymbolKind::Other("file".into()),
            2 => SymbolKind::Module,
            3 => SymbolKind::Other("namespace".into()),
            4 => SymbolKind::Package,
            5 => SymbolKind::Class,
            6 => SymbolKind::Method,
            7 => SymbolKind::Property,
            8 => SymbolKind::Field,
            9 => SymbolKind::Constructor,
            10 => SymbolKind::Enum,
            11 => SymbolKind::Interface,
            12 => SymbolKind::Function,
            13 => SymbolKind::Variable,
            14 => SymbolKind::Constant,
            _ => SymbolKind::Other(format!("kind_{kind_num}")),
        };

        let loc_val = val.get("location")?;
        let loc = self.parse_location(loc_val)?;
        let container = val
            .get("containerName")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());

        let id_str = format!("{}_{}", name, loc.range.start_line);
        let sym_id = SymbolId::new(id_str).unwrap();
        let uri_path = format!("symbol://workspace/{}", name);
        let uri = ResourceUri::parse(&uri_path)
            .unwrap_or_else(|_| ResourceUri::parse("symbol://workspace/symbol").unwrap());

        Some(SymbolRecord {
            id: sym_id,
            name,
            kind,
            location: loc,
            container_name: container,
            signature: None,
            uri,
            documentation: None,
        })
    }

    fn uri_to_relative_file(&self, uri: &str) -> String {
        uri_to_rel_path(uri, &self.workspace_root)
    }
}

fn uri_to_rel_path(uri: &str, workspace_root: &Path) -> String {
    let path_part = uri.strip_prefix("file:///").unwrap_or(uri);
    // `to_file_uri` omits the leading slash when constructing a Unix URI, so
    // restore it before comparing the URI path with the absolute workspace
    // root. On Windows, `file:///C:/...` must remain `C:/...`.
    let path_part = if cfg!(unix) && uri.starts_with("file:///") {
        format!("/{path_part}")
    } else {
        path_part.to_string()
    };
    let path = Path::new(&path_part);
    if let Ok(rel) = path.strip_prefix(workspace_root) {
        rel.to_string_lossy().replace('\\', "/")
    } else if cfg!(windows) {
        // Windows drive letters are case-insensitive, but Path::strip_prefix
        // compares them textually. rust-analyzer may emit `file:///c:/...`
        // while the process workspace is `C:\\...`; keep the public semantic
        // contract workspace-relative in that case.
        let path_text = path_part.replace('\\', "/");
        let root_text = workspace_root.to_string_lossy().replace('\\', "/");
        let root_text = root_text.strip_prefix("//?/").unwrap_or(&root_text);
        let path_lower = path_text.to_ascii_lowercase();
        let root_lower = root_text.trim_end_matches('/').to_ascii_lowercase();
        if path_lower.starts_with(&(root_lower.clone() + "/")) {
            path_text[root_lower.len() + 1..].to_owned()
        } else if path_lower == root_lower {
            String::new()
        } else {
            path_text
        }
    } else {
        let relative_candidate = path_part.trim_start_matches('/');
        if cfg!(unix)
            && path_part.starts_with('/')
            && workspace_root.join(relative_candidate).exists()
        {
            relative_candidate.replace('\\', "/")
        } else {
            path_part.replace('\\', "/")
        }
    }
}

fn parse_and_store_diagnostics(
    params: &Value,
    workspace_root: &Path,
    provider_id: &SemanticProviderId,
    target: &Arc<Mutex<HashMap<String, Vec<SemanticDiagnostic>>>>,
) {
    let Some(uri_str) = params.get("uri").and_then(|u| u.as_str()) else {
        return;
    };
    let file = uri_to_rel_path(uri_str, workspace_root);
    let diags_arr = params.get("diagnostics").and_then(|d| d.as_array());

    let mut results = Vec::new();
    if let Some(arr) = diags_arr {
        let full_path = workspace_root.join(&file);
        let file_text = std::fs::read_to_string(&full_path).ok();

        for d in arr {
            let message = d
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            let source = d
                .get("source")
                .and_then(|s| s.as_str())
                .unwrap_or("rust-analyzer")
                .to_string();
            let code = d.get("code").map(|c| {
                if let Some(s) = c.as_str() {
                    s.to_string()
                } else {
                    c.to_string()
                }
            });
            let severity_num = d.get("severity").and_then(|s| s.as_u64()).unwrap_or(1);
            let severity = match severity_num {
                1 => DiagnosticSeverity::Error,
                2 => DiagnosticSeverity::Warning,
                3 => DiagnosticSeverity::Information,
                4 => DiagnosticSeverity::Hint,
                _ => DiagnosticSeverity::Error,
            };

            let range_val = d.get("range");
            let (start_line, start_col, end_line, end_col) = if let Some(r) = range_val {
                let sl = r
                    .get("start")
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0) as usize;
                let sc = r
                    .get("start")
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0) as usize;
                let el = r
                    .get("end")
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0) as usize;
                let ec = r
                    .get("end")
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0) as usize;

                let sc_utf8 = if let Some(ref text) = file_text {
                    text.lines()
                        .nth(sl)
                        .map(|l| lsp_utf16_to_utf8_col(l, sc))
                        .unwrap_or(sc)
                } else {
                    sc
                };
                let ec_utf8 = if let Some(ref text) = file_text {
                    text.lines()
                        .nth(el)
                        .map(|l| lsp_utf16_to_utf8_col(l, ec))
                        .unwrap_or(ec)
                } else {
                    ec
                };
                (sl, sc_utf8, el, ec_utf8)
            } else {
                (0, 0, 0, 0)
            };

            let location = SourceLocation::new(
                file.clone(),
                SourceRange::new(start_line, start_col, end_line, end_col),
                provider_id.clone(),
                SemanticGeneration::new(1, 1),
            );

            results.push(SemanticDiagnostic {
                severity,
                message,
                source,
                code,
                location,
                related_locations: Vec::new(),
                provider: provider_id.clone(),
            });
        }
    }

    let mut guard = target.lock().unwrap();
    guard.insert(file, results);
}

impl Drop for LspClient {
    fn drop(&mut self) {
        if let Some(task) = self.reader_task.take() {
            task.abort();
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }
}

/// Concrete SemanticProvider for rust-analyzer.
pub struct RustAnalyzerProvider {
    id: SemanticProviderId,
    workspace_root: PathBuf,
    binary_path: Option<PathBuf>,
    binary_args: Vec<String>,
    readiness_timeout: Duration,
    client: Arc<tokio::sync::Mutex<Option<LspClient>>>,
}

impl RustAnalyzerProvider {
    pub fn new(workspace_root: PathBuf) -> Self {
        let binary_path = find_binary_on_path("rust-analyzer");
        let id = SemanticProviderId::new("rust-analyzer").unwrap();
        Self {
            id,
            workspace_root,
            binary_path,
            binary_args: Vec::new(),
            readiness_timeout: DEFAULT_LSP_READINESS_TIMEOUT,
            client: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    pub fn with_binary(workspace_root: PathBuf, binary: PathBuf) -> Self {
        let id = SemanticProviderId::new("rust-analyzer").unwrap();
        Self {
            id,
            workspace_root,
            binary_path: Some(binary),
            binary_args: Vec::new(),
            readiness_timeout: DEFAULT_LSP_READINESS_TIMEOUT,
            client: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    #[doc(hidden)]
    pub fn with_binary_args(
        workspace_root: PathBuf,
        binary: PathBuf,
        binary_args: Vec<String>,
    ) -> Self {
        Self::with_binary_args_and_readiness_timeout(
            workspace_root,
            binary,
            binary_args,
            DEFAULT_LSP_READINESS_TIMEOUT,
        )
    }

    #[doc(hidden)]
    pub fn with_binary_args_and_readiness_timeout(
        workspace_root: PathBuf,
        binary: PathBuf,
        binary_args: Vec<String>,
        readiness_timeout: Duration,
    ) -> Self {
        let id = SemanticProviderId::new("rust-analyzer").unwrap();
        Self {
            id,
            workspace_root,
            binary_path: Some(binary),
            binary_args,
            readiness_timeout,
            client: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    pub fn is_available(&self) -> bool {
        self.binary_path.is_some()
            && (self.workspace_root.join("Cargo.toml").exists()
                || self.workspace_root.join("rust-project.json").exists())
    }

    async fn get_or_start_client(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<LspClient>>, CoreError> {
        let mut guard = self.client.lock().await;
        if guard.is_none() {
            let Some(bin) = &self.binary_path else {
                return Err(CoreError::ExecutionFailed(
                    "rust-analyzer binary not found".into(),
                ));
            };
            let mut client = LspClient::spawn(
                bin,
                &self.binary_args,
                self.workspace_root.clone(),
                self.id.clone(),
            )
            .await?;
            client
                .initialize_with_readiness_timeout(DEFAULT_LSP_TIMEOUT, self.readiness_timeout)
                .await?;
            *guard = Some(client);
        }
        Ok(guard)
    }

    pub async fn latest_server_status(&self) -> Option<LspServerStatus> {
        let guard = self.client.lock().await;
        guard.as_ref().and_then(LspClient::latest_server_status)
    }
}

impl SemanticProvider for RustAnalyzerProvider {
    fn id(&self) -> SemanticProviderId {
        self.id.clone()
    }

    fn name(&self) -> &str {
        "rust-analyzer"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Live
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            symbol_search: true,
            definition: true,
            references: true,
            structural_search: false,
            diagnostics: true,
            packages: false,
        }
    }

    fn is_available(&self) -> bool {
        self.is_available()
    }

    fn symbol_search<'a>(
        &'a self,
        query: &'a str,
        _limit: usize,
    ) -> BoxFuture<'a, Result<Vec<SymbolRecord>, CoreError>> {
        Box::pin(async move {
            let mut guard = match self.get_or_start_client().await {
                Ok(g) => g,
                Err(e) => return Err(e),
            };
            if let Some(client) = guard.as_mut() {
                match client
                    .workspace_symbol_all(query, DEFAULT_LSP_TIMEOUT)
                    .await
                {
                    Ok(res) => Ok(res),
                    Err(e) => Err(e),
                }
            } else {
                Err(CoreError::ExecutionFailed(
                    "provider=rust-analyzer phase=client.state client_missing".into(),
                ))
            }
        })
    }

    fn symbol_definition<'a>(
        &'a self,
        symbol: &'a str,
        file: Option<&'a str>,
        line: Option<usize>,
        col: Option<usize>,
    ) -> BoxFuture<'a, Result<SemanticLookupResult<SourceLocation>, CoreError>> {
        Box::pin(async move {
            let mut guard = match self.get_or_start_client().await {
                Ok(g) => g,
                Err(e) => return Err(e),
            };
            if let Some(client) = guard.as_mut() {
                if let (Some(f), Some(l), Some(c)) = (file, line, col) {
                    match client.definition(f, l, c, DEFAULT_LSP_TIMEOUT).await {
                        Ok(res) => Ok(res),
                        Err(e) => Err(e),
                    }
                } else {
                    // Open files in src/ so rust-analyzer indexes them immediately
                    let src_dir = self.workspace_root.join("src");
                    if let Ok(entries) = std::fs::read_dir(&src_dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.extension().and_then(|s| s.to_str()) == Some("rs")
                                && let Ok(rel) = path.strip_prefix(&self.workspace_root)
                            {
                                let rel_str = rel.to_string_lossy().replace('\\', "/");
                                let _ = client.did_open_file(&rel_str).await;
                            }
                        }
                    }

                    // Fall back to workspace symbol search if exact line/col not given
                    match client
                        .workspace_symbol_all(symbol, DEFAULT_LSP_TIMEOUT)
                        .await
                    {
                        Ok(syms) => {
                            let exact: Vec<_> =
                                syms.into_iter().filter(|s| s.name == symbol).collect();
                            if exact.is_empty() {
                                Ok(SemanticLookupResult::NotFound)
                            } else if exact.len() == 1 {
                                Ok(SemanticLookupResult::Resolved(exact[0].location.clone()))
                            } else {
                                Ok(SemanticLookupResult::Ambiguous(exact))
                            }
                        }
                        Err(e) => Err(e),
                    }
                }
            } else {
                Err(CoreError::ExecutionFailed(
                    "provider=rust-analyzer phase=client.state client_missing".into(),
                ))
            }
        })
    }

    fn symbol_references<'a>(
        &'a self,
        symbol: &'a str,
        file: Option<&'a str>,
        line: Option<usize>,
        col: Option<usize>,
        limit: usize,
    ) -> BoxFuture<'a, Result<SemanticLookupResult<Vec<ReferenceRecord>>, CoreError>> {
        Box::pin(async move {
            let mut guard = match self.get_or_start_client().await {
                Ok(g) => g,
                Err(e) => return Err(e),
            };
            if let Some(client) = guard.as_mut() {
                if let (Some(f), Some(l), Some(c)) = (file, line, col) {
                    match client.references(f, l, c, limit, DEFAULT_LSP_TIMEOUT).await {
                        Ok(res) => Ok(res),
                        Err(e) => Err(e),
                    }
                } else {
                    // Look up definition first to get exact location
                    match client
                        .workspace_symbol_all(symbol, DEFAULT_LSP_TIMEOUT)
                        .await
                    {
                        Ok(syms) => {
                            let exact: Vec<_> =
                                syms.into_iter().filter(|s| s.name == symbol).collect();
                            if exact.is_empty() {
                                Ok(SemanticLookupResult::NotFound)
                            } else if exact.len() == 1 {
                                let loc = &exact[0].location;
                                match client
                                    .references(
                                        &loc.file,
                                        loc.range.start_line,
                                        loc.range.start_col,
                                        limit,
                                        DEFAULT_LSP_TIMEOUT,
                                    )
                                    .await
                                {
                                    Ok(res) => Ok(res),
                                    Err(e) => Err(e),
                                }
                            } else {
                                Ok(SemanticLookupResult::Ambiguous(exact))
                            }
                        }
                        Err(e) => Err(e),
                    }
                }
            } else {
                Err(CoreError::ExecutionFailed(
                    "provider=rust-analyzer phase=client.state client_missing".into(),
                ))
            }
        })
    }

    fn diagnostics<'a>(&'a self) -> BoxFuture<'a, Result<Vec<SemanticDiagnostic>, CoreError>> {
        Box::pin(async move {
            let guard = match self.get_or_start_client().await {
                Ok(g) => g,
                Err(e) => {
                    return Err(e);
                }
            };
            if let Some(client) = guard.as_ref() {
                Ok(client.get_diagnostics(None))
            } else {
                Err(CoreError::ExecutionFailed(
                    "provider=rust-analyzer phase=client.state client_missing".into(),
                ))
            }
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_rust_analyzer_workspace_symbol_filtering_capability() {
        let supported = json!({
            "capabilities": {
                "experimental": {
                    "workspaceSymbolScopeKindFiltering": true
                }
            }
        });
        let unsupported = json!({ "capabilities": { "experimental": {} } });

        assert!(initialize_supports_workspace_symbol_scope_kind_filtering(
            &supported
        ));
        assert!(!initialize_supports_workspace_symbol_scope_kind_filtering(
            &unsupported
        ));
    }

    #[test]
    fn all_symbol_query_uses_filtered_extension_when_supported() {
        assert_eq!(
            workspace_symbol_all_params("refresh_token", true),
            json!({
                "query": "refresh_token",
                "searchScope": "workspace",
                "searchKind": "allSymbols"
            })
        );
    }

    #[test]
    fn all_symbol_query_uses_documented_hash_fallback_when_extension_is_unavailable() {
        assert_eq!(
            workspace_symbol_all_params("refresh_token", false),
            json!({ "query": "refresh_token#" })
        );
    }

    #[cfg(windows)]
    #[test]
    fn uri_to_relative_file_handles_case_insensitive_windows_drive_letters() {
        let root = PathBuf::from(r"C:\workspace\fixture");
        assert_eq!(
            uri_to_rel_path("file:///c:/workspace/fixture/src/lib.rs", &root),
            "src/lib.rs"
        );
    }

    #[cfg(windows)]
    #[test]
    fn uri_to_relative_file_handles_canonical_windows_workspace_prefix() {
        let root = PathBuf::from(r"\\?\C:\workspace\fixture");
        assert_eq!(
            uri_to_rel_path("file:///c:/workspace/fixture/src/lib.rs", &root),
            "src/lib.rs"
        );
    }
}
