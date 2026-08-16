//! A2A Server — exposes NeoCoder as an A2A agent over HTTP + JSON-RPC 2.0.
//!
//! Routes:
//! - `GET  /.well-known/agent.json` — Agent Card (skills from AgentRegistry)
//! - `POST /a2a` — JSON-RPC dispatch: `message/send`, `message/stream` (SSE),
//!   `tasks/get`, `tasks/cancel`, `tasks/resubscribe` (SSE)
//!
//! Task execution is delegated to a pluggable `TaskExecutor` (production uses
//! the same background sub-agent machinery as Cloud Agent; tests use mocks).

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{Mutex, broadcast};

use super::{
    A2aError, AgentCard, Artifact, JsonRpcRequest, JsonRpcResponse, Message, Part, Skill, Task,
    TaskState, TaskStatus, agents_to_skills, build_agent_card,
};
use crate::agent::cloud::CloudTaskStatus;
use tauri::Manager;

/// Max accepted message size (bytes) to protect against memory abuse.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// How long the production executor waits for a background agent before failing.
pub const EXECUTOR_MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Task executor abstraction — decouples the HTTP layer from how tasks run.
#[async_trait::async_trait]
pub trait TaskExecutor: Send + Sync {
    /// Execute a task text, returning the result text or an error.
    /// `agent_id` is the requested skill/agent (from `metadata.skillId`),
    /// or `None` to fall back to the default (orchestrator).
    async fn execute(&self, task_text: String, agent_id: Option<String>) -> Result<String, String>;
}

/// Production executor: runs the task with a background sub-agent
/// (same machinery as Cloud Agent) and waits for completion.
pub struct CloudAgentExecutor {
    pub app: tauri::AppHandle,
}

#[async_trait::async_trait]
impl TaskExecutor for CloudAgentExecutor {
    async fn execute(&self, task_text: String, agent_id: Option<String>) -> Result<String, String> {
        use crate::commands::cloud::CloudTaskState;

        let session_id = format!("a2a-{}", uuid::Uuid::new_v4());
        let project_path = self
            .app
            .try_state::<Arc<tokio::sync::RwLock<crate::config::AppSettings>>>()
            .and_then(|s| {
                let guard = tokio::task::block_in_place(|| s.blocking_read());
                guard.project_paths.first().cloned()
            });

        let resolved_agent = agent_id.unwrap_or_else(|| "orchestrator".to_string());
        let task_id = crate::agent::cloud::spawn_background_sub_agent(
            self.app.clone(),
            session_id,
            task_text,
            resolved_agent,
            project_path,
        )?;

        let manager = self
            .app
            .try_state::<CloudTaskState>()
            .map(|s| s.inner().clone())
            .ok_or_else(|| "Error: CloudTaskManager not available".to_string())?;

        let started = tokio::time::Instant::now();
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if let Some(cloud_task) = manager.get(&task_id).await {
                match cloud_task.status {
                    CloudTaskStatus::Completed => {
                        return Ok(cloud_task.result.unwrap_or_default());
                    }
                    CloudTaskStatus::Failed(msg) => return Err(msg),
                    CloudTaskStatus::Cancelled => {
                        return Err("task was cancelled".to_string());
                    }
                    _ => {}
                }
            }
            if started.elapsed() > EXECUTOR_MAX_WAIT {
                return Err(format!(
                    "background agent did not finish within {}s",
                    EXECUTOR_MAX_WAIT.as_secs()
                ));
            }
        }
    }
}

/// Shared state for the A2A HTTP server.
pub struct A2aServerState {
    pub server_name: String,
    pub server_url: String,
    pub token: Option<String>,
    pub skills: Vec<Skill>,
    pub tasks: Mutex<HashMap<String, Task>>,
    pub tx: broadcast::Sender<String>,
    pub executor: Arc<dyn TaskExecutor>,
}

impl A2aServerState {
    pub fn new(
        server_name: impl Into<String>,
        server_url: impl Into<String>,
        token: Option<String>,
        skills: Vec<Skill>,
        executor: Arc<dyn TaskExecutor>,
    ) -> Self {
        let (tx, _rx) = broadcast::channel(256);
        Self {
            server_name: server_name.into(),
            server_url: server_url.into(),
            token,
            skills,
            tasks: Mutex::new(HashMap::new()),
            tx,
            executor,
        }
    }

