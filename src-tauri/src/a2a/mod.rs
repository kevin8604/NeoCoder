//! A2A (Agent2Agent) v1.0 protocol support.
//!
//! Implements the core protocol models (Agent Card, Task/Message/Part/Artifact),
//! JSON-RPC 2.0 message envelope, and error code mapping. The client and server
//! modules build on these shared types.

pub mod client;
pub mod server;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Client-side configuration of a remote A2A agent (persisted in AppSettings).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct A2aAgentConfig {
    pub name: String,
    pub url: String,
    pub description: String,
}

impl Default for A2aAgentConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            url: String::new(),
            description: String::new(),
        }
    }
}

/// Agent Card — describes an A2A agent's identity, capabilities and skills.
/// Served at `/.well-known/agent.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    pub name: String,
    pub description: String,
    pub url: String,
    pub version: String,
    #[serde(default)]
    pub capabilities: AgentCapabilities,
    #[serde(default)]
    pub authentication: Option<AgentAuthentication>,
    #[serde(default)]
    pub default_input_modes: Vec<String>,
    #[serde(default)]
    pub default_output_modes: Vec<String>,
    #[serde(default)]
    pub skills: Vec<Skill>,
}

/// Protocol capabilities advertised by an agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub push_notifications: bool,
    #[serde(default)]
    pub state_transition_history: bool,
}

impl Default for AgentCapabilities {
    fn default() -> Self {
        Self {
            streaming: false,
            push_notifications: false,
            state_transition_history: false,
        }
    }
}

/// Authentication schemes an agent accepts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentAuthentication {
    pub schemes: Vec<String>,
}

/// A skill an agent can perform (mapped from AgentRegistry definitions).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Task state machine per A2A v1.0 (kebab-case wire representation).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    Submitted,
    Working,
    Completed,
    Failed,
    Canceled,
    InputRequired,
    AuthRequired,
}

/// Status block of a Task: state + optional message + timestamp.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    pub state: TaskState,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
}

impl TaskStatus {
    pub fn new(state: TaskState) -> Self {
        Self {
            state,
            message: None,
            timestamp: Some(chrono::Utc::now().to_rfc3339()),
        }
    }

    pub fn with_message(state: TaskState, message: impl Into<String>) -> Self {
        Self {
            state,
            message: Some(message.into()),
            timestamp: Some(chrono::Utc::now().to_rfc3339()),
        }
    }
}

/// A2A Task — the unit of work exchanged between agents.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub status: TaskStatus,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    #[serde(default)]
    pub history: Vec<Message>,
    #[serde(default)]
    pub metadata: Option<Value>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl Task {
    pub fn new(id: impl Into<String>, status: TaskStatus) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id: id.into(),
            status,
            artifacts: Vec::new(),
            history: Vec::new(),
            metadata: None,
            created_at: Some(now.clone()),
            updated_at: Some(now),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.state,
            TaskState::Completed | TaskState::Failed | TaskState::Canceled
        )
    }
}

/// Message role: the sender of a message.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Agent,
}

/// A message exchanged within a task (history) or sent to an agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub role: MessageRole,
    pub parts: Vec<Part>,
    #[serde(default)]
    pub message_id: Option<String>,
}

impl Message {
    pub fn text(role: MessageRole, text: impl Into<String>) -> Self {
        Self {
            role,
            parts: vec![Part::Text { text: text.into() }],
            message_id: None,
        }
    }

