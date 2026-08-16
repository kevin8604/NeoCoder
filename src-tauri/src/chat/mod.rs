use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Optional base64-encoded images (data:image/...;base64,...)
    #[serde(default)]
    pub images: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Role {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "assistant")]
    Assistant,
    #[serde(rename = "system")]
    System,
    #[serde(rename = "tool")]
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub context: ChatContext,
    pub mode: ChatMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatContext {
    pub active_file: Option<String>,
    pub selected_code: Option<String>,
    pub file_mentions: Vec<String>,
    pub symbol_mentions: Vec<String>,
    pub include_codebase: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatMode {
    Ask,
    Edit,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChatEvent {
    Started {
        session_id: String,
        agent_id: Option<String>,
    },
    Delta {
        session_id: String,
        agent_id: Option<String>,
        token: String,
    },
    Finished {
        session_id: String,
        agent_id: Option<String>,
        full_text: String,
    },
    ToolCall {
        session_id: String,
        agent_id: Option<String>,
        tool_call: ToolCall,
    },
    ToolResult {
        session_id: String,
        agent_id: Option<String>,
        result: String,
        duration_ms: u64,
    },
    ToolRetry {
        session_id: String,
        agent_id: Option<String>,
        tool_name: String,
        attempt: u32,
        error: String,
    },
    TodoUpdate {
        session_id: String,
        agent_id: Option<String>,
        todos: Vec<TodoItem>,
    },
    AskUserQuestion {
        session_id: String,
        agent_id: Option<String>,
        question_id: String,
        questions: Vec<QuestionItem>,
    },
    AgentStatus {
        session_id: String,
        agent_id: Option<String>,
        status: String,
        iteration: u32,
        total_iterations: u32,
        estimated_tokens: u32,
        elapsed_ms: u64,
    },
    AgentThinking {
        session_id: String,
        agent_id: Option<String>,
        thought: String,
    },
    ContextTrimmed {
        session_id: String,
        agent_id: Option<String>,
        trimmed_count: u32,
        total_before: u32,
        total_after: u32,
    },
    AgentLog {
        session_id: String,
        agent_id: Option<String>,
        level: String,
        message: String,
    },
    EditDiff {
        session_id: String,
        agent_id: Option<String>,
        changes: Vec<FileChange>,
    },
    ConfirmRequest {
        session_id: String,
        agent_id: Option<String>,
        confirm_id: String,
        tool_name: String,
        description: String,
    },
    Cancelled {
        session_id: String,
        agent_id: Option<String>,
    },
    Error {
        session_id: String,
        agent_id: Option<String>,
        message: String,
    },
    FileRestored {
        session_id: String,
        agent_id: Option<String>,
        file_path: String,
        content: String,
    },
    CheckpointCreated {
        session_id: String,
        agent_id: Option<String>,
        iteration: u32,
        commit_hash: Option<String>,
        files: Vec<String>,
    },
    BudgetExhausted {
        session_id: String,
        agent_id: Option<String>,
        summary: String,
        max_iterations: u32,
    },
    PlanCreated {
        plan: PlanCreate,
    },
    PlanApproved {
        plan: PlanApproved,
    },
    PlanRejected {
        plan: PlanRejected,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub file_path: String,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    #[serde(rename = "type")]
    pub hunk_type: String,
    pub content: String,
    pub old_start: u32,
    pub new_start: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionItem {
    pub header: String,
    pub question: String,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanCreate {
    pub session_id: String,
    pub agent_id: Option<String>,
    pub plan_summary: String,
    pub plan_steps: Vec<PlanStep>,
    pub affected_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub order: u32,
    pub description: String,
    pub file_path: Option<String>,
    pub tool_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanApproved {
    pub session_id: String,
    pub agent_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRejected {
    pub session_id: String,
    pub agent_id: Option<String>,
    pub reason: Option<String>,
}

/// Conversation memory manager — backed by Markdown files via MemoryManager.
pub struct ConversationMemory {
    manager: std::sync::Arc<crate::memory::MemoryManager>,
}

pub struct ChatSession {
    pub id: String,
    pub title: String,
    pub messages: VecDeque<ChatMessage>,
    pub message_count: usize,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl ConversationMemory {
    pub fn new(base_dir: std::path::PathBuf) -> Self {
        Self {
            manager: std::sync::Arc::new(crate::memory::MemoryManager::new(base_dir)),
        }
    }

    /// Compatibility shim: accepts sessions_dir but uses its parent as memory base dir.
    pub fn with_storage(storage_dir: std::path::PathBuf) -> Self {
        let base = storage_dir
            .parent()
            .map(|p| p.join("memory"))
            .unwrap_or_else(|| {
                let mut p = storage_dir;
                p.pop();
                p.join("memory")
            });
        Self::new(base)
    }

    pub fn create_session(&self) -> String {
        self.manager.create_session().unwrap_or_else(|e| {
            log::error!("Failed to create session: {}", e);
            uuid::Uuid::new_v4().to_string()
        })
    }

    pub fn add_message(&self, session_id: &str, message: ChatMessage) {
        if let Err(e) = self.manager.add_message(session_id, message) {
            log::error!("Failed to add message: {}", e);
        }
    }

    pub fn get_context_window(&self, session_id: &str, max_tokens: usize) -> Vec<ChatMessage> {
        self.manager
            .get_context_window(session_id, max_tokens)
            .unwrap_or_default()
    }

    pub fn get_all_sessions(&self) -> Vec<ChatSession> {
        self.manager.get_all_sessions().unwrap_or_default()
    }

    pub fn clear_session(&self, session_id: &str) {
        if let Err(e) = self.manager.clear_session(session_id) {
            log::error!("Failed to clear session: {}", e);
        }
    }

    pub fn delete_session(&self, session_id: &str) {
        if let Err(e) = self.manager.delete_session(session_id) {
            log::error!("Failed to delete session: {}", e);
        }
    }

    /// Expose the underlying MemoryManager for memory injection/flush.
    pub fn memory_manager(&self) -> std::sync::Arc<crate::memory::MemoryManager> {
        self.manager.clone()
    }
}

/// System prompt for Chat
pub const CHAT_SYSTEM_PROMPT: &str = "You are an expert AI coding assistant integrated into a code editor. \
Your name is NeoCoder. You help developers write, understand, debug, and refactor code.

Guidelines:
1. Provide concise, accurate answers with code examples when relevant
2. Format code blocks with the appropriate language identifier for syntax highlighting
3. When suggesting changes, explain the reasoning briefly and cite exact file paths (and line numbers when known)
4. In Ask mode you are primarily explanatory: describe what to change and show snippets, but do not attempt to modify files
5. Prioritize best practices and idiomatic code patterns
6. If you don't know something, say so honestly rather than guessing
7. For security, never suggest running arbitrary or destructive code without user review

When in Edit mode, provide file changes as diff-like descriptions.
When in Agent mode, use tools to gather information and make changes.";