    pub async fn get_task(&self, id: &str) -> Option<Task> {
        self.tasks.lock().await.get(id).cloned()
    }

    /// Insert/update a task, stamp the updated_at timestamp and broadcast.
    async fn store_task(&self, task: Task) {
        let mut tasks = self.tasks.lock().await;
        tasks.insert(task.id.clone(), task.clone());
        drop(tasks);
        let _ = self
            .tx
            .send(serde_json::to_string(&task).unwrap_or_default());
    }
}

/// Map a CloudTask status onto the A2A task state machine.
pub fn map_cloud_status(status: &CloudTaskStatus) -> TaskState {
    match status {
        CloudTaskStatus::Pending => TaskState::Submitted,
        CloudTaskStatus::Running => TaskState::Working,
        CloudTaskStatus::Completed => TaskState::Completed,
        CloudTaskStatus::Failed(_) => TaskState::Failed,
        CloudTaskStatus::Cancelled => TaskState::Canceled,
        // 中断任务可通过 resume_cloud_task 恢复，A2A 视角视为 submitted（可重新运行）
        CloudTaskStatus::Interrupted => TaskState::Submitted,
    }
}

/// Build the axum router. The state must be `Arc<A2aServerState>`.
pub fn build_router(state: Arc<A2aServerState>) -> Router {
    Router::new()
        .route("/.well-known/agent.json", get(agent_card_handler))
        .route("/.well-known/agent-card.json", get(agent_card_handler))
        .route("/a2a", post(a2a_handler))
        .with_state(state)
}

/// Start the A2A server on 127.0.0.1:port in the background.
/// Returns immediately; failures are logged.
pub fn start_server(app: tauri::AppHandle, port: u16, token: Option<String>) -> Result<(), String> {
    let agents = crate::agent::definition::load_agents_from_disk();
    let skills = agents_to_skills(&agents);
    let server_url = format!("http://127.0.0.1:{}/a2a", port);
    let state = Arc::new(A2aServerState::new(
        "NeoCoder",
        server_url,
        token,
        skills,
        Arc::new(CloudAgentExecutor { app }),
    ));
    let router = build_router(state.clone());

    tokio::spawn(async move {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => {
                log::info!("[A2A] Server listening on http://127.0.0.1:{}", port);
                if let Err(e) = axum::serve(listener, router).await {
                    log::error!("[A2A] Server error: {}", e);
                }
            }
            Err(e) => log::error!("[A2A] Failed to bind 127.0.0.1:{}: {}", port, e),
        }
    });
    Ok(())
}

// ── handlers ──

async fn agent_card_handler(State(state): State<Arc<A2aServerState>>) -> Json<AgentCard> {
    let card = build_agent_card(
        &state.server_name,
        &state.server_url,
        state.token.is_some(),
        state.skills.clone(),
    );
    Json(card)
}

/// Check the Authorization header against the configured token.
fn auth_ok(state: &A2aServerState, headers: &HeaderMap) -> bool {
    let Some(expected) = &state.token else {
        return true; // 无 token 配置 = 不要求认证
    };
    if expected.is_empty() {
        return true;
    }
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == format!("Bearer {}", expected))
        .unwrap_or(false)
}

fn rpc_err_response(id: Value, err: A2aError) -> Response {
    Json(JsonRpcResponse::err(id, err.to_rpc_error())).into_response()
}

fn rpc_ok_response(id: Value, result: Value) -> Response {
    Json(JsonRpcResponse::ok(id, result)).into_response()
}

