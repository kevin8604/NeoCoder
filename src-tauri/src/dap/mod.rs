//! DAP (Debug Adapter Protocol) backend — drives an external debug adapter
//! over stdio so the Agent and the DebugPanel can inspect a live program.
//!
//! The wire format shares the `Content-Length: N\r\n\r\n{json}` framing used by
//! LSP, but the message shape is DAP:
//! - request  : `{ seq, type:"request",  command, arguments }`
//! - response : `{ seq, type:"response", request_seq, success, command, body }`
//! - event    : `{ seq, type:"event",    event, body }`
//!
//! A request is correlated to its response via `request_seq` (equal to the
//! request's `seq`); asynchronous notifications (stopped/continued/output/
//! terminated) arrive as events and are fanned out over a broadcast channel so
//! callers can `await` the next `stopped` after a continue/step.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, RwLock, broadcast};

/// Timeout for a single DAP request round-trip.
const REQUEST_TIMEOUT_SECS: u64 = 30;

// ── Public data types (serialized to the frontend / agent tool) ─────────────

/// A resolved breakpoint in a debug session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DapBreakpoint {
    pub id: i64,
    pub verified: bool,
    pub line: u32,
    pub path: String,
}

/// One frame of a call stack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DapStackFrame {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub line: u32,
    pub column: u32,
}

/// A scope (Locals / Globals / …) within a stack frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DapScope {
    pub name: String,
    pub variables_reference: i64,
    pub expensive: bool,
}

/// A variable entry; `children` is populated on demand via `get_variables`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DapVariable {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub r#type: String,
    pub variables_reference: i64,
    pub named_variables: i64,
}

/// A snapshot of runtime state at a stop location: stack + locals + stop reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub stopped: bool,
    pub reason: String,
    pub description: Option<String>,
    pub thread_id: i64,
    pub top_frame: Option<DapStackFrame>,
    pub frames: Vec<DapStackFrame>,
    pub locals: Vec<DapVariable>,
    pub output: Vec<String>,
}

/// High-level status of a debug session, returned to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugSessionInfo {
    pub session_id: String,
    pub running: bool,
    pub adapter: String,
    pub program: String,
    pub stopped: bool,
    pub stop_reason: Option<String>,
    pub breakpoints: Vec<DapBreakpoint>,
}

// ── Pure message-shaping helpers (unit-testable) ────────────────────────────

/// Encode a DAP message with Content-Length framing.
pub fn encode_message(message: &Value) -> Vec<u8> {
    let body = serde_json::to_string(message).unwrap_or_default();
    let mut out = Vec::with_capacity(body.len() + 32);
    out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
    out.extend_from_slice(body.as_bytes());
    out
}

/// Build a DAP request JSON with an explicit sequence number.
pub fn build_request(seq: u64, command: &str, arguments: Value) -> Value {
    serde_json::json!({
        "seq": seq,
        "type": "request",
        "command": command,
        "arguments": arguments
    })
}

/// Classification of a decoded inbound DAP message.
#[derive(Debug, Clone, PartialEq)]
pub enum InboundMessage {
    /// Response to one of our requests.
    Response {
        request_seq: u64,
        success: bool,
        message: Option<String>,
        body: Value,
    },
    /// Async event (stopped / continued / output / terminated / …).
    Event { event: String, body: Value },
    /// A request initiated by the adapter (reverse request).
    ReverseRequest {
        seq: u64,
        command: String,
        arguments: Value,
    },
}

/// Parse a decoded inbound DAP message into its logical kind.
pub fn classify_message(value: &Value) -> Option<InboundMessage> {
    match value.get("type").and_then(|t| t.as_str()) {
        Some("response") => Some(InboundMessage::Response {
            request_seq: value
                .get("request_seq")
                .and_then(|s| s.as_u64())
                .unwrap_or(0),
            success: value
                .get("success")
                .and_then(|s| s.as_bool())
                .unwrap_or(false),
            message: value
                .get("message")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string()),
            body: value
                .get("body")
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default())),
        }),
        Some("event") => Some(InboundMessage::Event {
            event: value.get("event").and_then(|e| e.as_str())?.to_string(),
            body: value
                .get("body")
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default())),
        }),
        Some("request") => Some(InboundMessage::ReverseRequest {
            seq: value.get("seq").and_then(|s| s.as_u64()).unwrap_or(0),
            command: value.get("command").and_then(|c| c.as_str())?.to_string(),
            arguments: value
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default())),
        }),
        _ => None,
    }
}

