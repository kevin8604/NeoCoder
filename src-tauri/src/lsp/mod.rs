use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::sync::RwLock;

// ── Public Data Types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LSPSymbol {
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LSPHoverInfo {
    pub contents: String,
}

/// A text edit applied to a file (used by rename / code action / formatting).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LSPTextEdit {
    pub file_path: String,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub new_text: String,
}

/// A quick-fix / refactor action offered by the language server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LSPCodeAction {
    pub title: String,
    pub kind: Option<String>,
    pub is_preferred: Option<bool>,
    /// Flattened workspace edits (may touch multiple files).
    pub edits: Vec<LSPTextEdit>,
}

/// Detect language from file path
pub fn detect_language(file_path: &str) -> String {
    let ext = file_path.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "rs" => "rust".to_string(),
        "ts" | "tsx" => "typescript".to_string(),
        "js" | "jsx" => "javascript".to_string(),
        "py" => "python".to_string(),
        "go" => "go".to_string(),
        "java" => "java".to_string(),
        "rb" => "ruby".to_string(),
        "php" => "php".to_string(),
        "c" | "h" => "c".to_string(),
        "cpp" | "hpp" | "cc" | "cxx" => "cpp".to_string(),
        "cs" => "csharp".to_string(),
        "swift" => "swift".to_string(),
        "kt" | "kts" => "kotlin".to_string(),
        "scala" => "scala".to_string(),
        "sql" => "sql".to_string(),
        "sh" | "bash" => "bash".to_string(),
        "html" => "html".to_string(),
        "css" | "scss" | "less" => "css".to_string(),
        "json" => "json".to_string(),
        "yaml" | "yml" => "yaml".to_string(),
        "toml" => "toml".to_string(),
        "md" | "markdown" => "markdown".to_string(),
        _ => "text".to_string(),
    }
}

// ── Language Server Mapping ────────────────────────────────────────────────

fn language_server_command(language: &str) -> Option<(&'static str, Vec<&'static str>)> {
    match language {
        "rust" => Some(("rust-analyzer", vec![])),
        "typescript" | "javascript" => Some(("typescript-language-server", vec!["--stdio"])),
        "python" => Some(("pylsp", vec![])),
        "go" => Some(("gopls", vec![])),
        "java" => Some(("jdtls", vec![])),
        "cpp" | "c" => Some(("clangd", vec![])),
        "ruby" => Some(("solargraph", vec!["stdio"])),
        "php" => Some(("intelephense", vec!["--stdio"])),
        "csharp" => Some(("omnisharp", vec!["--stdio"])),
        "kotlin" => Some(("kotlin-language-server", vec![])),
        _ => None,
    }
}

// ── Helper: file path to LSP URI ───────────────────────────────────────────

fn path_to_uri(file_path: &str) -> String {
    // Normalize backslashes to forward slashes
    let normalized = file_path.replace('\\', "/");
    if normalized.starts_with('/') {
        format!("file://{}", normalized)
    } else {
        format!("file:///{}", normalized)
    }
}

// ── LSP Client ─────────────────────────────────────────────────────────────

type PendingMap =
    Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>>>>;

pub struct LspClient {
    child: Option<Child>,
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
    pending: PendingMap,
    next_id: AtomicU64,
}

impl LspClient {
    /// Start a new LSP client for the given language in the given workspace root.
    pub async fn start(language: &str, root_uri: &str) -> Result<Self, String> {
        let (cmd, args) = language_server_command(language)
            .ok_or_else(|| format!("No LSP server configured for language: {}", language))?;

        let mut child = Command::new(cmd)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to start LSP server '{}': {}", cmd, e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or("Failed to open stdin for LSP server")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("Failed to open stdout for LSP server")?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = pending.clone();

        // Spawn background reader task
        tokio::spawn(async move {
            Self::reader_task(stdout, pending_clone).await;
        });

        let client = LspClient {
            child: Some(child),
            stdin: Arc::new(Mutex::new(stdin)),
            pending,
            next_id: AtomicU64::new(1),
        };

        // Send initialize request
        let init_params = serde_json::json!({
            "processId": null,
            "clientInfo": {
                "name": "NeoCoder",
                "version": "0.1.0"
            },
            "capabilities": {
                "textDocument": {
                    "documentSymbol": { "dynamicRegistration": false },
                    "hover": { "dynamicRegistration": false },
                    "completion": { "dynamicRegistration": false },
                    "definition": { "dynamicRegistration": false },
                    "references": { "dynamicRegistration": false },
                    "rename": { "dynamicRegistration": false },
                    "codeAction": { "dynamicRegistration": false },
                    "formatting": { "dynamicRegistration": false }
                },
                "workspace": {
                    "didChangeConfiguration": { "dynamicRegistration": false }
                }
            },
            "rootUri": root_uri,
            "workspaceFolders": [
                { "uri": root_uri, "name": "workspace" }
            ]
        });

        let _init_result = client.send_request("initialize", init_params).await?;

        // Send initialized notification
        client
            .send_notification("initialized", serde_json::json!({}))
            .await?;

        log::info!("LSP client started for {} with {}", language, cmd);
        Ok(client)
    }