async fn a2a_handler(
    State(state): State<Arc<A2aServerState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if !auth_ok(&state, &headers) {
        return rpc_err_response(
            json!(null),
            A2aError::AuthRequired("missing or invalid bearer token".into()),
        );
    }

    let req: JsonRpcRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            return rpc_err_response(
                json!(null),
                A2aError::InvalidParams(format!("invalid JSON-RPC request: {}", e)),
            );
        }
    };

    match req.method.as_str() {
        "message/send" => handle_message_send(&state, &req).await,
        "message/stream" => handle_message_stream(&state, &req).await,
        "tasks/get" => handle_tasks_get(&state, &req).await,
        "tasks/cancel" => handle_tasks_cancel(&state, &req).await,
        "tasks/resubscribe" => handle_resubscribe(&state, &req).await,
        other => rpc_err_response(req.id.clone(), A2aError::MethodNotFound(other.into())),
    }
}

/// Extract the text payload from a message/send params value.
fn extract_message_text(params: &Option<Value>) -> Result<String, A2aError> {
    let params = params
        .as_ref()
        .ok_or_else(|| A2aError::InvalidParams("missing params".into()))?;
    let message: Message =
        serde_json::from_value(params.get("message").cloned().unwrap_or_default())
            .map_err(|e| A2aError::InvalidParams(format!("invalid message: {}", e)))?;
    let text = message.text_content();
    if text.trim().is_empty() {
        return Err(A2aError::InvalidParams("message has no text parts".into()));
    }
    if text.len() > MAX_MESSAGE_BYTES {
        return Err(A2aError::InvalidParams(format!(
            "message exceeds {} bytes limit",
            MAX_MESSAGE_BYTES
        )));
    }
    Ok(text)
}

