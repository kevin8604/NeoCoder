use crate::chat::{QuestionItem, TodoItem};
use crate::lsp::LspManager;
use crate::rag::CodeIndexer;
use crate::sandbox::SandboxChecker;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

// 工具模块声明
pub mod a2a_invoke;
pub mod append_file;
pub mod ask_user_question;
pub mod auto_fix;
pub mod coverage;
pub mod create_directory;
pub mod delete_directory;
pub mod delete_file;
pub mod edit;
pub mod generate_diagram;
pub mod generate_tests;
pub mod get_diagnostics;
pub mod get_symbols;
pub mod git_blame;
pub mod git_branch;
pub mod git_checkout;
pub mod git_commit;
pub mod git_diff;
pub mod git_log;
pub mod git_push;
pub mod git_stash;
pub mod git_status;
pub mod glob;
pub mod grep;
pub mod list_directory;
pub mod memory_search;
pub mod orchestrate;
pub mod read_file;
pub mod run_build;
pub mod run_terminal_command;
pub mod run_terminal_session;
pub mod run_tests;
pub mod search_codebase;
pub mod tdd;
pub mod todo_write;
pub mod web_browser;
pub mod web_fetch;
pub mod web_preview;
pub mod web_search;
pub mod write_file;

#[cfg(test)]
mod tests;

/// 工具执行上下文 —— 所有工具共享的运行时环境
#[derive(Clone)]
pub struct ToolContext {
    pub project_path: Option<String>,
    pub indexer: Option<Arc<CodeIndexer>>,
    /// Sandbox security checker for path/command/URL validation
    pub sandbox: Arc<SandboxChecker>,
    /// LSP manager for precise symbol resolution (optional; tools fall back to heuristics)
    pub lsp_manager: Option<Arc<LspManager>>,
    /// Optional app handle for tools that need to emit events
    pub app_handle: Option<tauri::AppHandle>,
    /// Optional session ID for audit and event correlation
    pub session_id: Option<String>,
    /// Tavily Search API key (empty = use fallback)
    pub tavily_api_key: String,
    // ── P4: LLM config for AI-assisted edit fallback ─
    pub llm_provider: crate::config::LlmProvider,
    pub llm_api_key: String,
    pub llm_base_url: Option<String>,
    pub llm_model: String,
}

/// 子 Agent 调度任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchTask {
    pub agent_id: String,
    pub task: String,
    pub file_path: Option<String>,
    /// IDs of agents this task depends on (output injected as context)
    pub depends_on: Option<Vec<String>>,
    /// Run in background (fire-and-forget with completion notification)
    #[serde(default)]
    pub background: bool,
}

/// 工具执行后的特殊动作（避免字符串匹配）
pub enum PostExecuteAction {
    None,
    UpdateTodos(Vec<TodoItem>),
    AskUser(Vec<QuestionItem>),
    /// Dispatch a single sub-agent (serial)
    DispatchAgent {
        agent_id: String,
        task: String,
        /// Run in background (fire-and-forget with completion notification)
        background: bool,
    },
    /// Dispatch multiple sub-agents (parallel with conflict detection)
    DispatchAgents(Vec<DispatchTask>),
}

/// 工具 trait —— 每个工具实现此 trait
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// 工具名称，必须与 tools.json 中的 name 一致
    fn name(&self) -> &str;

    /// 执行工具逻辑，返回结果字符串
    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String;

    /// 返回执行后需要的特殊动作，默认无
    fn post_execute_action(&self, _args: &serde_json::Value) -> PostExecuteAction {
        PostExecuteAction::None
    }
}

/// Default timeout for tool execution in milliseconds.
pub const DEFAULT_TOOL_TIMEOUT_MS: u64 = 120_000; // 2 minutes

/// 工具执行器 —— 管理所有工具的注册和调度
pub struct ToolExecutor {
    tools: std::sync::Mutex<HashMap<String, Arc<dyn Tool>>>,
}