    // ── Background reader ──────────────────────────────────────────────────

    async fn reader_task(mut stdout: tokio::process::ChildStdout, pending: PendingMap) {
        let mut reader = BufReader::new(&mut stdout);

        loop {
            // Parse Content-Length header
            let content_length = match Self::read_content_length(&mut reader).await {
                Some(len) => len,
                None => break, // EOF or error
            };

            // Read body bytes
            let mut body = vec![0u8; content_length as usize];
            if reader.read_exact(&mut body).await.is_err() {
                log::error!("LSP: failed to read response body");
                break;
            }

            // Parse JSON
            let value: serde_json::Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("LSP: failed to parse JSON: {}", e);
                    continue;
                }
            };

            // Dispatch to pending request if it has an id
            if let Some(id_val) = value.get("id")
                && let Some(id) = id_val.as_u64()
            {
                let mut map = pending.lock().await;
                if let Some(sender) = map.remove(&id) {
                    // Check for error response
                    if let Some(error) = value.get("error") {
                        let msg = error
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("Unknown LSP error");
                        let _ = sender.send(Err(msg.to_string()));
                    } else if value.get("result").is_some() {
                        let _ = sender.send(Ok(value));
                    }
                }
            }
        }

        // Reader task ended – cancel all pending requests
        let mut map = pending.lock().await;
        for (_, sender) in map.drain() {
            let _ = sender.send(Err("LSP server disconnected".to_string()));
        }
    }

    async fn read_content_length(
        reader: &mut BufReader<&mut tokio::process::ChildStdout>,
    ) -> Option<u32> {
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line).await {
                Ok(0) => return None,
                Ok(_) => {}
                Err(e) => {
                    log::error!("LSP: read error: {}", e);
                    return None;
                }
            }

            if let Some(len_str) = line.strip_prefix("Content-Length: ")
                && let Ok(len) = len_str.trim().parse::<u32>()
            {
                // Consume until empty line (end of headers)
                loop {
                    let mut empty = String::new();
                    match reader.read_line(&mut empty).await {
                        Ok(0) => return None,
                        Ok(_) => {
                            if empty == "\r\n" || empty == "\n" {
                                return Some(len);
                            }
                        }
                        Err(_) => return None,
                    }
                }
            }
        }
    }

    // ── JSON-RPC helpers ───────────────────────────────────────────────────

    async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        self.write_message(&request).await?;

        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(format!("LSP response channel closed for '{}'", method)),
            Err(_) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&id);
                Err(format!("LSP request '{}' timed out", method))
            }
        }
    }

    async fn send_notification(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), String> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        self.write_message(&notification).await
    }

    async fn write_message(&self, message: &serde_json::Value) -> Result<(), String> {
        let body = serde_json::to_string(message).map_err(|e| e.to_string())?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());

        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(header.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin
            .write_all(body.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── Public API ────────────────────────────────────────────────────────

    /// Get document symbols for a file.
    pub async fn get_symbols(&self, file_path: &str) -> Result<Vec<LSPSymbol>, String> {
        let uri = path_to_uri(file_path);
        let result = self
            .send_request(
                "textDocument/documentSymbol",
                serde_json::json!({
                    "textDocument": { "uri": uri }
                }),
            )
            .await?;

        let symbols = parse_symbol_response(&result, file_path)?;
        Ok(symbols)
    }

    /// Get hover information at a position.
    pub async fn get_hover(
        &self,
        file_path: &str,
        line: u32,
        column: u32,
    ) -> Result<Option<LSPHoverInfo>, String> {
        let uri = path_to_uri(file_path);
        let result = self
            .send_request(
                "textDocument/hover",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": column }
                }),
            )
            .await?;

        Ok(parse_hover_response(&result))
    }

    /// Notify LSP that a document was opened.
    pub async fn did_open(
        &self,
        file_path: &str,
        language: &str,
        text: &str,
    ) -> Result<(), String> {
        let uri = path_to_uri(file_path);
        self.send_notification(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language,
                    "version": 1,
                    "text": text
                }
            }),
        )
        .await
    }

    /// Notify LSP that a document was changed.
    pub async fn did_change(
        &self,
        file_path: &str,
        text: &str,
        version: i32,
    ) -> Result<(), String> {
        let uri = path_to_uri(file_path);
        self.send_notification(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "version": version
                },
                "contentChanges": [{
                    "text": text
                }]
            }),
        )
        .await
    }

    /// Notify LSP that a document was closed.
    pub async fn did_close(&self, file_path: &str) -> Result<(), String> {
        let uri = path_to_uri(file_path);
        self.send_notification(
            "textDocument/didClose",
            serde_json::json!({
                "textDocument": { "uri": uri }
            }),
        )
        .await
    }

    /// Rename a symbol at the given position. Returns all edits across files.
    pub async fn rename_symbol(
        &self,
        file_path: &str,
        line: u32,
        column: u32,
        new_name: &str,
    ) -> Result<Vec<LSPTextEdit>, String> {
        let uri = path_to_uri(file_path);
        let result = self
            .send_request(
                "textDocument/rename",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": column },
                    "newName": new_name
                }),
            )
            .await?;
        parse_workspace_edit(&result)
    }

    /// Request code actions (quick fixes / refactors) at a position.
    pub async fn code_action(
        &self,
        file_path: &str,
        line: u32,
        column: u32,
        diagnostics: &[serde_json::Value],
    ) -> Result<Vec<LSPCodeAction>, String> {
        let uri = path_to_uri(file_path);
        let result = self
            .send_request(
                "textDocument/codeAction",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "range": {
                        "start": { "line": line, "character": column },
                        "end": { "line": line, "character": column }
                    },
                    "context": { "diagnostics": diagnostics }
                }),
            )
            .await?;
        parse_code_actions(&result)
    }

    /// Format the whole document. Returns text edits to apply.
    pub async fn format_document(&self, file_path: &str) -> Result<Vec<LSPTextEdit>, String> {
        let uri = path_to_uri(file_path);
        let result = self
            .send_request(
                "textDocument/formatting",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "options": { "tabSize": 4, "insertSpaces": true }
                }),
            )
            .await?;
        parse_text_edits(&result, file_path)
    }

    /// Shutdown and exit the LSP server.
    pub async fn shutdown(mut self) -> Result<(), String> {
        let _ = self.send_request("shutdown", serde_json::json!({})).await;
        let _ = self.send_notification("exit", serde_json::json!({})).await;

        // Wait for process to exit
        if let Some(ref mut child) = self.child {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
        }
        Ok(())
    }
}