/// Convert a `setBreakpoints` response body into resolved breakpoints.
pub fn parse_breakpoints(body: &Value, path: &str, requested: &[u32]) -> Vec<DapBreakpoint> {
    let mut out = Vec::new();
    if let Some(arr) = body.get("breakpoints").and_then(|b| b.as_array()) {
        for (i, bp) in arr.iter().enumerate() {
            let line = bp
                .get("line")
                .and_then(|l| l.as_u64())
                .unwrap_or(requested.get(i).copied().unwrap_or(0) as u64)
                as u32;
            out.push(DapBreakpoint {
                id: bp.get("id").and_then(|x| x.as_i64()).unwrap_or(i as i64),
                verified: bp
                    .get("verified")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                line,
                path: path.to_string(),
            });
        }
    }
    out
}

/// Convert a `stackTrace` response body into frames.
pub fn parse_frames(body: &Value) -> Vec<DapStackFrame> {
    body.get("stackFrames")
        .and_then(|f| f.as_array())
        .map(|arr| {
            arr.iter()
                .map(|f| DapStackFrame {
                    id: f.get("id").and_then(|x| x.as_i64()).unwrap_or(0),
                    name: f
                        .get("name")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    path: f
                        .get("source")
                        .and_then(|s| s.get("path"))
                        .and_then(|p| p.as_str())
                        .unwrap_or("")
                        .to_string(),
                    line: f.get("line").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                    column: f.get("column").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Convert a `variables` response body into variables.
pub fn parse_variables(body: &Value) -> Vec<DapVariable> {
    body.get("variables")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|v| DapVariable {
                    name: v
                        .get("name")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    value: v
                        .get("value")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    r#type: v
                        .get("type")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    variables_reference: v
                        .get("variablesReference")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0),
                    named_variables: v
                        .get("namedVariables")
                        .and_then(|x| x.as_i64())
                        .unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Render a runtime snapshot as a compact, LLM-friendly text report.
pub fn render_runtime_snapshot(snap: &RuntimeSnapshot) -> String {
    let mut out = String::new();
    if !snap.stopped {
        out.push_str("Program is not stopped (no breakpoint hit within the wait window).\n");
        if !snap.output.is_empty() {
            out.push_str("\n## Program output\n");
            for line in snap.output.iter().take(40) {
                out.push_str(line);
                out.push('\n');
            }
        }
        return out;
    }
    out.push_str(&format!(
        "Stopped at: {} (reason: {})\n",
        snap.top_frame
            .as_ref()
            .map(|f| format!("{} — {}:{}", f.name, shorten_path(&f.path), f.line))
            .unwrap_or_else(|| "(unknown location)".to_string()),
        snap.reason
    ));

    if !snap.frames.is_empty() {
        out.push_str("\n## Call stack\n");
        for (i, f) in snap.frames.iter().take(12).enumerate() {
            out.push_str(&format!(
                "#{}  {}  ({}:{})\n",
                i,
                f.name,
                shorten_path(&f.path),
                f.line
            ));
        }
    }

    if !snap.locals.is_empty() {
        out.push_str("\n## Local variables\n");
        for v in snap.locals.iter().take(40) {
            let ty = if v.r#type.is_empty() {
                String::new()
            } else {
                format!(" : {}", v.r#type)
            };
            out.push_str(&format!("  {}{} = {}\n", v.name, ty, v.value));
        }
    }

    if !snap.output.is_empty() {
        out.push_str("\n## Program output\n");
        for line in snap.output.iter().take(40) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn shorten_path(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

// ── Debug adapter launch mapping ────────────────────────────────────────────

/// Resolve a debug adapter executable + args for a language, or from an
/// explicit override. Returns `(command, args, dap_type)`.
pub fn adapter_for_language(
    language: &str,
    override_cmd: Option<&str>,
) -> Option<(String, Vec<String>, String)> {
    if let Some(cmd) = override_cmd.filter(|c| !c.is_empty()) {
        return Some((cmd.to_string(), vec![], infer_dap_type(language)));
    }
    match language {
        "rust" => Some(("codelldb".to_string(), vec![], "lldb".to_string())),
        "c" | "cpp" => Some(("lldb-dap".to_string(), vec![], "lldb".to_string())),
        "swift" => Some(("lldb-dap".to_string(), vec![], "lldb".to_string())),
        "python" => Some((
            "python".to_string(),
            vec!["-m".to_string(), "debugpy.adapter".to_string()],
            "python".to_string(),
        )),
        _ => None,
    }
}

fn infer_dap_type(language: &str) -> String {
    match language {
        "python" => "python".to_string(),
        "go" => "go".to_string(),
        "c" | "cpp" | "rust" | "swift" => "lldb".to_string(),
        other => other.to_string(),
    }
}

// ── DAP Client ──────────────────────────────────────────────────────────────

type PendingMap = Arc<Mutex<HashMap<u64, tokio::sync::oneshot::Sender<Result<Value, String>>>>>;

/// A live connection to one debug-adapter subprocess.
pub struct DapClient {
    child: Arc<Mutex<Option<Child>>>,
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
    pending: PendingMap,
    next_seq: AtomicU64,
    events_tx: broadcast::Sender<InboundMessage>,
    adapter_cmd: String,
}

impl DapClient {
    /// Spawn the debug adapter process over piped stdio and run the
    /// `initialize` handshake.
    pub async fn start(command: &str, args: &[String]) -> Result<Self, String> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                format!(
                    "Failed to start debug adapter '{}' (is it installed and on PATH?): {}",
                    command, e
                )
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or("Failed to open debug adapter stdin")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("Failed to open debug adapter stdout")?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, _rx) = broadcast::channel(128);
        let reader_pending = pending.clone();
        let reader_events = events_tx.clone();

        tokio::spawn(async move {
            Self::reader_task(stdout, reader_pending, reader_events).await;
        });

        let client = DapClient {
            child: Arc::new(Mutex::new(Some(child))),
            stdin: Arc::new(Mutex::new(stdin)),
            pending,
            next_seq: AtomicU64::new(1),
            events_tx: events_tx.clone(),
            adapter_cmd: command.to_string(),
        };

        // Subscribe BEFORE sending `initialize` so the adapter's `initialized`
        // event is delivered to us (broadcast loses messages with no subscriber).
        let mut rx = events_tx.subscribe();

        // initialize handshake
        client
            .send_request(
                "initialize",
                serde_json::json!({
                    "clientID": "NeoCoder",
                    "adapterID": "dap",
                    "linesStartAt1": true,
                    "columnsStartAt1": true,
                    "pathFormat": "path",
                    "supportsVariableType": true,
                    "supportsRunToTermination": true
                }),
            )
            .await?;

        // Wait for the `initialized` event before we are allowed to send any
        // configuration or launch requests.
        wait_for_event(&mut rx, "initialized", std::time::Duration::from_secs(15)).await?;

        Ok(client)
    }

    // ── Background reader ────────────────────────────────────────────────────

    async fn reader_task(
        mut stdout: tokio::process::ChildStdout,
        pending: PendingMap,
        events_tx: broadcast::Sender<InboundMessage>,
    ) {
        let mut reader = BufReader::new(&mut stdout);
        loop {
            let len = match read_content_length(&mut reader).await {
                Some(l) => l,
                None => break,
            };
            let mut body = vec![0u8; len as usize];
            if reader.read_exact(&mut body).await.is_err() {
                log::error!("[DAP] failed to read message body");
                break;
            }
            let value: Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    log::warn!("[DAP] failed to parse JSON: {}", e);
                    continue;
                }
            };
            match classify_message(&value) {
                Some(InboundMessage::Response {
                    request_seq,
                    success,
                    message,
                    body,
                }) => {
                    let mut map = pending.lock().await;
                    if let Some(tx) = map.remove(&request_seq) {
                        let result = if success {
                            Ok(body)
                        } else {
                            Err(message.unwrap_or_else(|| "DAP request failed".to_string()))
                        };
                        let _ = tx.send(result);
                    }
                }
                Some(evt) => {
                    let _ = events_tx.send(evt);
                }
                None => {}
            }
        }

        let mut map = pending.lock().await;
        for (_, tx) in map.drain() {
            let _ = tx.send(Err("Debug adapter disconnected".to_string()));
        }
    }

    // ── JSON round-trip ──────────────────────────────────────────────────────

    pub async fn send_request(&self, command: &str, arguments: Value) -> Result<Value, String> {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.lock().await.insert(seq, tx);

        let request = build_request(seq, command, arguments);
        self.write_message(&request).await?;

        match tokio::time::timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS), rx).await {
            Ok(Ok(res)) => res,
            Ok(Err(_)) => Err(format!("DAP response channel closed for '{}'", command)),
            Err(_) => {
                self.pending.lock().await.remove(&seq);
                Err(format!("DAP request '{}' timed out", command))
            }
        }
    }

    async fn write_message(&self, message: &Value) -> Result<(), String> {
        let bytes = encode_message(message);
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&bytes).await.map_err(|e| e.to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Subscribe to the event stream.
    pub fn subscribe_events(&self) -> broadcast::Receiver<InboundMessage> {
        self.events_tx.subscribe()
    }

    pub fn adapter_cmd(&self) -> &str {
        &self.adapter_cmd
    }

    /// Send `disconnect` and kill the adapter process.
    pub async fn shutdown(&mut self) {
        let _ = self
            .send_request(
                "disconnect",
                serde_json::json!({ "terminateDebuggee": true }),
            )
            .await;
        let mut child = self.child.lock().await;
        if let Some(c) = child.as_mut() {
            let _ = c.start_kill();
        }
        *child = None;
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
            Err(_) => return None,
        }
        if let Some(len_str) = line.trim().strip_prefix("Content-Length:")
            && let Ok(len) = len_str.trim().parse::<u32>()
        {
            // drain remaining headers until blank line
            loop {
                let mut h = String::new();
                match reader.read_line(&mut h).await {
                    Ok(0) => return None,
                    Ok(_) => {
                        if h.trim().is_empty() {
                            return Some(len);
                        }
                    }
                    Err(_) => return None,
                }
            }
        }
    }
}

// ── Debug Session (high-level wrapper over one client) ──────────────────────

/// A configured, running debug session bound to one adapter + program.
pub struct DapSession {
    client: DapClient,
    pub program: String,
    pub cwd: Option<String>,
    pub dap_type: String,
    breakpoints: Vec<DapBreakpoint>,
    output_lines: Vec<String>,
    /// Launch request arguments, stashed here until `configuration_done` fires
    /// the actual `launch` request (so all breakpoints are already installed
    /// when the adapter starts the program).
    pending_launch: Option<Value>,
    /// Persistent subscription to the client's broadcast channel. Held for the
    /// session's lifetime so no adapter event is ever dropped by the sender
    /// (broadcast messages are discarded when no subscriber exists).
    events: broadcast::Receiver<InboundMessage>,
}

impl DapSession {
    /// Construct a session with a launch-request body deferred to
    /// `configuration_done`.
    pub fn prepared(
        client: DapClient,
        program: String,
        args: Vec<String>,
        cwd: Option<String>,
        env: HashMap<String, String>,
        dap_type: String,
        extra: Option<Value>,
    ) -> Self {
        let mut launch_args = serde_json::json!({
            "request": "launch",
            "name": format!("Debug {}", program),
            "program": program,
            "args": args,
            "cwd": cwd.clone().unwrap_or_else(|| ".".to_string()),
            "stopAtEntry": false,
        });
        if !env.is_empty() {
            let map: serde_json::Map<String, Value> = env
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect();
            launch_args["env"] = Value::Object(map);
        }
        if let Some(Value::Object(m)) = extra
            && let Some(obj) = launch_args.as_object_mut()
        {
            for (k, v) in m {
                obj.insert(k, v);
            }
        }
        launch_args["type"] = Value::String(dap_type.clone());

        // Subscribe IMMEDIATELY (before any further request) so the persistent
        // receiver keeps the broadcast channel alive for every future event.
        let events = client.subscribe_events();

        DapSession {
            client,
            program,
            cwd,
            dap_type,
            breakpoints: Vec::new(),
            output_lines: Vec::new(),
            pending_launch: Some(launch_args),
            events,
        }
    }

    /// Set breakpoints on a single source file (replacing any prior ones there).
    pub async fn set_breakpoints(
        &mut self,
        path: &str,
        lines: &[u32],
    ) -> Result<Vec<DapBreakpoint>, String> {
        let bps: Vec<Value> = lines
            .iter()
            .map(|l| serde_json::json!({ "line": l }))
            .collect();
        let body = self
            .client
            .send_request(
                "setBreakpoints",
                serde_json::json!({
                    "source": { "path": path },
                    "breakpoints": bps,
                    "sourceModified": false
                }),
            )
            .await?;
        let parsed = parse_breakpoints(&body, path, lines);
        // Replace breakpoints belonging to this source path.
        self.breakpoints.retain(|b| b.path != path);
        self.breakpoints.extend(parsed.iter().cloned());
        Ok(parsed)
    }

    /// Finish configuration so execution may begin. Sends the deferred
    /// `launch` request (if any) first, then `configurationDone`.
    pub async fn configuration_done(&mut self) -> Result<(), String> {
        if let Some(launch_args) = self.pending_launch.take() {
            self.client.send_request("launch", launch_args).await?;
        }
        self.client
            .send_request("configurationDone", serde_json::json!({}))
            .await
            .map(|_| ())
    }

    pub async fn threads(&mut self) -> Result<Vec<i64>, String> {
        let body = self
            .client
            .send_request("threads", serde_json::json!({}))
            .await?;
        Ok(body
            .get("threads")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.get("id").and_then(|x| x.as_i64()))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn first_thread(&mut self) -> Result<i64, String> {
        self.threads().await.and_then(|t| {
            t.into_iter()
                .next()
                .ok_or_else(|| "No debug threads".to_string())
        })
    }

    /// Wait for the next `stopped` event without sending any resume request.
    /// Used right after `configuration_done` to capture the first breakpoint
    /// hit (execution starts automatically on configurationDone).
    pub async fn wait_for_stop(&mut self, timeout_secs: u64) -> Result<Value, String> {
        let evt = wait_for_event(
            &mut self.events,
            "stopped",
            std::time::Duration::from_secs(timeout_secs),
        )
        .await?;
        Ok(match evt {
            InboundMessage::Event { body, .. } => body,
            _ => Value::Null,
        })
    }

    /// Resume execution (`continue` / `next` / `stepIn` / `stepOut`) and wait
    /// for the resulting `stopped` event. `wait` skips sending any request and
    /// only awaits the initial stop after configurationDone.
    pub async fn resume_and_wait(
        &mut self,
        kind: &str,
        timeout_secs: u64,
    ) -> Result<Value, String> {
        if kind == "wait" {
            return self.wait_for_stop(timeout_secs).await;
        }
        let thread_id = self.first_thread().await?;
        match kind {
            "continue" => {
                self.client
                    .send_request("continue", serde_json::json!({ "threadId": thread_id }))
                    .await?;
            }
            "next" => {
                self.client
                    .send_request("next", serde_json::json!({ "threadId": thread_id }))
                    .await?;
            }
            "stepIn" => {
                self.client
                    .send_request("stepIn", serde_json::json!({ "threadId": thread_id }))
                    .await?;
            }
            "stepOut" => {
                self.client
                    .send_request("stepOut", serde_json::json!({ "threadId": thread_id }))
                    .await?;
            }
            other => return Err(format!("Unknown resume kind: {}", other)),
        }
        let evt = wait_for_event(
            &mut self.events,
            "stopped",
            std::time::Duration::from_secs(timeout_secs),
        )
        .await?;
        Ok(match evt {
            InboundMessage::Event { body, .. } => body,
            _ => Value::Null,
        })
    }

    pub async fn stack_trace(&mut self) -> Result<Vec<DapStackFrame>, String> {
        let thread_id = self.first_thread().await?;
        let body = self
            .client
            .send_request("stackTrace", serde_json::json!({ "threadId": thread_id }))
            .await?;
        Ok(parse_frames(&body))
    }

    pub async fn scopes(&mut self, frame_id: i64) -> Result<Vec<DapScope>, String> {
        let body = self
            .client
            .send_request("scopes", serde_json::json!({ "frameId": frame_id }))
            .await?;
        Ok(body
            .get("scopes")
            .and_then(|s| s.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|s| DapScope {
                        name: s
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        variables_reference: s
                            .get("variablesReference")
                            .and_then(|x| x.as_i64())
                            .unwrap_or(0),
                        expensive: s
                            .get("expensive")
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn variables(&mut self, reference: i64) -> Result<Vec<DapVariable>, String> {
        let body = self
            .client
            .send_request(
                "variables",
                serde_json::json!({ "variablesReference": reference }),
            )
            .await?;
        Ok(parse_variables(&body))
    }

    pub async fn evaluate(
        &mut self,
        expression: &str,
        frame_id: Option<i64>,
    ) -> Result<String, String> {
        let mut args = serde_json::json!({ "expression": expression, "context": "watch" });
        if let Some(f) = frame_id {
            args["frameId"] = serde_json::json!(f);
        }
        let body = self.client.send_request("evaluate", args).await?;
        Ok(body
            .get("result")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string())
    }

    pub fn breakpoints(&self) -> Vec<DapBreakpoint> {
        self.breakpoints.clone()
    }

    pub async fn shutdown(&mut self) {
        self.client.shutdown().await;
    }

    /// Convenience: capture a full runtime snapshot at the current stop.
    pub async fn snapshot(&mut self, reason: &str, description: Option<String>) -> RuntimeSnapshot {
        let frames = self.stack_trace().await.unwrap_or_default();
        let top = frames.first().cloned();
        let mut locals = Vec::new();
        if let Some(top_frame) = &top
            && let Ok(scopes) = self.scopes(top_frame.id).await
        {
            for scope in scopes.iter().take(2) {
                if let Ok(vars) = self.variables(scope.variables_reference).await {
                    locals.extend(vars);
                }
            }
        }
        let thread_id = self
            .threads()
            .await
            .unwrap_or_default()
            .into_iter()
            .next()
            .unwrap_or(1);
        RuntimeSnapshot {
            stopped: true,
            reason: reason.to_string(),
            description,
            thread_id,
            top_frame: top,
            frames,
            locals,
            output: self.output_lines.clone(),
        }
    }
}

/// Consume broadcast events until one with the given `kind` arrives (or timeout).
pub async fn wait_for_event(
    rx: &mut broadcast::Receiver<InboundMessage>,
    kind: &str,
    timeout: std::time::Duration,
) -> Result<InboundMessage, String> {
    let fut = async {
        loop {
            match rx.recv().await {
                Ok(msg) => match &msg {
                    InboundMessage::Event { event, .. } if event == kind => return Ok(msg),
                    InboundMessage::Event { event, .. } if event == "terminated" => {
                        return Err("Debug target terminated before stop".to_string());
                    }
                    _ => {}
                },
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    return Err("Event stream closed".to_string());
                }
            }
        }
    };
    tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| format!("Timed out waiting for '{}' event", kind))?
}

// ── Session manager ─────────────────────────────────────────────────────────

/// Manages multiple named debug sessions (one per workspace / agent run).
pub struct DapManager {
    sessions: Arc<RwLock<HashMap<String, Arc<RwLock<DapSession>>>>>,
}

impl Default for DapManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DapManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn list_ids(&self) -> Vec<String> {
        self.sessions.read().await.keys().cloned().collect()
    }

    async fn get(&self, session_id: &str) -> Result<Arc<RwLock<DapSession>>, String> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| format!("No debug session with id '{}'", session_id))
    }

    /// Start a new debug session, launching `program` under the resolved adapter.
    pub async fn start_session(
        &self,
        session_id: &str,
        language: &str,
        program: &str,
        args: Vec<String>,
        cwd: Option<String>,
        adapter_override: Option<&str>,
        extra_launch: Option<Value>,
    ) -> Result<DebugSessionInfo, String> {
        let (cmd, cmd_args, dap_type) = adapter_for_language(language, adapter_override)
            .ok_or_else(|| {
                format!(
                    "No debug adapter configured for language '{}'. Pass an explicit adapter command.",
                    language
                )
            })?;

        let client = DapClient::start(&cmd, &cmd_args).await?;
        let session = DapSession::prepared(
            client,
            program.to_string(),
            args,
            cwd,
            HashMap::new(),
            dap_type,
            extra_launch,
        );

        let info = DebugSessionInfo {
            session_id: session_id.to_string(),
            running: true,
            adapter: cmd,
            program: program.to_string(),
            stopped: false,
            stop_reason: None,
            breakpoints: Vec::new(),
        };
        self.sessions
            .write()
            .await
            .insert(session_id.to_string(), Arc::new(RwLock::new(session)));
        Ok(info)
    }

    pub async fn set_breakpoints(
        &self,
        session_id: &str,
        path: &str,
        lines: Vec<u32>,
    ) -> Result<Vec<DapBreakpoint>, String> {
        let s = self.get(session_id).await?;
        let mut sess = s.write().await;
        sess.set_breakpoints(path, &lines).await
    }

    pub async fn configuration_done(&self, session_id: &str) -> Result<(), String> {
        let s = self.get(session_id).await?;
        let mut sess = s.write().await;
        sess.configuration_done().await
    }

    /// Resume (continue/next/stepIn/stepOut) and return a snapshot at the stop.
    pub async fn resume(
        &self,
        session_id: &str,
        kind: &str,
        timeout_secs: u64,
    ) -> Result<RuntimeSnapshot, String> {
        let s = self.get(session_id).await?;
        let mut sess = s.write().await;
        let body = sess.resume_and_wait(kind, timeout_secs).await?;
        let reason = body
            .get("reason")
            .and_then(|r| r.as_str())
            .unwrap_or("unknown")
            .to_string();
        let desc = body
            .get("description")
            .and_then(|d| d.as_str())
            .map(|s| s.to_string());
        Ok(sess.snapshot(&reason, desc).await)
    }

    pub async fn snapshot(&self, session_id: &str) -> Result<Vec<DapStackFrame>, String> {
        let s = self.get(session_id).await?;
        let mut sess = s.write().await;
        sess.stack_trace().await
    }

    pub async fn locals(&self, session_id: &str) -> Result<Vec<DapVariable>, String> {
        let s = self.get(session_id).await?;
        let mut sess = s.write().await;
        let frames = sess.stack_trace().await?;
        let mut out = Vec::new();
        if let Some(top) = frames.first() {
            for scope in sess.scopes(top.id).await.unwrap_or_default().iter().take(2) {
                out.extend(
                    sess.variables(scope.variables_reference)
                        .await
                        .unwrap_or_default(),
                );
            }
        }
        Ok(out)
    }

    pub async fn evaluate(&self, session_id: &str, expression: &str) -> Result<String, String> {
        let s = self.get(session_id).await?;
        let mut sess = s.write().await;
        let frames = sess.stack_trace().await.unwrap_or_default();
        let frame = frames.first().map(|f| f.id);
        sess.evaluate(expression, frame).await
    }

    pub async fn session_info(&self, session_id: &str) -> Result<DebugSessionInfo, String> {
        let s = self.get(session_id).await?;
        let sess = s.read().await;
        Ok(DebugSessionInfo {
            session_id: session_id.to_string(),
            running: true,
            adapter: sess.client.adapter_cmd().to_string(),
            program: sess.program.clone(),
            stopped: false,
            stop_reason: None,
            breakpoints: sess.breakpoints(),
        })
    }

    pub async fn stop_session(&self, session_id: &str) -> Result<(), String> {
        if let Some(s) = self.sessions.write().await.remove(session_id) {
            let mut sess = s.write().await;
            sess.shutdown().await;
        }
        Ok(())
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_message_has_content_length() {
        let msg = build_request(3, "next", serde_json::json!({ "threadId": 1 }));
        let bytes = encode_message(&msg);
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.starts_with("Content-Length: "));
        assert!(s.contains("\r\n\r\n"));
        assert!(s.contains("\"command\":\"next\""));
    }

    #[test]
    fn build_request_shape() {
        let req = build_request(7, "initialize", serde_json::json!({ "a": 1 }));
        assert_eq!(req["seq"], 7);
        assert_eq!(req["type"], "request");
        assert_eq!(req["command"], "initialize");
        assert_eq!(req["arguments"]["a"], 1);
    }

    #[test]
    fn classify_response() {
        let v = serde_json::json!({
            "seq": 5, "type": "response", "request_seq": 2,
            "success": true, "command": "stackTrace", "body": { "stackFrames": [] }
        });
        let m = classify_message(&v).unwrap();
        assert_eq!(
            m,
            InboundMessage::Response {
                request_seq: 2,
                success: true,
                message: None,
                body: v["body"].clone()
            }
        );
    }

    #[test]
    fn classify_error_response_carries_message() {
        let v = serde_json::json!({
            "seq": 5, "type": "response", "request_seq": 2,
            "success": false, "message": "boom", "command": "next"
        });
        let m = classify_message(&v).unwrap();
        match m {
            InboundMessage::Response {
                success, message, ..
            } => {
                assert!(!success);
                assert_eq!(message.as_deref(), Some("boom"));
            }
            _ => panic!("expected response"),
        }
    }

    #[test]
    fn classify_event_and_reverse_request() {
        let ev = serde_json::json!({
            "seq": 9, "type": "event", "event": "stopped", "body": { "reason": "breakpoint" }
        });
        assert!(matches!(
            classify_message(&ev),
            Some(InboundMessage::Event { ref event, .. }) if event == "stopped"
        ));

        let rr = serde_json::json!({
            "seq": 11, "type": "request", "command": "runInTerminal", "arguments": { "kind": "local" }
        });
        assert!(matches!(
            classify_message(&rr),
            Some(InboundMessage::ReverseRequest { ref command, .. }) if command == "runInTerminal"
        ));
    }

    #[test]
    fn parse_breakpoints_maps_verified() {
        let body = serde_json::json!({
            "breakpoints": [
                { "id": 1, "verified": true, "line": 12 },
                { "id": 2, "verified": false }
            ]
        });
        let bps = parse_breakpoints(&body, "src/main.rs", &[12, 20]);
        assert_eq!(bps.len(), 2);
        assert!(bps[0].verified);
        assert_eq!(bps[0].line, 12);
        assert_eq!(bps[1].line, 20); // falls back to requested line
        assert!(!bps[1].verified);
    }

    #[test]
    fn parse_frames_and_variables() {
        let body = serde_json::json!({
            "stackFrames": [
                { "id": 0, "name": "main", "line": 42, "column": 5,
                  "source": { "path": "/proj/src/main.rs" } }
            ]
        });
        let frames = parse_frames(&body);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].line, 42);
        assert!(frames[0].path.ends_with("main.rs"));

        let vbody = serde_json::json!({
            "variables": [
                { "name": "x", "value": "10", "type": "i32", "variablesReference": 0 },
                { "name": "v", "value": "[...]", "variablesReference": 7, "namedVariables": 3 }
            ]
        });
        let vars = parse_variables(&vbody);
        assert_eq!(vars[0].r#type, "i32");
        assert_eq!(vars[1].variables_reference, 7);
        assert_eq!(vars[1].named_variables, 3);
    }

    #[test]
    fn render_snapshot_lists_stack_and_locals() {
        let snap = RuntimeSnapshot {
            stopped: true,
            reason: "breakpoint".into(),
            description: None,
            thread_id: 1,
            top_frame: Some(DapStackFrame {
                id: 0,
                name: "do_work".into(),
                path: "/a/b/main.rs".into(),
                line: 30,
                column: 4,
            }),
            frames: vec![DapStackFrame {
                id: 0,
                name: "do_work".into(),
                path: "/a/b/main.rs".into(),
                line: 30,
                column: 4,
            }],
            locals: vec![DapVariable {
                name: "sum".into(),
                value: "42".into(),
                r#type: "i32".into(),
                variables_reference: 0,
                named_variables: 0,
            }],
            output: vec!["hello".into()],
        };
        let text = render_runtime_snapshot(&snap);
        assert!(text.contains("do_work"), "{}", text);
        assert!(text.contains("main.rs:30"), "{}", text);
        assert!(text.contains("sum"), "{}", text);
        assert!(text.contains("## Call stack"), "{}", text);
    }

    #[test]
    fn adapter_mapping_and_overrides() {
        let (cmd, _, ty) = adapter_for_language("rust", None).unwrap();
        assert_eq!(cmd, "codelldb");
        assert_eq!(ty, "lldb");

        let (cmd, args, ty) = adapter_for_language("python", None).unwrap();
        assert_eq!(cmd, "python");
        assert_eq!(args, vec!["-m", "debugpy.adapter"]);
        assert_eq!(ty, "python");

        let (cmd, _, _) = adapter_for_language("unknown", Some("my-dap")).unwrap();
        assert_eq!(cmd, "my-dap");

        assert!(adapter_for_language("unknown", None).is_none());
    }
}
