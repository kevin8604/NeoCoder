//! A2A Client — discover remote agents, send tasks, poll or stream results.
//!
//! Built on reqwest (already a dependency) with a JSON-RPC 2.0 envelope and
//! SSE consumption for `message/stream` / `tasks/resubscribe`.

use std::time::Duration;

use futures_util::StreamExt;
use serde_json::json;
use tokio_stream::wrappers::ReceiverStream;

use super::{A2aError, AgentCard, JsonRpcRequest, JsonRpcResponse, Message, MessageRole, Part, Task};

/// A2A protocol JSON-RPC method names.
pub mod methods {
    pub const MESSAGE_SEND: &str = "message/send";
    pub const MESSAGE_STREAM: &str = "message/stream";
    pub const TASKS_GET: &str = "tasks/get";
    pub const TASKS_CANCEL: &str = "tasks/cancel";
    pub const TASKS_RESUBSCRIBE: &str = "tasks/resubscribe";
}

pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const DISCOVER_TIMEOUT: Duration = Duration::from_secs(10);

/// HTTP client for talking to remote A2A agents.
pub struct A2aClient {
    http: reqwest::Client,
    base_url: String,
    token: Option<String>,
    poll_interval: Duration,
    timeout: Duration,
}

impl A2aClient {
    /// Create a client for the given base URL (e.g. `http://127.0.0.1:41234`).
    pub fn new(
        base_url: impl Into<String>,
        token: Option<String>,
        poll_interval: Duration,
        timeout: Duration,
    ) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent("NeoCoder-A2A/1.0")
                .build()
                .unwrap_or_default(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
            poll_interval,
            timeout,
        }
    }

    /// Create a client with default polling/timeout settings.
    pub fn with_defaults(base_url: impl Into<String>, token: Option<String>) -> Self {
        Self::new(base_url, token, DEFAULT_POLL_INTERVAL, DEFAULT_TIMEOUT)
    }

    fn bearer_header(&self) -> Option<(String, String)> {
        self.token
            .as_deref()
            .filter(|t| !t.is_empty())
            .map(|t| ("Authorization".to_string(), format!("Bearer {}", t)))
    }

    /// GET the Agent Card from `/.well-known/agent.json`, falling back to
    /// `/.well-known/agent-card.json` on 404.
    pub async fn discover(&self) -> Result<AgentCard, A2aError> {
        let mut attempts: Vec<String> = Vec::new();
        for path in [".well-known/agent.json", ".well-known/agent-card.json"] {
            let url = format!("{}/{}", self.base_url, path);
            let mut req = self.http.get(&url).timeout(DISCOVER_TIMEOUT);
            if let Some((k, v)) = self.bearer_header() {
                req = req.header(k, v);
            }
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let text = resp
                            .text()
                            .await
                            .map_err(|e| A2aError::Transport(e.to_string()))?;
                        return serde_json::from_str::<AgentCard>(&text)
                            .map_err(|e| A2aError::Transport(format!("invalid Agent Card JSON from {}: {}", url, e)));
                    }
                    if status.as_u16() == 404 {
                        attempts.push(url);
                        continue;
                    }
                    return Err(A2aError::Transport(format!(
                        "GET {} returned HTTP {}",
                        url,
                        status
                    )));
                }
                Err(e) => {
                    return Err(A2aError::Transport(format!(
                        "failed to reach {}: {}",
                        url, e
                    )))
                }
            }
        }
        Err(A2aError::Transport(format!(
            "agent card not found at any of: {}",
            attempts.join(", ")
        )))
    }

    /// POST `message/send` with the given text. Returns the task.
    /// If `streaming` is false and the returned task is already completed,
    /// the task is returned directly (no polling needed).
    /// `skill_id` (optional) selects a specific skill/agent advertised in the
    /// Agent Card — sent as `metadata.skillId`.
    pub async fn send_message(
        &self,
        card: &AgentCard,
        text: &str,
        streaming: bool,
        skill_id: Option<&str>,
    ) -> Result<Task, A2aError> {
        let mut params = json!({
            "message": Message {
                role: MessageRole::User,
                parts: vec![Part::Text { text: text.to_string() }],
                message_id: None,
            },
            "streaming": streaming,
        });
        if let Some(skill) = skill_id.filter(|s| !s.is_empty()) {
            params["metadata"] = json!({ "skillId": skill });
        }
        let req = JsonRpcRequest::new(json!(1), methods::MESSAGE_SEND).with_params(params);
        let resp = self.rpc_call(&card.url, &req).await?;
        serde_json::from_value::<Task>(resp)
            .map_err(|e| A2aError::Transport(format!("invalid Task in message/send response: {}", e)))
    }

    /// POST `tasks/get` to poll task state.
    pub async fn get_task(&self, url: &str, task_id: &str) -> Result<Task, A2aError> {
        let req = JsonRpcRequest::new(json!(1), methods::TASKS_GET).with_params(json!({ "id": task_id }));
        let resp = self.rpc_call(url, &req).await?;
        serde_json::from_value::<Task>(resp)
            .map_err(|e| A2aError::Transport(format!("invalid Task in tasks/get response: {}", e)))
    }

    /// POST `tasks/cancel`.
    pub async fn cancel_task(&self, url: &str, task_id: &str) -> Result<Task, A2aError> {
        let req =
            JsonRpcRequest::new(json!(1), methods::TASKS_CANCEL).with_params(json!({ "id": task_id }));
        let resp = self.rpc_call(url, &req).await?;
        serde_json::from_value::<Task>(resp)
            .map_err(|e| A2aError::Transport(format!("invalid Task in tasks/cancel response: {}", e)))
    }

    /// POST `tasks/resubscribe` and consume the SSE stream of task updates.
    /// Yields each parsed Task until the stream ends.
    pub async fn stream(
        &self,
        url: &str,
        task_id: &str,
    ) -> Result<ReceiverStream<Result<Task, A2aError>>, A2aError> {
        let req = JsonRpcRequest::new(json!(1), methods::TASKS_RESUBSCRIBE)
            .with_params(json!({ "id": task_id }));
        let mut builder = self.http.post(url);
        if let Some((k, v)) = self.bearer_header() {
            builder = builder.header(k, v);
        }
        let resp = builder
            .json(&req)
            .send()
            .await
            .map_err(|e| A2aError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(A2aError::Transport(format!(
                "tasks/resubscribe returned HTTP {}",
                resp.status()
            )));
        }

        let (tx, rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            let mut chunks = resp.bytes_stream();
            let mut buf = String::new();
            while let Some(chunk) = chunks.next().await {
                match chunk {
                    Ok(bytes) => {
                        buf.push_str(&String::from_utf8_lossy(&bytes));
                        // 提取所有完整事件块（以空行分隔）
                        loop {
                            match find_event_boundary(&buf) {
                                Some(end) => {
                                    let block = buf[..end].to_string();
                                    buf = buf[end..].to_string();
                                    for data in parse_sse_events(&block) {
                                        match serde_json::from_str::<Task>(&data) {
                                            Ok(task) => {
                                                if tx.send(Ok(task)).await.is_err() {
                                                    return;
                                                }
                                            }
                                            Err(_) => {} // 跳过非 Task 的 data
                                        }
                                    }
                                }
                                None => break,
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(A2aError::Transport(e.to_string()))).await;
                        break;
                    }
                }
            }
        });
        Ok(ReceiverStream::new(rx))
    }

    /// High-level invocation: discover → send → wait (sync/poll) or stream.
    /// `mode` is "sync" (default), "poll", or "stream".
    /// `skill_id` (optional) routes the task to a specific advertised skill.
    /// Returns a human-readable summary of the remote result.
    pub async fn invoke(
        &self,
        task_text: &str,
        mode: &str,
        skill_id: Option<&str>,
    ) -> Result<String, A2aError> {
        let mode = match mode {
            "stream" => "stream",
            "poll" => "poll",
            _ => "sync",
        };

        let card = match self.discover().await {
            Ok(card) => card,
            // 容错：调用方直接传了 /a2a 端点时，跳过 discover 直接使用
            Err(_) if self.base_url.ends_with("/a2a") => AgentCard {
                name: self.base_url.clone(),
                description: String::new(),
                url: self.base_url.clone(),
                version: String::new(),
                capabilities: Default::default(),
                authentication: None,
                default_input_modes: vec![],
                default_output_modes: vec![],
                skills: vec![],
            },
            Err(e) => return Err(e),
        };

        let send = self
            .send_message(&card, task_text, mode == "stream", skill_id)
            .await?;

        let final_task = if mode == "stream" {
            let mut last = send.clone();
            let stream = self.stream(&card.url, &send.id).await?;
            let mut stream = std::pin::pin!(stream);
            while let Some(item) = stream.next().await {
                match item {
                    Ok(task) => {
                        last = task.clone();
                        if task.is_terminal() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            // 流提前结束但未到终态：回退轮询
            if !last.is_terminal() {
                self.poll_until_terminal(&card.url, &send.id).await?
            } else {
                last
            }
        } else {
            self.poll_until_terminal(&card.url, &send.id).await?
        };

        let skills = card
            .skills
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let summary = super::summarize_task(&final_task);
        Ok(format!(
            "Remote agent '{}' (skills: {}) task completed:\n{}",
            card.name, skills, summary
        ))
    }

    /// Poll `tasks/get` until the task reaches a terminal state or timeout.
    async fn poll_until_terminal(&self, url: &str, task_id: &str) -> Result<Task, A2aError> {
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            let task = self.get_task(url, task_id).await?;
            if task.is_terminal() {
                return Ok(task);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(A2aError::Transport(format!(
                    "timed out after {}s waiting for task {} to finish",
                    self.timeout.as_secs(),
                    task_id
                )));
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// POST a JSON-RPC request and extract the `result` value.
    /// Surfaces remote JSON-RPC errors as `A2aError::Remote`.
    async fn rpc_call(&self, url: &str, req: &JsonRpcRequest) -> Result<serde_json::Value, A2aError> {
        let mut builder = self.http.post(url);
        if let Some((k, v)) = self.bearer_header() {
            builder = builder.header(k, v);
        }
        let resp = builder
            .json(req)
            .send()
            .await
            .map_err(|e| A2aError::Transport(format!("request to {} failed: {}", url, e)))?;
        if !resp.status().is_success() {
            return Err(A2aError::Transport(format!(
                "{} returned HTTP {}",
                url,
                resp.status()
            )));
        }
        let body: JsonRpcResponse = resp
            .json()
            .await
            .map_err(|e| A2aError::Transport(format!("invalid JSON-RPC response from {}: {}", url, e)))?;
        if let Some(err) = body.error {
            return Err(A2aError::Remote {
                code: err.code,
                message: err.message,
            });
        }
        body.result.ok_or_else(|| {
            A2aError::Transport(format!("JSON-RPC response from {} has neither result nor error", url))
        })
    }
}

/// Locate the end of the first complete SSE event block (empty-line separated).
/// Returns the byte index just past the terminating blank line.
fn find_event_boundary(buf: &str) -> Option<usize> {
    for (i, ch) in buf.char_indices() {
        if ch == '\n' {
            let after = &buf[i + 1..];
            if after.starts_with('\n') {
                return Some(i + 2);
            }
            if after.starts_with("\r\n") {
                return Some(i + 3);
            }
        }
    }
    None
}

/// Parse a raw SSE text block into the `data:` payloads of `task_update` events.
/// Non-JSON / malformed data lines are skipped by the caller.
pub fn parse_sse_events(text: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut in_task_update = false;
    let mut data_lines: Vec<String> = Vec::new();

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            in_task_update = rest.trim() == "task_update";
            data_lines.clear();
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            if in_task_update {
                data_lines.push(rest.trim_start().to_string());
            }
            continue;
        }
        if line.trim().is_empty() {
            // 事件结束
            if in_task_update && !data_lines.is_empty() {
                results.push(data_lines.join("\n"));
            }
            in_task_update = false;
            data_lines.clear();
        }
    }
    // 末尾无空行结束的最后一个事件
    if in_task_update && !data_lines.is_empty() {
        results.push(data_lines.join("\n"));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::{JsonRpcResponse, RpcError, TaskStatus, TaskState};
    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response, sse::{Event, KeepAlive, Sse}},
        routing::{get, post},
    };
    use serde_json::Value;
    use std::convert::Infallible;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    // ── mock server helpers ──

    fn task_json(id: &str, state: TaskState) -> Value {
        serde_json::to_value(Task::new(id, TaskStatus::new(state))).unwrap()
    }

    fn rpc_ok_result(result: Value) -> Json<JsonRpcResponse> {
        Json(JsonRpcResponse::ok(json!(1), result))
    }

    fn rpc_err_result(code: i32, message: &str) -> Json<JsonRpcResponse> {
        Json(JsonRpcResponse::err(
            json!(1),
            RpcError {
                code,
                message: message.to_string(),
                data: None,
            },
        ))
    }

    async fn spawn_mock_server(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{}", addr)
    }

    fn card_json(name: &str, url: &str) -> Value {
        json!({
            "name": name,
            "description": "mock",
            "url": url,
            "version": "1.0.0",
            "capabilities": { "streaming": true, "pushNotifications": false, "stateTransitionHistory": false },
            "skills": [{ "id": "sk1", "name": "Skill1", "description": "d" }]
        })
    }

    /// 简易 JSON-RPC mock：根据 method 返回不同行为
    #[derive(Clone)]
    struct MockBehavior {
        counter: Arc<AtomicUsize>,
        task_id: String,
    }

    async fn mock_a2a_handler(
        State(behavior): State<MockBehavior>,
        body: String,
    ) -> Response {
        let req: Value = serde_json::from_str(&body).unwrap();
        let method = req["method"].as_str().unwrap_or("");
        let n = behavior.counter.fetch_add(1, Ordering::SeqCst);
        match method {
            "message/send" => {
                if n < 2 {
                    // 前 2 次返回 working，之后 completed
                    rpc_ok_result(task_json(&behavior.task_id, TaskState::Working))
                        .into_response()
                } else {
                    rpc_ok_result(task_json(&behavior.task_id, TaskState::Completed)).into_response()
                }
            }
            "tasks/get" => {
                if n < 2 {
                    rpc_ok_result(task_json(&behavior.task_id, TaskState::Working)).into_response()
                } else {
                    rpc_ok_result(task_json(&behavior.task_id, TaskState::Completed)).into_response()
                }
            }
            "tasks/cancel" => {
                rpc_ok_result(task_json(&behavior.task_id, TaskState::Canceled)).into_response()
            }
            _ => rpc_err_result(-32601, "method not found").into_response(),
        }
    }

    /// 构建一个带 Agent Card 和 JSON-RPC mock 的完整 server
    /// Agent Card 的 url 从 Host 头推导，保证指向真实地址
    async fn spawn_full_mock_server() -> (String, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_for_router = counter.clone();
        let app = Router::new()
            .route(
                "/.well-known/agent.json",
                get(|headers: HeaderMap| async move {
                    let host = headers
                        .get("host")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("127.0.0.1:0")
                        .to_string();
                    Json(card_json("MockAgent", &format!("http://{}/a2a", host)))
                }),
            )
            .route(
                "/a2a",
                post(mock_a2a_handler).with_state(MockBehavior {
                    counter: counter_for_router,
                    task_id: "task-1".into(),
                }),
            );
        let base = spawn_mock_server(app).await;
        (base, counter)
    }

    // ── tests ──

    #[tokio::test]
    async fn test_discover_parses_agent_card() {
        let (base, _) = spawn_full_mock_server().await;
        let client = A2aClient::with_defaults(&base, None);
        let card = client.discover().await.unwrap();
        assert_eq!(card.name, "MockAgent");
        assert_eq!(card.skills.len(), 1);
        assert_eq!(card.skills[0].id, "sk1");
    }

    #[tokio::test]
    async fn test_discover_falls_back_to_agent_card_json() {
        // agent.json 404 → agent-card.json 成功
        let app = Router::new()
            .route("/.well-known/agent.json", get(|| async { StatusCode::NOT_FOUND }))
            .route(
                "/.well-known/agent-card.json",
                get(|| async { Json(card_json("Fallback", "http://x/a2a")) }),
            );
        let base = spawn_mock_server(app).await;
        let client = A2aClient::with_defaults(&base, None);
        let card = client.discover().await.unwrap();
        assert_eq!(card.name, "Fallback");
    }

    #[tokio::test]
    async fn test_discover_both_missing_returns_error() {
        let app = Router::new()
            .route(
                "/.well-known/agent.json",
                get(|| async { StatusCode::NOT_FOUND }),
            )
            .route(
                "/.well-known/agent-card.json",
                get(|| async { StatusCode::NOT_FOUND }),
            );
        let base = spawn_mock_server(app).await;
        let client = A2aClient::with_defaults(&base, None);
        let err = client.discover().await.unwrap_err();
        assert!(err.to_string().contains("agent card not found"), "{}", err);
    }

    #[tokio::test]
    async fn test_discover_invalid_json_returns_error() {
        let app = Router::new()
            .route("/.well-known/agent.json", get(|| async { "not json at all" }));
        let base = spawn_mock_server(app).await;
        let client = A2aClient::with_defaults(&base, None);
        let err = client.discover().await.unwrap_err();
        assert!(err.to_string().contains("invalid Agent Card"), "{}", err);
    }

    #[tokio::test]
    async fn test_send_message_sync_completed() {
        // mock 直接返回 completed
        let app = Router::new().route(
            "/a2a",
            post(|body: String| async move {
                let req: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(req["method"], "message/send");
                assert_eq!(req["params"]["message"]["role"], "user");
                assert_eq!(req["params"]["streaming"], false);
                rpc_ok_result(task_json("t1", TaskState::Completed))
            }),
        );
        let base = spawn_mock_server(app).await;
        let client = A2aClient::with_defaults(&base, None);
        let card: AgentCard = serde_json::from_value(card_json("M", &format!("{}/a2a", base))).unwrap();
        let task = client.send_message(&card, "hello", false, None).await.unwrap();
        assert_eq!(task.status.state, TaskState::Completed);
    }

    #[tokio::test]
    async fn test_send_message_sends_skill_id() {
        // 记录收到的请求 body，断言 metadata.skillId 透传
        let seen = Arc::new(tokio::sync::Mutex::new(None::<Value>));
        let seen_for_router = seen.clone();
        let app = Router::new()
            .route(
                "/.well-known/agent.json",
                get(|| async { Json(card_json("MockAgent", "http://x/a2a")) }),
            )
            .route(
                "/a2a",
                post(move |body: String| {
                    let seen = seen_for_router.clone();
                    async move {
                        *seen.lock().await = serde_json::from_str::<Value>(&body).ok();
                        rpc_ok_result(task_json("t-skill", TaskState::Completed)).into_response()
                    }
                }),
            );
        let base = spawn_mock_server(app).await;
        let client = A2aClient::with_defaults(&base, None);
        let card: AgentCard =
            serde_json::from_value(card_json("M", &format!("{}/a2a", base))).unwrap();

        // 带 skill_id → metadata.skillId
        client
            .send_message(&card, "hello", false, Some("code_writer"))
            .await
            .unwrap();
        let body = seen.lock().await.clone().unwrap();
        assert_eq!(body["params"]["metadata"]["skillId"], "code_writer");

        // None → 不带 metadata
        client.send_message(&card, "hello", false, None).await.unwrap();
        let body = seen.lock().await.clone().unwrap();
        assert!(body["params"].get("metadata").is_none());
    }

    #[tokio::test]
    async fn test_send_message_returns_working_task() {
        let (base, _) = spawn_full_mock_server().await;
        let client = A2aClient::with_defaults(&base, None);
        let card: AgentCard = serde_json::from_value(card_json("M", &format!("{}/a2a", base))).unwrap();
        let task = client.send_message(&card, "hello", true, None).await.unwrap();
        assert_eq!(task.id, "task-1");
        assert_eq!(task.status.state, TaskState::Working);
    }

    #[tokio::test]
    async fn test_get_task_polls_until_completed() {
        let (base, counter) = spawn_full_mock_server().await;
        let client = A2aClient::new(
            &base,
            None,
            Duration::from_millis(10),
            Duration::from_secs(5),
        );
        let url = format!("{}/a2a", base);
        let task = client.poll_until_terminal(&url, "task-1").await.unwrap();
        assert_eq!(task.status.state, TaskState::Completed);
        // message/send + 2×tasks/get（working→completed）
        assert!(counter.load(Ordering::SeqCst) >= 3, "calls: {}", counter.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_get_task_timeout() {
        // 永远返回 working → 超时
        let app = Router::new().route(
            "/a2a",
            post(|| async { rpc_ok_result(task_json("slow", TaskState::Working)) }),
        );
        let base = spawn_mock_server(app).await;
        let client = A2aClient::new(
            &base,
            None,
            Duration::from_millis(10),
            Duration::from_millis(150),
        );
        let err = client
            .poll_until_terminal(&format!("{}/a2a", base), "slow")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("timed out"), "{}", err);
    }

    #[tokio::test]
    async fn test_cancel_task() {
        let (base, _) = spawn_full_mock_server().await;
        let client = A2aClient::with_defaults(&base, None);
        let task = client
            .cancel_task(&format!("{}/a2a", base), "task-1")
            .await
            .unwrap();
        assert_eq!(task.status.state, TaskState::Canceled);
    }

    #[tokio::test]
    async fn test_invoke_sync_mode() {
        let (base, _) = spawn_full_mock_server().await;
        let client = A2aClient::new(&base, None, Duration::from_millis(10), Duration::from_secs(5));
        let result = client.invoke("do something", "sync", None).await.unwrap();
        assert!(result.contains("Remote agent 'MockAgent'"), "{}", result);
        assert!(result.contains("skills: sk1"), "{}", result);
        assert!(result.contains("completed"), "{}", result);
    }

    #[tokio::test]
    async fn test_invoke_poll_mode() {
        let (base, _) = spawn_full_mock_server().await;
        let client = A2aClient::new(&base, None, Duration::from_millis(10), Duration::from_secs(5));
        let result = client.invoke("do something", "poll", None).await.unwrap();
        assert!(result.contains("task completed"), "{}", result);
    }

    #[tokio::test]
    async fn test_invoke_stream_mode() {
        // mock SSE：先 event: task_update working，再 completed，最后结束流
        let app = Router::new()
            .route(
                "/.well-known/agent.json",
                get(|headers: HeaderMap| async move {
                    let host = headers
                        .get("host")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("127.0.0.1:0")
                        .to_string();
                    Json(card_json("SseAgent", &format!("http://{}/a2a", host)))
                }),
            )
            .route(
                "/a2a",
                post(|body: String| async move {
                    let req: Value = serde_json::from_str(&body).unwrap();
                    match req["method"].as_str().unwrap() {
                        "message/send" => {
                            rpc_ok_result(task_json("s1", TaskState::Working)).into_response()
                        }
                        "tasks/resubscribe" => {
                            let t1 = task_json("s1", TaskState::Working);
                            let t2 = task_json("s1", TaskState::Completed);
                            let stream = tokio_stream::iter(vec![
                                Ok::<Event, Infallible>(
                                    Event::default().event("task_update").data(t1.to_string()),
                                ),
                                Ok::<Event, Infallible>(
                                    Event::default().event("task_update").data(t2.to_string()),
                                ),
                            ]);
                            Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
                        }
                        _ => rpc_err_result(-32601, "method not found").into_response(),
                    }
                }),
            );
        let base = spawn_mock_server(app).await;
        let client = A2aClient::new(&base, None, Duration::from_millis(10), Duration::from_secs(5));
        let result = client.invoke("stream task", "stream", None).await.unwrap();
        assert!(result.contains("SseAgent"), "{}", result);
        assert!(result.contains("completed"), "{}", result);
    }

    #[tokio::test]
    async fn test_invoke_error_paths() {
        // 不可达 URL
        let client = A2aClient::with_defaults("http://127.0.0.1:1", None);
        let err = client.invoke("hi", "sync", None).await.unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn test_rpc_error_response_surface() {
        // 远端返回 JSON-RPC error → Remote 错误透传 code/message
        let app = Router::new().route(
            "/a2a",
            post(|| async { rpc_err_result(-32602, "bad task id") }),
        );
        let base = spawn_mock_server(app).await;
        let client = A2aClient::with_defaults(&base, None);
        let err = client
            .get_task(&format!("{}/a2a", base), "x")
            .await
            .unwrap_err();
        match err {
            A2aError::Remote { code, message } => {
                assert_eq!(code, -32602);
                assert!(message.contains("bad task id"));
            }
            other => panic!("expected Remote error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sse_parse_events() {
        let text = "event: task_update\ndata: {\"id\":\"a\"}\n\n\
                    event: keepalive\ndata: ping\n\n\
                    event: task_update
data: {\"id\":\"b\"}
data: {\"id\":\"c\"}

";
        let events = parse_sse_events(text);
        assert_eq!(events.len(), 2, "{:?}", events);
        assert!(events[0].contains("\"a\""));
        // 多行 data 合并
        assert!(events[1].contains("\"b\"") && events[1].contains("\"c\""));
    }

    #[tokio::test]
    async fn test_sse_malformed_event_skipped() {
        let text = "event: task_update\ndata: not json\n\n\
                    event: task_update\ndata: {\"id\":\"ok\"}\n\n";
        let events = parse_sse_events(text);
        assert_eq!(events.len(), 2);
        // 非 JSON / 缺字段的 data 在 stream() 中被跳过：只有完整 Task 被解析
        let valid = task_json("ok", TaskState::Working).to_string();
        let tasks: Vec<Task> = events
            .iter()
            .filter_map(|d| serde_json::from_str::<Task>(d).ok())
            .collect();
        assert_eq!(tasks.len(), 0, "both data lines are invalid tasks");
        // 混入合法 Task 后能解析
        let mixed = format!("{}\nevent: task_update\ndata: {}\n\n", text, valid);
        let tasks: Vec<Task> = parse_sse_events(&mixed)
            .iter()
            .filter_map(|d| serde_json::from_str::<Task>(d).ok())
            .collect();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "ok");
    }

    #[tokio::test]
    async fn test_discover_with_token_auth() {
        // server 校验 Bearer token；client 携带 token 成功，不携带被拒
        let app = Router::new().route(
            "/.well-known/agent.json",
            get(|headers: HeaderMap| async move {
                match headers.get("authorization") {
                    Some(v) if v == "Bearer secret123" => {
                        Json(card_json("SecureAgent", "http://x/a2a")).into_response()
                    }
                    _ => StatusCode::UNAUTHORIZED.into_response(),
                }
            }),
        );
        let base = spawn_mock_server(app).await;

        // 带 token → 成功
        let client = A2aClient::with_defaults(&base, Some("secret123".into()));
        let card = client.discover().await.unwrap();
        assert_eq!(card.name, "SecureAgent");

        // 不带 token → 失败
        let client_no_token = A2aClient::with_defaults(&base, None);
        let err = client_no_token.discover().await.unwrap_err();
        assert!(err.to_string().contains("401"), "{}", err);
    }

    #[test]
    fn test_find_event_boundary() {
        assert_eq!(find_event_boundary("a\n\nb"), Some(3));
        assert_eq!(find_event_boundary("a\r\n\r\nb"), Some(5));
        assert_eq!(find_event_boundary("no blank line yet"), None);
        assert_eq!(find_event_boundary("data: x\n"), None);
    }
}