// ── Response Parsers ───────────────────────────────────────────────────────

fn parse_symbol_response(
    response: &serde_json::Value,
    file_path: &str,
) -> Result<Vec<LSPSymbol>, String> {
    let mut symbols = Vec::new();

    let items = match response.get("result") {
        Some(serde_json::Value::Array(arr)) => arr,
        Some(serde_json::Value::Null) => return Ok(vec![]),
        _ => return Err("Unexpected LSP symbol response format".to_string()),
    };

    for item in items {
        // LSP can return either DocumentSymbol[] or SymbolInformation[]
        // Handle both formats
        if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
            let kind = item.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
            let kind_name = symbol_kind_name(kind);

            // DocumentSymbol format (nested)
            if let Some(selection_range) = item.get("selectionRange") {
                let start_line = selection_range
                    .pointer("/start/line")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let start_col = selection_range
                    .pointer("/start/character")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let end_line = selection_range
                    .pointer("/end/line")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let end_col = selection_range
                    .pointer("/end/character")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;

                let detail = item
                    .get("detail")
                    .and_then(|d| d.as_str())
                    .map(|s| s.to_string());

                symbols.push(LSPSymbol {
                    name: name.to_string(),
                    kind: kind_name,
                    file_path: file_path.to_string(),
                    start_line,
                    start_column: start_col,
                    end_line,
                    end_column: end_col,
                    detail,
                });

                // Parse children recursively
                if let Some(children) = item.get("children").and_then(|c| c.as_array()) {
                    for child in children {
                        if let Some(child_name) = child.get("name").and_then(|n| n.as_str()) {
                            let child_kind =
                                child.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
                            let child_range = child.get("selectionRange");
                            if let Some(cr) = child_range {
                                symbols.push(LSPSymbol {
                                    name: child_name.to_string(),
                                    kind: symbol_kind_name(child_kind),
                                    file_path: file_path.to_string(),
                                    start_line: cr
                                        .pointer("/start/line")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                        as u32,
                                    start_column: cr
                                        .pointer("/start/character")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                        as u32,
                                    end_line: cr
                                        .pointer("/end/line")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                        as u32,
                                    end_column: cr
                                        .pointer("/end/character")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                        as u32,
                                    detail: child
                                        .get("detail")
                                        .and_then(|d| d.as_str())
                                        .map(|s| s.to_string()),
                                });
                            }
                        }
                    }
                }
            }
            // SymbolInformation format (flat)
            else if let Some(location) = item.get("location")
                && let Some(range) = location.get("range")
            {
                let start_line = range
                    .pointer("/start/line")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let start_col = range
                    .pointer("/start/character")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let end_line = range
                    .pointer("/end/line")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let end_col = range
                    .pointer("/end/character")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;

                symbols.push(LSPSymbol {
                    name: name.to_string(),
                    kind: kind_name,
                    file_path: file_path.to_string(),
                    start_line,
                    start_column: start_col,
                    end_line,
                    end_column: end_col,
                    detail: item
                        .get("detail")
                        .and_then(|d| d.as_str())
                        .map(|s| s.to_string()),
                });
            }
        }
    }

    Ok(symbols)
}

