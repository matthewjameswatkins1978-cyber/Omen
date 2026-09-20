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
use tokio::sync::oneshot;

pub const DEFAULT_LSP_TIMEOUT: Duration = Duration::from_millis(5000);

/// Real JSON-RPC stdio LSP client with request correlation, bounded timeouts, and cancellation.
pub struct LspClient {
    child: Option<Child>,
    stdin: tokio::process::ChildStdin,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    workspace_root: PathBuf,
    provider_id: SemanticProviderId,
    generation: SemanticGeneration,
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

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = pending.clone();

        // Background reader reading Content-Length framed JSON-RPC messages
        tokio::spawn(async move {
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
                    let mut body = vec![0u8; len];
                    if reader.read_exact(&mut body).await.is_err() {
                        return;
                    }

                    if let Ok(val) = serde_json::from_slice::<Value>(&body)
                        && let Some(id_val) = val.get("id").and_then(|id| id.as_u64())
                    {
                        let mut map = pending_clone.lock().unwrap();
                        if let Some(tx) = map.remove(&id_val) {
                            let _ = tx.send(val);
                        }
                        // If not in map: safely discarded as a late or canceled response!
                    }
                }
            }
        });

        Ok(Self {
            child: Some(child),
            stdin,
            next_id: AtomicU64::new(1),
            pending,
            workspace_root,
            provider_id,
            generation: SemanticGeneration::new(1, 1),
        })
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
            Ok(Ok(response)) => {
                if let Some(error) = response.get("error") {
                    return Err(CoreError::ExecutionFailed(format!(
                        "LSP error response: {error}"
                    )));
                }
                Ok(response.get("result").cloned().unwrap_or(Value::Null))
            }
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
                    "references": { "dynamicRegistration": false }
                }
            }
        });

        self.send_request("initialize", params, timeout_duration)
            .await?;
        self.send_notification("initialized", json!({})).await?;
        Ok(())
    }

    pub async fn workspace_symbol(
        &mut self,
        query: &str,
        timeout_duration: Duration,
    ) -> Result<Vec<SymbolRecord>, CoreError> {
        let res = self
            .send_request(
                "workspace/symbol",
                json!({ "query": query }),
                timeout_duration,
            )
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
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": col }
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
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": col },
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
        let start_col = start.get("character")?.as_u64()? as usize;
        let end_line = end.get("line")?.as_u64()? as usize;
        let end_col = end.get("character")?.as_u64()? as usize;

        let file = self.uri_to_relative_file(uri_str);

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
        let path_part = uri.strip_prefix("file:///").unwrap_or(uri);
        let path = Path::new(path_part);
        if let Ok(rel) = path.strip_prefix(&self.workspace_root) {
            rel.to_string_lossy().replace('\\', "/")
        } else {
            path_part.replace('\\', "/")
        }
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
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
            client: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    pub fn with_binary(workspace_root: PathBuf, binary: PathBuf) -> Self {
        let id = SemanticProviderId::new("rust-analyzer").unwrap();
        Self {
            id,
            workspace_root,
            binary_path: Some(binary),
            client: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    pub fn is_available(&self) -> bool {
        self.binary_path.is_some()
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
            let mut client =
                LspClient::spawn(bin, &[], self.workspace_root.clone(), self.id.clone()).await?;
            client.initialize(DEFAULT_LSP_TIMEOUT).await?;
            *guard = Some(client);
        }
        Ok(guard)
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
                Err(e) => {
                    tracing::warn!("Failed to start rust-analyzer: {e}");
                    return Ok(Vec::new());
                }
            };
            if let Some(client) = guard.as_mut() {
                match client.workspace_symbol(query, DEFAULT_LSP_TIMEOUT).await {
                    Ok(res) => Ok(res),
                    Err(e) => {
                        tracing::warn!("LSP symbol search error: {e}");
                        Ok(Vec::new())
                    }
                }
            } else {
                Ok(Vec::new())
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
                Err(e) => {
                    tracing::warn!("Failed to start rust-analyzer: {e}");
                    return Ok(SemanticLookupResult::Unsupported);
                }
            };
            if let Some(client) = guard.as_mut() {
                if let (Some(f), Some(l), Some(c)) = (file, line, col) {
                    match client.definition(f, l, c, DEFAULT_LSP_TIMEOUT).await {
                        Ok(res) => Ok(res),
                        Err(e) => {
                            tracing::warn!("LSP definition error: {e}");
                            Ok(SemanticLookupResult::NotFound)
                        }
                    }
                } else {
                    // Fall back to workspace symbol search if exact line/col not given
                    match client.workspace_symbol(symbol, DEFAULT_LSP_TIMEOUT).await {
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
                        Err(e) => {
                            tracing::warn!("LSP workspace symbol error: {e}");
                            Ok(SemanticLookupResult::NotFound)
                        }
                    }
                }
            } else {
                Ok(SemanticLookupResult::Unsupported)
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
                Err(e) => {
                    tracing::warn!("Failed to start rust-analyzer: {e}");
                    return Ok(SemanticLookupResult::Unsupported);
                }
            };
            if let Some(client) = guard.as_mut() {
                if let (Some(f), Some(l), Some(c)) = (file, line, col) {
                    match client.references(f, l, c, limit, DEFAULT_LSP_TIMEOUT).await {
                        Ok(res) => Ok(res),
                        Err(e) => {
                            tracing::warn!("LSP references error: {e}");
                            Ok(SemanticLookupResult::NotFound)
                        }
                    }
                } else {
                    // Look up definition first to get exact location
                    match client.workspace_symbol(symbol, DEFAULT_LSP_TIMEOUT).await {
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
                                    Err(e) => {
                                        tracing::warn!("LSP references error: {e}");
                                        Ok(SemanticLookupResult::NotFound)
                                    }
                                }
                            } else {
                                Ok(SemanticLookupResult::Ambiguous(exact))
                            }
                        }
                        Err(e) => {
                            tracing::warn!("LSP workspace symbol error: {e}");
                            Ok(SemanticLookupResult::NotFound)
                        }
                    }
                }
            } else {
                Ok(SemanticLookupResult::Unsupported)
            }
        })
    }
}