impl Default for ToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolExecutor {
    pub fn new() -> Self {
        Self {
            tools: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// 注册一个工具
    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        self.tools
            .get_mut()
            .unwrap()
            .insert(tool.name().to_string(), Arc::new(tool));
    }

    /// Register a dynamically-created tool (e.g., MCP bridge).
    pub fn register_raw(&self, name: String, tool: Arc<dyn Tool>) {
        self.tools.lock().unwrap().insert(name, tool);
    }

    /// 按名称查找工具
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.lock().unwrap().get(name).cloned()
    }

    /// 返回所有已注册工具的名称（用于 schema 一致性校验）
    pub fn registered_names(&self) -> Vec<String> {
        self.tools.lock().unwrap().keys().cloned().collect()
    }

    /// Execute a tool by name with timeout protection.
    /// Returns error string if not found, or timeout message if execution exceeds limit.
    pub async fn execute(&self, name: &str, args: serde_json::Value, ctx: &ToolContext) -> String {
        let tool = self.get(name);
        match tool {
            Some(tool) => {
                match tokio::time::timeout(
                    std::time::Duration::from_millis(DEFAULT_TOOL_TIMEOUT_MS),
                    tool.execute(args, ctx),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_elapsed) => {
                        format!(
                            "[TIMEOUT] Tool '{}' exceeded {}s limit and was cancelled",
                            name,
                            DEFAULT_TOOL_TIMEOUT_MS / 1000
                        )
                    }
                }
            }
            None => format!("Unknown tool: {}", name),
        }
    }

    /// 获取某工具的执行后动作
    pub fn post_execute_action(&self, name: &str, args: &serde_json::Value) -> PostExecuteAction {
        self.get(name)
            .map(|t| t.post_execute_action(args))
            .unwrap_or(PostExecuteAction::None)
    }
}

/// 构建注册了所有 17 个工具的 ToolExecutor
pub fn build_executor() -> ToolExecutor {
    let mut executor = ToolExecutor::new();
    executor.register(read_file::ReadFile);
    executor.register(write_file::WriteFile);
    executor.register(append_file::AppendFile);
    executor.register(delete_file::DeleteFile);
    executor.register(list_directory::ListDirectory);
    executor.register(create_directory::CreateDirectory);
    executor.register(delete_directory::DeleteDirectory);
    executor.register(get_symbols::GetSymbols);
    executor.register(search_codebase::SearchCodebase);
    executor.register(grep::Grep);
    executor.register(run_terminal_command::RunTerminalCommand);
    executor.register(glob::Glob);
    executor.register(edit::Edit);
    executor.register(todo_write::TodoWrite);
    executor.register(web_search::WebSearch);
    executor.register(web_fetch::WebFetch);
    executor.register(ask_user_question::AskUserQuestion);
    executor.register(get_diagnostics::GetDiagnostics);
    executor.register(orchestrate::DispatchAgent);
    executor.register(orchestrate::DispatchAgents);
    executor.register(memory_search::MemorySearch);
    executor.register(git_status::GitStatus);
    executor.register(git_diff::GitDiff);
    executor.register(git_commit::GitCommit);
    executor.register(git_log::GitLog);
    executor.register(git_blame::GitBlame);
    executor.register(git_branch::GitBranch);
    executor.register(git_push::GitPush);
    executor.register(git_checkout::GitCheckout);
    executor.register(git_stash::GitStash);
    executor.register(generate_diagram::GenerateDiagram);
    executor.register(run_tests::RunTests);
    executor.register(run_build::RunBuild);
    executor.register(run_terminal_session::RunTerminalSession);
    executor.register(web_preview::WebPreview);
    executor.register(web_browser::WebBrowser);
    executor.register(generate_tests::GenerateTests);
    executor.register(auto_fix::AutoFix);
    executor.register(tdd::TddTool);
    executor.register(coverage::CoverageTool);
    executor.register(a2a_invoke::A2aInvoke);
    executor
}