fn parse_hover_response(response: &serde_json::Value) -> Option<LSPHoverInfo> {
    let result = response.get("result")?;

    // Hover result can be { contents: MarkupContent | MarkedString | MarkedString[] }
    let contents = result.get("contents")?;

    let text = match contents {
        // MarkupContent: { kind: "markdown", value: "..." }
        serde_json::Value::Object(_) => {
            contents
                .get("value")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    // Could be MarkedString: { language: "rust", value: "..." }
                    contents.get("language").and_then(|_| {
                        contents
                            .get("value")
                            .and_then(|v| v.as_str().map(|s| s.to_string()))
                    })
                })
        }
        // MarkedString[]: array of strings or objects
        serde_json::Value::Array(arr) => {
            let mut combined = String::new();
            for item in arr {
                match item {
                    serde_json::Value::String(s) => {
                        combined.push_str(s);
                        combined.push('\n');
                    }
                    serde_json::Value::Object(_) => {
                        if let Some(value) = item.get("value").and_then(|v| v.as_str()) {
                            combined.push_str(value);
                            combined.push('\n');
                        }
                    }
                    _ => {}
                }
            }
            if combined.is_empty() {
                None
            } else {
                Some(combined)
            }
        }
        // Plain string
        serde_json::Value::String(s) => Some(s.clone()),
        _ => None,
    }?;

    Some(LSPHoverInfo { contents: text })
}

// ── Parse rename / code action / formatting responses ──────────────────────

/// Convert a `file://` URI back to a filesystem path.
fn uri_to_path(uri: &str) -> String {
    let stripped = uri.strip_prefix("file://").unwrap_or(uri);
    // Windows: file:///C:/path -> C:/path
    let stripped = stripped.strip_prefix('/').unwrap_or(stripped);
    stripped.replace('/', "\\")
}

/// Parse a TextEdit array into LSPTextEdit entries.
fn parse_text_edits(
    response: &serde_json::Value,
    file_path: &str,
) -> Result<Vec<LSPTextEdit>, String> {
    let mut edits = Vec::new();
    let result = response.get("result");
    let items = match result {
        Some(serde_json::Value::Array(arr)) => arr,
        Some(serde_json::Value::Null) => return Ok(vec![]),
        _ => return Err("Unexpected LSP edit response format".to_string()),
    };

    for item in items {
        if let Some((sl, sc, el, ec, new_text)) = parse_single_edit(item) {
            edits.push(LSPTextEdit {
                file_path: file_path.to_string(),
                start_line: sl,
                start_column: sc,
                end_line: el,
                end_column: ec,
                new_text,
            });
        }
    }
    Ok(edits)
}