    /// Concatenated text of all Text parts (used to extract the task payload).
    pub fn text_content(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A message part: text, file (base64 bytes) or structured data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Part {
    Text {
        text: String,
    },
    File {
        name: String,
        #[serde(default)]
        mime_type: Option<String>,
        #[serde(default)]
        bytes: Option<String>,
    },
    Data {
        data: Value,
    },
}

/// A task artifact — a named output produced by the agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub name: String,
    pub parts: Vec<Part>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

impl Artifact {
    /// Concatenated text of the artifact's Text parts.
    pub fn text_content(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn new(id: Value, method: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params: None,
        }
    }

    pub fn with_params(mut self, params: Value) -> Self {
        self.params = Some(params);
        self
    }
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// JSON-RPC 2.0 response envelope (result XOR error).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl JsonRpcResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Value, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(error),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

/// A2A error type with JSON-RPC error-code mapping.
#[derive(Debug, Clone, thiserror::Error)]
pub enum A2aError {
    #[error("Method not found: {0}")]
    MethodNotFound(String),
    #[error("Invalid params: {0}")]
    InvalidParams(String),
    #[error("Authentication required: {0}")]
    AuthRequired(String),
    #[error("Task not found: {0}")]
    TaskNotFound(String),
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("Transport error: {0}")]
    Transport(String),
    #[error("Remote error ({code}): {message}")]
    Remote { code: i32, message: String },
}

impl A2aError {
    /// JSON-RPC error code for this error.
    pub fn code(&self) -> i32 {
        match self {
            Self::MethodNotFound(_) => -32601,
            Self::InvalidParams(_) => -32602,
            Self::AuthRequired(_) => -32001,
            Self::TaskNotFound(_) => -32002,
            Self::Internal(_) | Self::Transport(_) => -32000,
            Self::Remote { code, .. } => *code,
        }
    }

    /// Convert to a JSON-RPC error object.
    pub fn to_rpc_error(&self) -> RpcError {
        RpcError {
            code: self.code(),
            message: self.to_string(),
            data: None,
        }
    }
}

/// Build an Agent Card for this NeoCoder instance.
/// Skills are derived from the agent registry (one skill per agent).
pub fn build_agent_card(
    name: &str,
    url: &str,
    token_required: bool,
    skills: Vec<Skill>,
) -> AgentCard {
    AgentCard {
        name: name.to_string(),
        description: "NeoCoder AI coding assistant - A2A interoperable agent".to_string(),
        url: url.to_string(),
        version: "1.0.0".to_string(),
        capabilities: AgentCapabilities {
            streaming: true,
            push_notifications: false,
            state_transition_history: false,
        },
        authentication: if token_required {
            Some(AgentAuthentication {
                schemes: vec!["bearer".to_string()],
            })
        } else {
            None
        },
        default_input_modes: vec!["text".to_string(), "text/plain".to_string()],
        default_output_modes: vec!["text".to_string(), "text/plain".to_string()],
        skills,
    }
}

/// Map agent definitions to A2A skills.
pub fn agents_to_skills(agents: &[crate::agent::definition::AgentDefinition]) -> Vec<Skill> {
    agents
        .iter()
        .map(|a| Skill {
            id: a.id.clone(),
            name: a.name.clone(),
            description: a.description.clone(),
            tags: vec!["agent".to_string()],
        })
        .collect()
}

/// Summarize a completed task into a human-readable text block.
pub fn summarize_task(task: &Task) -> String {
    let mut out = format!("Task {} state: {:?}", task.id, task.status.state);
    if let Some(msg) = &task.status.message {
        out.push_str(&format!("\nMessage: {}", msg));
    }
    for artifact in &task.artifacts {
        let text = artifact.text_content();
        if !text.is_empty() {
            out.push_str(&format!("\n[artifact: {}]\n{}", artifact.name, text));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_card() -> AgentCard {
        AgentCard {
            name: "TestAgent".into(),
            description: "A test agent".into(),
            url: "http://127.0.0.1:9999/a2a".into(),
            version: "1.0.0".into(),
            capabilities: AgentCapabilities {
                streaming: true,
                push_notifications: false,
                state_transition_history: true,
            },
            authentication: Some(AgentAuthentication {
                schemes: vec!["bearer".into()],
            }),
            default_input_modes: vec!["text".into()],
            default_output_modes: vec!["text".into()],
            skills: vec![Skill {
                id: "sk1".into(),
                name: "Write Code".into(),
                description: "Writes code".into(),
                tags: vec!["code".into()],
            }],
        }
    }

    #[test]
    fn test_agent_card_roundtrip() {
        let card = sample_card();
        let json = serde_json::to_string(&card).unwrap();
        // 字段名使用 camelCase（A2A 规范）
        assert!(json.contains("\"defaultInputModes\""), "{}", json);
        assert!(json.contains("\"pushNotifications\""), "{}", json);
        let back: AgentCard = serde_json::from_str(&json).unwrap();
        assert_eq!(card, back);
    }

    #[test]
    fn test_agent_card_defaults() {
        // 缺失 capabilities/authentication/skills 时回退默认值
        let json = json!({
            "name": "Minimal",
            "description": "d",
            "url": "http://x/a2a",
            "version": "1.0"
        });
        let card: AgentCard = serde_json::from_value(json).unwrap();
        assert!(!card.capabilities.streaming);
        assert!(!card.capabilities.push_notifications);
        assert!(card.authentication.is_none());
        assert!(card.skills.is_empty());
        assert!(card.default_input_modes.is_empty());
    }

    #[test]
    fn test_task_state_serde_lowercase() {
        // kebab-case wire names
        assert_eq!(
            serde_json::to_string(&TaskState::Submitted).unwrap(),
            "\"submitted\""
        );
        assert_eq!(
            serde_json::to_string(&TaskState::Working).unwrap(),
            "\"working\""
        );
        assert_eq!(
            serde_json::to_string(&TaskState::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(serde_json::to_string(&TaskState::Failed).unwrap(), "\"failed\"");
        assert_eq!(
            serde_json::to_string(&TaskState::Canceled).unwrap(),
            "\"canceled\""
        );
        assert_eq!(
            serde_json::to_string(&TaskState::InputRequired).unwrap(),
            "\"input-required\""
        );
        assert_eq!(
            serde_json::to_string(&TaskState::AuthRequired).unwrap(),
            "\"auth-required\""
        );

        // Failed 携带错误消息 roundtrip
        let status = TaskStatus::with_message(TaskState::Failed, "boom");
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["state"], "failed");
        assert_eq!(json["message"], "boom");
        let back: TaskStatus = serde_json::from_value(json).unwrap();
        assert_eq!(back.state, TaskState::Failed);
        assert_eq!(back.message.as_deref(), Some("boom"));

        // 解析 kebab-case 字符串
        let state: TaskState = serde_json::from_str("\"input-required\"").unwrap();
        assert_eq!(state, TaskState::InputRequired);
    }

    #[test]
    fn test_task_roundtrip() {
        let mut task = Task::new("t1", TaskStatus::new(TaskState::Working));
        task.artifacts = vec![Artifact {
            name: "result.txt".into(),
            parts: vec![Part::Text { text: "done".into() }],
            metadata: None,
        }];
        task.history = vec![Message::text(MessageRole::User, "hello")];
        task.metadata = Some(json!({ "session": "s1" }));

        let json = serde_json::to_string(&task).unwrap();
        assert!(json.contains("\"createdAt\""), "{}", json);
        let back: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(task, back);
        assert!(back.created_at.is_some());
        assert!(back.updated_at.is_some());
        assert!(back.is_terminal() == false);
    }

    #[test]
    fn test_part_variants() {
        // Text
        let text: Part = serde_json::from_value(json!({"kind": "text", "text": "hi"})).unwrap();
        assert!(matches!(text, Part::Text { ref text } if text == "hi"));
        // File（base64 bytes）
        let file: Part = serde_json::from_value(json!({
            "kind": "file",
            "name": "a.txt",
            "mimeType": "text/plain",
            "bytes": "aGVsbG8="
        }))
        .unwrap();
        match file {
            Part::File { ref name, ref bytes, .. } => {
                assert_eq!(name, "a.txt");
                assert_eq!(bytes.as_deref(), Some("aGVsbG8="));
            }
            _ => panic!("expected file part"),
        }
        // Data
        let data: Part = serde_json::from_value(json!({"kind": "data", "data": {"x": 1}})).unwrap();
        assert!(matches!(data, Part::Data { .. }));

        // roundtrip 各变体
        for p in [text, file, data] {
            let v = serde_json::to_value(&p).unwrap();
            let kind = v["kind"].as_str().unwrap();
            assert!(matches!(kind, "text" | "file" | "data"), "{}", kind);
            let back: Part = serde_json::from_value(v).unwrap();
            assert_eq!(p, back);
        }
    }

    #[test]
    fn test_message_roles() {
        let user: MessageRole = serde_json::from_str("\"user\"").unwrap();
        let agent: MessageRole = serde_json::from_str("\"agent\"").unwrap();
        assert_eq!(user, MessageRole::User);
        assert_eq!(agent, MessageRole::Agent);
        assert!(serde_json::from_str::<MessageRole>("\"system\"").is_err());

        // text_content 提取
        let m = Message {
            role: MessageRole::User,
            parts: vec![
                Part::Text { text: "line1".into() },
                Part::Data { data: json!({"a": 1}) },
                Part::Text { text: "line2".into() },
            ],
            message_id: None,
        };
        assert_eq!(m.text_content(), "line1\nline2");
    }

    #[test]
    fn test_artifact_roundtrip() {
        let artifact = Artifact {
            name: "out".into(),
            parts: vec![Part::Text { text: "data".into() }],
            metadata: Some(json!({"lang": "rust"})),
        };
        let v = serde_json::to_value(&artifact).unwrap();
        assert_eq!(v["name"], "out");
        assert_eq!(v["metadata"]["lang"], "rust");
        let back: Artifact = serde_json::from_value(v).unwrap();
        assert_eq!(artifact, back);
        assert_eq!(back.text_content(), "data");
    }

    #[test]
    fn test_jsonrpc_request_format() {
        let req = JsonRpcRequest::new(json!(1), "tasks/get").with_params(json!({"id": "t1"}));
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(v["method"], "tasks/get");
        assert_eq!(v["params"]["id"], "t1");

        // 无 params 时省略
        let req2 = JsonRpcRequest::new(json!(2), "tasks/get");
        let v2 = serde_json::to_value(&req2).unwrap();
        assert!(v2.get("params").is_none(), "{}", v2);
    }

    #[test]
    fn test_jsonrpc_error_codes() {
        let cases = [
            (A2aError::MethodNotFound("x".into()), -32601),
            (A2aError::InvalidParams("x".into()), -32602),
            (A2aError::Internal("x".into()), -32000),
            (A2aError::AuthRequired("x".into()), -32001),
            (A2aError::TaskNotFound("x".into()), -32002),
        ];
        for (err, expected) in cases {
            assert_eq!(err.code(), expected);
            let rpc = err.to_rpc_error();
            let v = serde_json::to_value(&rpc).unwrap();
            assert_eq!(v["code"], expected);
            assert!(v["message"].as_str().is_some());
        }
    }

    #[test]
    fn test_jsonrpc_success_response() {
        // result + 无 error
        let ok = JsonRpcResponse::ok(json!(1), json!({"taskId": "t1"}));
        assert!(ok.is_ok());
        let v = serde_json::to_value(&ok).unwrap();
        assert_eq!(v["result"]["taskId"], "t1");
        assert!(v.get("error").is_none());
        // error 为 null 或缺省均可解析
        let with_null: JsonRpcResponse =
            serde_json::from_value(json!({"jsonrpc": "2.0", "id": 1, "result": {}, "error": null}))
                .unwrap();
        assert!(with_null.is_ok());
        let minimal: JsonRpcResponse =
            serde_json::from_value(json!({"jsonrpc": "2.0", "id": 1, "result": {"a": 1}}))
                .unwrap();
        assert!(minimal.is_ok());
    }

    #[test]
    fn test_part_unknown_kind_rejected() {
        // 未知 kind 应报错而非静默忽略
        let result: Result<Part, _> = serde_json::from_value(json!({"kind": "audio", "text": "x"}));
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_message_parts() {
        // parts 为空数组合法
        let m: Message =
            serde_json::from_value(json!({"role": "user", "parts": []})).unwrap();
        assert!(m.parts.is_empty());
        // 缺 parts 字段报错
        assert!(serde_json::from_value::<Message>(json!({"role": "user"})).is_err());
    }

    #[test]
    fn test_task_without_id_rejected() {
        // id 必填
        let result: Result<Task, _> =
            serde_json::from_value(json!({"status": {"state": "working"}}));
        assert!(result.is_err());
    }

    #[test]
    fn test_a2a_agent_config_serde() {
        let cfg = A2aAgentConfig {
            name: "remote".into(),
            url: "http://127.0.0.1:9999".into(),
            description: "d".into(),
        };
        let v = serde_json::to_value(&cfg).unwrap();
        let back: A2aAgentConfig = serde_json::from_value(v).unwrap();
        assert_eq!(cfg, back);
        // 空对象回退默认值
        let empty: A2aAgentConfig = serde_json::from_value(json!({})).unwrap();
        assert_eq!(empty.name, "");
    }

    #[test]
    fn test_build_agent_card_and_summarize() {
        let card = build_agent_card(
            "NeoCoder",
            "http://127.0.0.1:41234/a2a",
            true,
            vec![Skill {
                id: "orchestrator".into(),
                name: "Orchestrator".into(),
                description: "Master".into(),
                tags: vec![],
            }],
        );
        assert_eq!(card.name, "NeoCoder");
        assert!(card.capabilities.streaming);
        assert_eq!(
            card.authentication.as_ref().unwrap().schemes,
            vec!["bearer".to_string()]
        );
        assert_eq!(card.skills[0].id, "orchestrator");

        let mut task = Task::new("t1", TaskStatus::with_message(TaskState::Completed, "ok"));
        task.artifacts = vec![Artifact {
            name: "result".into(),
            parts: vec![Part::Text { text: "hello world".into() }],
            metadata: None,
        }];
        let summary = summarize_task(&task);
        assert!(summary.contains("Completed"), "{}", summary);
        assert!(summary.contains("hello world"));
    }
}