/// Resolve the requested skill/agent from `metadata.skillId` (or top-level
/// `skillId`). Returns `None` when the caller did not ask for a specific
/// skill; errors when a requested skill is not advertised in the Agent Card.
pub fn extract_skill_id(
    params: &Option<Value>,
    skills: &[Skill],
) -> Result<Option<String>, A2aError> {
    let Some(params) = params.as_ref() else {
        return Ok(None);
    };
    let requested = params
        .get("metadata")
        .and_then(|m| m.get("skillId"))
        .or_else(|| params.get("skillId"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(requested) = requested else {
        return Ok(None);
    };
    if skills.iter().any(|s| s.id == requested) {
        Ok(Some(requested))
    } else {
        let available = skills
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Err(A2aError::InvalidParams(format!(
            "unknown skill '{}' (available: {})",
            requested, available
        )))
    }
}

/// Create a task and spawn its execution. Returns the task.
async fn create_and_run_task(
    state: Arc<A2aServerState>,
    text: String,
    agent_id: Option<String>,
) -> Task {
    let id = format!("a2a-{}", uuid::Uuid::new_v4());
    let task = Task::new(id.clone(), TaskStatus::new(TaskState::Submitted));
    state.store_task(task.clone()).await;

    let state_clone = state.clone();
    let task_clone = task.clone();
    tokio::spawn(async move {
        let mut working = task_clone.clone();
        working.status = TaskStatus::new(TaskState::Working);
        state_clone.store_task(working).await;

        match state_clone.executor.execute(text, agent_id).await {
            Ok(result) => {
                let mut done = task_clone;
                done.status = TaskStatus::with_message(TaskState::Completed, "task completed");
                done.artifacts = vec![Artifact {
                    name: "result.txt".into(),
                    parts: vec![Part::Text { text: result }],
                    metadata: None,
                }];
                state_clone.store_task(done).await;
            }
            Err(e) => {
                let mut failed = task_clone;
                failed.status = TaskStatus::with_message(TaskState::Failed, e);
                state_clone.store_task(failed).await;
            }
        }
    });

    task
}

async fn handle_message_send(state: &Arc<A2aServerState>, req: &JsonRpcRequest) -> Response {
    let text = match extract_message_text(&req.params) {
        Ok(t) => t,
        Err(e) => return rpc_err_response(req.id.clone(), e),
    };
    let agent_id = match extract_skill_id(&req.params, &state.skills) {
        Ok(s) => s,
        Err(e) => return rpc_err_response(req.id.clone(), e),
    };
    let task = create_and_run_task(state.clone(), text, agent_id).await;
    rpc_ok_response(
        req.id.clone(),
        serde_json::to_value(&task).unwrap_or_default(),
    )
}

/// SSE stream for a task: emits the initial state, then all subsequent updates.
fn task_sse_stream(
    state: Arc<A2aServerState>,
    initial: Option<Task>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.tx.subscribe();
    let initial_events = initial.into_iter().map(|t| {
        Ok::<Event, Infallible>(
            Event::default()
                .event("task_update")
                .data(serde_json::to_string(&t).unwrap_or_default()),
        )
    });
    let live = futures_util::stream::unfold(rx, |mut rx| async move {
        let msg = loop {
            match rx.recv().await {
                Ok(v) => break Some(v),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break None,
            }
        };
        msg.map(|json_str| {
            (
                Ok::<Event, Infallible>(Event::default().event("task_update").data(json_str)),
                rx,
            )
        })
    });
    Sse::new(futures_util::stream::iter(initial_events).chain(live))
}

async fn handle_message_stream(state: &Arc<A2aServerState>, req: &JsonRpcRequest) -> Response {
    let text = match extract_message_text(&req.params) {
        Ok(t) => t,
        Err(e) => return rpc_err_response(req.id.clone(), e),
    };
    let agent_id = match extract_skill_id(&req.params, &state.skills) {
        Ok(s) => s,
        Err(e) => return rpc_err_response(req.id.clone(), e),
    };
    let task = create_and_run_task(state.clone(), text, agent_id).await;
    task_sse_stream(state.clone(), Some(task)).into_response()
}

async fn handle_tasks_get(state: &Arc<A2aServerState>, req: &JsonRpcRequest) -> Response {
    let task_id = req
        .params
        .as_ref()
        .and_then(|p| p.get("id").and_then(|v| v.as_str()))
        .unwrap_or("");
    if task_id.is_empty() {
        return rpc_err_response(
            req.id.clone(),
            A2aError::InvalidParams("missing task id".into()),
        );
    }
    match state.get_task(task_id).await {
        Some(task) => rpc_ok_response(
            req.id.clone(),
            serde_json::to_value(&task).unwrap_or_default(),
        ),
        None => rpc_err_response(
            req.id.clone(),
            A2aError::TaskNotFound(format!("task {} not found", task_id)),
        ),
    }
}

async fn handle_tasks_cancel(state: &Arc<A2aServerState>, req: &JsonRpcRequest) -> Response {
    let task_id = req
        .params
        .as_ref()
        .and_then(|p| p.get("id").and_then(|v| v.as_str()))
        .unwrap_or("");
    if task_id.is_empty() {
        return rpc_err_response(
            req.id.clone(),
            A2aError::InvalidParams("missing task id".into()),
        );
    }
    let mut tasks = state.tasks.lock().await;
    match tasks.get_mut(task_id) {
        Some(task) => {
            if !task.is_terminal() {
                task.status = TaskStatus::with_message(TaskState::Canceled, "cancelled by request");
            }
            let cancelled = task.clone();
            drop(tasks);
            let _ = state
                .tx
                .send(serde_json::to_string(&cancelled).unwrap_or_default());
            rpc_ok_response(
                req.id.clone(),
                serde_json::to_value(&cancelled).unwrap_or_default(),
            )
        }
        None => rpc_err_response(
            req.id.clone(),
            A2aError::TaskNotFound(format!("task {} not found", task_id)),
        ),
    }
}

async fn handle_resubscribe(state: &Arc<A2aServerState>, req: &JsonRpcRequest) -> Response {
    let task_id = req
        .params
        .as_ref()
        .and_then(|p| p.get("id").and_then(|v| v.as_str()))
        .unwrap_or("");
    if task_id.is_empty() {
        return rpc_err_response(
            req.id.clone(),
            A2aError::InvalidParams("missing task id".into()),
        );
    }
    let current = state.get_task(task_id).await;
    if current.is_none() {
        return rpc_err_response(
            req.id.clone(),
            A2aError::TaskNotFound(format!("task {} not found", task_id)),
        );
    }
    task_sse_stream(state.clone(), current).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::client::parse_sse_events;
    use crate::agent::definition::default_agents;
    use axum::http::StatusCode;
    use serde_json::json;
    use std::time::Duration;

    /// Mock executor: returns a fixed result after an optional delay and
    /// records the last requested agent id (for skill-routing assertions).
    struct MockExecutor {
        result: String,
        delay: Duration,
        agent_ids: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl TaskExecutor for MockExecutor {
        async fn execute(&self, _text: String, agent_id: Option<String>) -> Result<String, String> {
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            if let Ok(mut ids) = self.agent_ids.lock() {
                ids.push(agent_id.unwrap_or_else(|| "orchestrator".to_string()));
            }
            Ok(self.result.clone())
        }
    }

    fn mock_executor(delay_ms: u64) -> Arc<dyn TaskExecutor> {
        Arc::new(MockExecutor {
            result: "mock result".to_string(),
            delay: Duration::from_millis(delay_ms),
            agent_ids: std::sync::Arc::new(std::sync::Mutex::new(vec![])),
        })
    }

    /// Like `mock_executor` but exposes the recorded agent ids for assertions.
    fn mock_executor_recording(
        delay_ms: u64,
    ) -> (
        Arc<dyn TaskExecutor>,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        let agent_ids = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let exec = Arc::new(MockExecutor {
            result: "mock result".to_string(),
            delay: Duration::from_millis(delay_ms),
            agent_ids: agent_ids.clone(),
        });
        (exec, agent_ids)
    }

    fn test_skills() -> Vec<Skill> {
        agents_to_skills(&default_agents())
    }

    fn new_test_state(
        executor: Arc<dyn TaskExecutor>,
        token: Option<String>,
    ) -> Arc<A2aServerState> {
        Arc::new(A2aServerState::new(
            "NeoCoder",
            "http://127.0.0.1:0/a2a",
            token,
            test_skills(),
            executor,
        ))
    }

    async fn spawn_server(state: Arc<A2aServerState>) -> String {
        let router = build_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://{}", addr)
    }

    async fn rpc_post(base: &str, method: &str, params: Value, token: Option<&str>) -> Value {
        let client = reqwest::Client::new();
        let mut req = client
            .post(format!("{}/a2a", base))
            .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }));
        if let Some(t) = token {
            req = req.header("Authorization", format!("Bearer {}", t));
        }
        let resp = req.send().await.unwrap();
        resp.json().await.unwrap()
    }

    fn assert_rpc_err(resp: &Value, expected_code: i32) {
        let err = resp.get("error").expect("expected error, got result");
        assert_eq!(err["code"], expected_code, "{}", resp);
    }

    #[tokio::test]
    async fn test_agent_card_endpoint() {
        let state = new_test_state(mock_executor(0), None);
        let base = spawn_server(state).await;
        let resp = reqwest::get(format!("{}/.well-known/agent.json", base))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let card: Value = resp.json().await.unwrap();
        assert_eq!(card["name"], "NeoCoder");
        assert!(card["capabilities"]["streaming"].as_bool().unwrap());
        let skills = card["skills"].as_array().unwrap();
        assert_eq!(skills.len(), default_agents().len());
        for (i, agent) in default_agents().iter().enumerate() {
            assert_eq!(skills[i]["id"], agent.id);
        }
    }

    #[tokio::test]
    async fn test_message_send_creates_task() {
        let state = new_test_state(mock_executor(0), None);
        let base = spawn_server(state).await;
        let resp = rpc_post(
            &base,
            "message/send",
            json!({ "message": { "role": "user", "parts": [{ "kind": "text", "text": "hello" }] } }),
            None,
        )
        .await;
        assert!(resp.get("error").is_none(), "{}", resp);
        let task = &resp["result"];
        assert!(task["id"].as_str().unwrap().starts_with("a2a-"));
        let state_name = task["status"]["state"].as_str().unwrap();
        assert!(
            ["submitted", "working", "completed"].contains(&state_name),
            "{}",
            state_name
        );
    }

    #[test]
    fn test_extract_skill_id_none_when_absent() {
        let skills = test_skills();
        // 无 params → None
        assert_eq!(extract_skill_id(&None, &skills).unwrap(), None);
        // params 无 metadata.skillId → None
        let params = json!({ "message": { "role": "user", "parts": [] } });
        assert_eq!(
            extract_skill_id(&Some(params.clone()), &skills).unwrap(),
            None
        );
        // 空字符串 → None
        let empty = json!({ "metadata": { "skillId": "" } });
        assert_eq!(extract_skill_id(&Some(empty), &skills).unwrap(), None);
    }

    #[test]
    fn test_extract_skill_id_metadata_and_top_level() {
        let skills = test_skills();
        let meta = json!({ "metadata": { "skillId": "code_writer" } });
        assert_eq!(
            extract_skill_id(&Some(meta), &skills).unwrap(),
            Some("code_writer".to_string())
        );
        // 顶层 skillId 也接受（宽松兼容）
        let top = json!({ "skillId": "debugger" });
        assert_eq!(
            extract_skill_id(&Some(top), &skills).unwrap(),
            Some("debugger".to_string())
        );
        // metadata 优先于顶层
        let both = json!({ "metadata": { "skillId": "reviewer" }, "skillId": "debugger" });
        assert_eq!(
            extract_skill_id(&Some(both), &skills).unwrap(),
            Some("reviewer".to_string())
        );
    }

    #[test]
    fn test_extract_skill_id_unknown_rejected() {
        let skills = test_skills();
        let bad = json!({ "metadata": { "skillId": "ghost_agent" } });
        let err = extract_skill_id(&Some(bad), &skills).unwrap_err();
        assert!(
            err.to_string().contains("unknown skill 'ghost_agent'"),
            "{}",
            err
        );
        assert!(
            err.to_string().contains("orchestrator"),
            "available list: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_message_send_routes_to_requested_skill() {
        let (executor, agent_ids) = mock_executor_recording(0);
        let state = new_test_state(executor, None);
        let base = spawn_server(state).await;

        // metadata.skillId 指定 code_writer → executor 收到对应 agent
        let resp = rpc_post(
            &base,
            "message/send",
            json!({
                "message": { "role": "user", "parts": [{ "kind": "text", "text": "write tests" }] },
                "metadata": { "skillId": "code_writer" }
            }),
            None,
        )
        .await;
        assert!(resp.get("error").is_none(), "{}", resp);
        let id = resp["result"]["id"].as_str().unwrap().to_string();

        // 等终态后断言记录
        for _ in 0..50 {
            let get = rpc_post(&base, "tasks/get", json!({ "id": id }), None).await;
            if get["result"]["status"]["state"].as_str() == Some("completed") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let ids = agent_ids.lock().unwrap();
        assert_eq!(ids.last(), Some(&"code_writer".to_string()));
    }

    #[tokio::test]
    async fn test_message_send_unknown_skill_rejected() {
        let state = new_test_state(mock_executor(0), None);
        let base = spawn_server(state).await;
        let resp = rpc_post(
            &base,
            "message/send",
            json!({
                "message": { "role": "user", "parts": [{ "kind": "text", "text": "x" }] },
                "metadata": { "skillId": "ghost_agent" }
            }),
            None,
        )
        .await;
        assert_rpc_err(&resp, -32602);
    }

    #[tokio::test]
    async fn test_tasks_get_known_task() {
        let state = new_test_state(mock_executor(0), None);
        let base = spawn_server(state).await;
        let resp = rpc_post(
            &base,
            "message/send",
            json!({ "message": { "role": "user", "parts": [{ "kind": "text", "text": "hi" }] } }),
            None,
        )
        .await;
        let task_id = resp["result"]["id"].as_str().unwrap().to_string();

        // 轮询直到终态，再 tasks/get 验证内容
        let mut final_state = None;
        for _ in 0..20 {
            let get = rpc_post(&base, "tasks/get", json!({ "id": task_id }), None).await;
            let s = get["result"]["status"]["state"]
                .as_str()
                .unwrap()
                .to_string();
            if s == "completed" {
                final_state = Some(s);
                let result_text = get["result"]["artifacts"][0]["parts"][0]["text"]
                    .as_str()
                    .unwrap();
                assert_eq!(result_text, "mock result");
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(final_state.as_deref(), Some("completed"));
    }

    #[tokio::test]
    async fn test_tasks_get_unknown_task_returns_error() {
        let state = new_test_state(mock_executor(0), None);
        let base = spawn_server(state).await;
        let resp = rpc_post(&base, "tasks/get", json!({ "id": "nope" }), None).await;
        assert_rpc_err(&resp, -32002);
    }

    #[tokio::test]
    async fn test_tasks_cancel_idempotent() {
        let state = new_test_state(mock_executor(200), None);
        let base = spawn_server(state).await;
        let resp = rpc_post(
            &base,
            "message/send",
            json!({ "message": { "role": "user", "parts": [{ "kind": "text", "text": "slow task" }] } }),
            None,
        )
        .await;
        let task_id = resp["result"]["id"].as_str().unwrap().to_string();

        for _ in 0..2 {
            let cancel = rpc_post(&base, "tasks/cancel", json!({ "id": task_id }), None).await;
            assert!(cancel.get("error").is_none(), "{}", cancel);
            assert_eq!(cancel["result"]["status"]["state"], "canceled");
        }
    }

    #[tokio::test]
    async fn test_token_required_rejects() {
        let state = new_test_state(mock_executor(0), Some("sekrit".into()));
        let base = spawn_server(state).await;

        // 无 token → -32001
        let resp = rpc_post(
            &base,
            "message/send",
            json!({ "message": { "role": "user", "parts": [{ "kind": "text", "text": "x" }] } }),
            None,
        )
        .await;
        assert_rpc_err(&resp, -32001);

        // 错误 token → -32001
        let resp = rpc_post(
            &base,
            "message/send",
            json!({ "message": { "role": "user", "parts": [{ "kind": "text", "text": "x" }] } }),
            Some("wrong"),
        )
        .await;
        assert_rpc_err(&resp, -32001);

        // 正确 token → 通过
        let resp = rpc_post(
            &base,
            "message/send",
            json!({ "message": { "role": "user", "parts": [{ "kind": "text", "text": "x" }] } }),
            Some("sekrit"),
        )
        .await;
        assert!(resp.get("error").is_none(), "{}", resp);
    }

    #[tokio::test]
    async fn test_token_absent_accepts() {
        let state = new_test_state(mock_executor(0), None);
        let base = spawn_server(state).await;
        let resp = rpc_post(
            &base,
            "message/send",
            json!({ "message": { "role": "user", "parts": [{ "kind": "text", "text": "x" }] } }),
            None,
        )
        .await;
        assert!(resp.get("error").is_none(), "{}", resp);
    }

    #[tokio::test]
    async fn test_unknown_method_returns_method_not_found() {
        let state = new_test_state(mock_executor(0), None);
        let base = spawn_server(state).await;
        let resp = rpc_post(&base, "bogus/method", json!({}), None).await;
        assert_rpc_err(&resp, -32601);
    }

    #[tokio::test]
    async fn test_invalid_json_body() {
        let state = new_test_state(mock_executor(0), None);
        let base = spawn_server(state).await;
        let resp = reqwest::Client::new()
            .post(format!("{}/a2a", base))
            .body("this is not json {{{")
            .header("content-type", "application/json")
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
        let body: Value = resp.json().await.unwrap();
        assert_rpc_err(&body, -32602);
    }

    #[tokio::test]
    async fn test_unknown_route_404() {
        let state = new_test_state(mock_executor(0), None);
        let base = spawn_server(state).await;
        let resp = reqwest::get(format!("{}/nope", base)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_task_state_transition_mapping() {
        assert_eq!(
            map_cloud_status(&CloudTaskStatus::Pending),
            TaskState::Submitted
        );
        assert_eq!(
            map_cloud_status(&CloudTaskStatus::Running),
            TaskState::Working
        );
        assert_eq!(
            map_cloud_status(&CloudTaskStatus::Completed),
            TaskState::Completed
        );
        assert_eq!(
            map_cloud_status(&CloudTaskStatus::Failed("x".into())),
            TaskState::Failed
        );
        assert_eq!(
            map_cloud_status(&CloudTaskStatus::Cancelled),
            TaskState::Canceled
        );
    }

    #[tokio::test]
    async fn test_sse_stream_emits_updates() {
        let state = new_test_state(mock_executor(100), None);
        let base = spawn_server(state).await;
        let resp = rpc_post(
            &base,
            "message/send",
            json!({ "message": { "role": "user", "parts": [{ "kind": "text", "text": "stream me" }] } }),
            None,
        )
        .await;
        let task_id = resp["result"]["id"].as_str().unwrap().to_string();

        // resubscribe 读 SSE，直到看到 completed（或超时）
        let client = reqwest::Client::new();
        let stream_resp = client
            .post(format!("{}/a2a", base))
            .json(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tasks/resubscribe", "params": { "id": task_id } }))
            .send()
            .await
            .unwrap();
        assert!(stream_resp.status().is_success());

        let mut all_text = String::new();
        let mut bytes = stream_resp.bytes_stream();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk.unwrap();
            all_text.push_str(&String::from_utf8_lossy(&chunk));
            if all_text.contains("\"completed\"") || tokio::time::Instant::now() >= deadline {
                break;
            }
        }

        // 每个 task_update 事件的 data 可解析为 Task
        let events = parse_sse_events(&all_text);
        assert!(!events.is_empty(), "sse text: {}", all_text);
        for data in &events {
            let task: Task = serde_json::from_str(data)
                .unwrap_or_else(|e| panic!("event data not a Task ({}): {}", e, data));
            assert_eq!(task.id, task_id);
        }
        assert!(
            events.iter().any(|d| d.contains("\"completed\"")),
            "no completed event: {}",
            all_text
        );
    }

    #[tokio::test]
    async fn test_concurrent_tasks_independent() {
        let state = new_test_state(mock_executor(30), None);
        let base = spawn_server(state).await;

        // 并发创建 3 个任务
        let mut ids = Vec::new();
        for i in 0..3 {
            let resp = rpc_post(
                &base,
                "message/send",
                json!({ "message": { "role": "user", "parts": [{ "kind": "text", "text": format!("task {}", i) }] } }),
                None,
            )
            .await;
            let id = resp["result"]["id"].as_str().unwrap().to_string();
            assert!(!ids.contains(&id), "duplicate task id: {}", id);
            ids.push(id);
        }
        assert_eq!(ids.len(), 3);

        // 各自轮询到终态，互不串扰
        for id in &ids {
            let mut done = false;
            for _ in 0..50 {
                let get = rpc_post(&base, "tasks/get", json!({ "id": id }), None).await;
                let s = get["result"]["status"]["state"]
                    .as_str()
                    .unwrap()
                    .to_string();
                if s == "completed" {
                    assert_eq!(
                        get["result"]["artifacts"][0]["parts"][0]["text"],
                        "mock result"
                    );
                    done = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(done, "task {} did not complete", id);
        }
    }

    #[tokio::test]
    async fn test_oversized_message_rejected() {
        let state = new_test_state(mock_executor(0), None);
        let base = spawn_server(state).await;
        let huge = "x".repeat(MAX_MESSAGE_BYTES + 1);
        let resp = rpc_post(
            &base,
            "message/send",
            json!({ "message": { "role": "user", "parts": [{ "kind": "text", "text": huge }] } }),
            None,
        )
        .await;
        assert_rpc_err(&resp, -32602);
    }

    #[tokio::test]
    async fn test_malformed_task_id_handled() {
        let state = new_test_state(mock_executor(0), None);
        let base = spawn_server(state).await;
        // 非法字符 taskId 安全返回 JSON-RPC 错误，不 panic
        let resp = rpc_post(
            &base,
            "tasks/get",
            json!({ "id": "bad id with spaces/../\\" }),
            None,
        )
        .await;
        assert_rpc_err(&resp, -32002);
    }

    #[tokio::test]
    async fn test_message_send_missing_text_rejected() {
        let state = new_test_state(mock_executor(0), None);
        let base = spawn_server(state).await;
        // 无文本 part
        let resp = rpc_post(
            &base,
            "message/send",
            json!({ "message": { "role": "user", "parts": [] } }),
            None,
        )
        .await;
        assert_rpc_err(&resp, -32602);
    }
}