/// Extract (start_line, start_col, end_line, end_col, new_text) from a TextEdit.
fn parse_single_edit(item: &serde_json::Value) -> Option<(u32, u32, u32, u32, String)> {
    let range = item.get("range")?;
    let start_line = range.pointer("/start/line")?.as_u64()? as u32;
    let start_col = range.pointer("/start/character")?.as_u64()? as u32;
    let end_line = range.pointer("/end/line")?.as_u64()? as u32;
    let end_col = range.pointer("/end/character")?.as_u64()? as u32;
    let new_text = item
        .get("newText")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some((start_line, start_col, end_line, end_col, new_text))
}

/// Parse a WorkspaceEdit (rename result) — handles `changes` and `documentChanges`.
fn parse_workspace_edit(response: &serde_json::Value) -> Result<Vec<LSPTextEdit>, String> {
    let mut edits = Vec::new();
    let result = response.get("result");

    // `changes`: { uri: [TextEdit, ...] }
    if let Some(changes) = result
        .and_then(|r| r.get("changes"))
        .and_then(|c| c.as_object())
    {
        for (uri, edit_arr) in changes {
            let file_path = uri_to_path(uri);
            if let Some(arr) = edit_arr.as_array() {
                for item in arr {
                    if let Some((sl, sc, el, ec, text)) = parse_single_edit(item) {
                        edits.push(LSPTextEdit {
                            file_path: file_path.clone(),
                            start_line: sl,
                            start_column: sc,
                            end_line: el,
                            end_column: ec,
                            new_text: text,
                        });
                    }
                }
            }
        }
    }

    // `documentChanges`: [{ textDocument: { uri }, edits: [TextEdit, ...] }]
    if let Some(doc_changes) = result
        .and_then(|r| r.get("documentChanges"))
        .and_then(|c| c.as_array())
    {
        for change in doc_changes {
            let uri = change
                .pointer("/textDocument/uri")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let file_path = uri_to_path(uri);
            if let Some(edit_arr) = change.get("edits").and_then(|e| e.as_array()) {
                for item in edit_arr {
                    if let Some((sl, sc, el, ec, text)) = parse_single_edit(item) {
                        edits.push(LSPTextEdit {
                            file_path: file_path.clone(),
                            start_line: sl,
                            start_column: sc,
                            end_line: el,
                            end_column: ec,
                            new_text: text,
                        });
                    }
                }
            }
        }
    }

    if edits.is_empty() && result.is_some() && result.unwrap().is_null() {
        return Ok(vec![]);
    }
    Ok(edits)
}

/// Parse a codeAction response into LSPCodeAction entries (only those with edits).
fn parse_code_actions(response: &serde_json::Value) -> Result<Vec<LSPCodeAction>, String> {
    let mut actions = Vec::new();
    let items = match response.get("result") {
        Some(serde_json::Value::Array(arr)) => arr,
        Some(serde_json::Value::Null) => return Ok(vec![]),
        _ => return Err("Unexpected LSP code action response format".to_string()),
    };

    for item in items {
        let title = item
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        if title.is_empty() {
            continue;
        }
        let kind = item
            .get("kind")
            .and_then(|k| k.as_str())
            .map(|s| s.to_string());
        let is_preferred = item.get("isPreferred").and_then(|p| p.as_bool());

        let mut edits = Vec::new();
        if let Some(edit) = item.get("edit") {
            let workspace = serde_json::json!({ "result": edit });
            edits = parse_workspace_edit(&workspace)?;
        }
        actions.push(LSPCodeAction {
            title,
            kind,
            is_preferred,
            edits,
        });
    }
    Ok(actions)
}

fn symbol_kind_name(kind: u64) -> String {
    match kind {
        1 => "File".into(),
        2 => "Module".into(),
        3 => "Namespace".into(),
        4 => "Package".into(),
        5 => "Class".into(),
        6 => "Method".into(),
        7 => "Property".into(),
        8 => "Field".into(),
        9 => "Constructor".into(),
        10 => "Enum".into(),
        11 => "Interface".into(),
        12 => "Function".into(),
        13 => "Variable".into(),
        14 => "Constant".into(),
        15 => "String".into(),
        16 => "Number".into(),
        17 => "Boolean".into(),
        18 => "Array".into(),
        19 => "Object".into(),
        20 => "Key".into(),
        21 => "Null".into(),
        22 => "EnumMember".into(),
        23 => "Struct".into(),
        24 => "Event".into(),
        25 => "Operator".into(),
        26 => "TypeParameter".into(),
        _ => "Unknown".into(),
    }
}

// ── LSP Manager ────────────────────────────────────────────────────────────

/// Manages multiple LSP clients, keyed by language.
pub struct LspManager {
    clients: RwLock<HashMap<String, LspClient>>,
}

impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LspManager {
    pub fn new() -> Self {
        LspManager {
            clients: RwLock::new(HashMap::new()),
        }
    }

    /// Get or start an LSP client for the given language/root.
    pub async fn get_or_start(&self, language: &str, root_uri: &str) -> Result<(), String> {
        let mut clients = self.clients.write().await;
        if !clients.contains_key(language) {
            let client = LspClient::start(language, root_uri).await?;
            clients.insert(language.to_string(), client);
        }
        Ok(())
    }

    /// Get a reference to an existing client.
    pub async fn get_client(
        &self,
        language: &str,
    ) -> Option<tokio::sync::RwLockReadGuard<'_, HashMap<String, LspClient>>> {
        let clients = self.clients.read().await;
        if clients.contains_key(language) {
            Some(clients) // Return the guard so the caller can access
        } else {
            None
        }
    }

    /// Shutdown and remove all LSP clients.
    pub async fn shutdown_all(&self) {
        let languages: Vec<String> = {
            let clients = self.clients.read().await;
            clients.keys().cloned().collect()
        };
        for lang in languages {
            if let Some(client) = self.clients.write().await.remove(&lang) {
                log::info!("Shutting down LSP client for {}", lang);
                let _ = client.shutdown().await;
            }
        }
    }

    /// Check if a client exists for the given language.
    pub async fn has_client(&self, language: &str) -> bool {
        self.clients.read().await.contains_key(language)
    }

    /// Get symbols from the LSP client for the given language.
    pub async fn get_symbols(
        &self,
        language: &str,
        file_path: &str,
    ) -> Result<Vec<LSPSymbol>, String> {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(language) {
            client.get_symbols(file_path).await
        } else {
            Err(format!("No LSP client active for {}", language))
        }
    }

    /// Get hover info from the LSP client.
    pub async fn get_hover(
        &self,
        language: &str,
        file_path: &str,
        line: u32,
        column: u32,
    ) -> Result<Option<LSPHoverInfo>, String> {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(language) {
            client.get_hover(file_path, line, column).await
        } else {
            Err(format!("No LSP client active for {}", language))
        }
    }

    /// Notify LSP that a document was opened.
    pub async fn did_open(
        &self,
        language: &str,
        file_path: &str,
        file_text: &str,
    ) -> Result<(), String> {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(language) {
            client.did_open(file_path, language, file_text).await
        } else {
            Err(format!("No LSP client active for {}", language))
        }
    }

    /// Notify LSP that a document was changed.
    pub async fn did_change(
        &self,
        language: &str,
        file_path: &str,
        text: &str,
        version: i32,
    ) -> Result<(), String> {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(language) {
            client.did_change(file_path, text, version).await
        } else {
            Err(format!("No LSP client active for {}", language))
        }
    }

    /// Notify LSP that a document was closed.
    pub async fn did_close(&self, language: &str, file_path: &str) -> Result<(), String> {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(language) {
            client.did_close(file_path).await
        } else {
            Err(format!("No LSP client active for {}", language))
        }
    }

    /// Rename a symbol via the LSP client.
    pub async fn rename_symbol(
        &self,
        language: &str,
        file_path: &str,
        line: u32,
        column: u32,
        new_name: &str,
    ) -> Result<Vec<LSPTextEdit>, String> {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(language) {
            client
                .rename_symbol(file_path, line, column, new_name)
                .await
        } else {
            Err(format!("No LSP client active for {}", language))
        }
    }

    /// Request code actions via the LSP client.
    pub async fn code_action(
        &self,
        language: &str,
        file_path: &str,
        line: u32,
        column: u32,
        diagnostics: &[serde_json::Value],
    ) -> Result<Vec<LSPCodeAction>, String> {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(language) {
            client
                .code_action(file_path, line, column, diagnostics)
                .await
        } else {
            Err(format!("No LSP client active for {}", language))
        }
    }

    /// Format a document via the LSP client.
    pub async fn format_document(
        &self,
        language: &str,
        file_path: &str,
    ) -> Result<Vec<LSPTextEdit>, String> {
        let clients = self.clients.read().await;
        if let Some(client) = clients.get(language) {
            client.format_document(file_path).await
        } else {
            Err(format!("No LSP client active for {}", language))
        }
    }
}
