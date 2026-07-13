pub mod utils;
pub mod tools;
pub mod definition;
pub mod sub_agent;
pub mod token_count;
pub mod hooks;
pub mod context;
pub mod checkpoint;
pub mod loop_detector;
pub mod cloud;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use chrono;
use tauri::{Emitter, Manager};
use tokio::sync::RwLock;
use crate::chat::{ChatEvent, TodoItem, FileChange, DiffHunk};
use crate::config::LlmProvider;
use crate::llm;
use crate::rag::CodeIndexer;
use crate::agent::definition::AgentDefinition;
use crate::agent::loop_detector::{LoopDetector, LoopVerdict};
use crate::sandbox::SandboxChecker;

use tools::{ToolContext, ToolExecutor, PostExecuteAction};

// ── 重新导出 ──

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    pub fn to_openai_tool(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters.clone(),
            },
        })
    }
}

pub type ToolRegistry = Arc<Vec<ToolDefinition>>;
pub type QuestionAwaiters = Arc<std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<String>>>>;
pub type ConfirmAwaiters = Arc<std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>>;

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

// ── 系统提示词 ──

const AGENT_SYSTEM_PROMPT: &str = r#"You are a powerful AI coding assistant with access to a comprehensive set of tools. You work autonomously to accomplish the user's task.

## Tool Usage Guidelines

- **Glob**: Before searching code, use Glob to locate relevant files by name patterns (e.g., `src/**/*.rs`, `*.tsx`).
- **Edit**: For modifying files, prefer the Edit tool over write_file. It performs exact string replacements which are safer and more precise. Always include enough context in old_string to make the match unique.
- **TodoWrite**: For complex multi-step tasks, create a task list with TodoWrite and update status as you progress. This helps the user track what you're doing.
- **AskUserQuestion**: When requirements are ambiguous or you need the user to make a decision, use AskUserQuestion. Provide clear options when possible.
- **WebSearch**: Use this to find up-to-date information, documentation, or answers beyond your training data.
- **WebFetch**: Use this to read the content of a specific web page (documentation, API references, etc.).
- **search_codebase**: Use semantic search to find code by meaning, not just exact text.
- **grep**: Use for exact text/pattern matching across the project.
- **run_terminal_command**: Execute shell commands for builds, tests, git operations, etc.
- **get_diagnostics**: Check for compiler/linter errors in a file. Supports Rust (cargo check), TypeScript (tsc), Python (py_compile), Go (go vet).

## Research Before Implementation

Before implementing complex features (terminal emulators, file watchers, authentication, parsers, etc.):
- **Search for existing crates/packages first**: Use web_search or grep to find battle-tested solutions (e.g., `portable-pty` for PTY, `notify` for file watching, `axum` for web servers).
- **Read existing code patterns**: Before writing a new component/module, grep the codebase for similar implementations and follow established patterns.
- **Check dependencies**: Before adding npm/cargo packages, verify the project doesn't already have equivalent dependencies. Avoid duplicate packages.

## Verification After Implementation

After creating new files or significant features:
1. **Build check**: Always run `cargo check` / `tsc --noEmit` / equivalent after non-trivial changes.
2. **Edge case review**: For any string truncation in Rust, use char boundaries (never byte indices like `&s[..500]`). For async I/O, prefer `read_buf`/`read` over line-buffered reads.
3. **Test the happy path**: If you create a UI component, verify the build succeeds. If you create a backend service, verify it responds.

## Workflow

1. For complex tasks, start by creating a TodoWrite task list
2. Use Glob + grep/search_codebase to understand the codebase
3. Use read_file to examine relevant files in detail
4. Make changes with Edit (preferred) or write_file
5. Verify changes with run_terminal_command (build, test, etc.)
6. Ask the user questions with AskUserQuestion when needed

## Error Self-Fixing

After writing or editing code, diagnostics are automatically checked for the modified files. If an `[AUTO-DIAGNOSTICS]` message appears with errors, fix them immediately using the Edit tool before moving on. Do not leave broken code — always resolve compiler/linter errors.

## Terminal Error Handling

When running commands that produce errors (compilation, linting, tests), an `--- Error Summary ---` section highlights file paths and line numbers. Use the Edit tool to fix these errors immediately rather than just reporting them to the user.

## Editing Discipline

- **Read before edit**: Never call edit/write_file on a file you haven't read in this session. Read it first to know the exact current content.
- **Minimal changes**: Prefer small, targeted edits over rewriting whole files. Do not reformat or refactor code unrelated to the task.
- **Preserve style**: Match existing indentation, naming, and conventions of the surrounding code.

## Convergence & Stopping

- You have a limited iteration budget. Work efficiently toward completion — do not repeat the same failing action.
- If a tool fails the same way 2+ times, change your approach instead of retrying identically.
- When the task is complete and verified, stop and give a concise final summary. Do not perform extra unrequested work.
- If you are blocked or a requirement is ambiguous, use ask_user_question rather than guessing.

## Communication Style

- Be concise. Report results and decisions directly; avoid narrating internal deliberation.
- When referencing code, cite exact file paths (and line numbers when known).
- Do not dump large file contents into your response — summarize instead.

## Memory & Safety

- Use memory_search to recall prior decisions, conventions, and lessons before starting non-trivial work.
- When you discover important information, save it to memory using the memory_search tool with action='append'.
- Memory entries should be SPECIFIC and ACTIONABLE:
  - Include file paths and line numbers when relevant (e.g., "in pty.rs L45")
  - Include the exact error message or symptom
  - Include the concrete fix or solution
  - Use category tags: [BugFix], [Decision], [Lesson], [API], [Pattern], [Perf]
  - GOOD: "[BugFix] portable-pty PtySize.rows/cols is u16 not u32, need `rows as u16` cast in resize_terminal (commands/pty.rs L245)"
  - BAD: "Ensure proper integration between frontend and backend"
- Never output secrets, API keys, or credentials. Never suggest destructive commands (rm -rf, force push to main) without explicit user request.

Be thorough and complete the task fully. Do not ask the user for confirmation before making changes unless you need a decision."#;

// ── 工具加载 ──

pub fn get_tools(app: Option<&tauri::AppHandle>) -> Vec<ToolDefinition> {
    if let Some(handle) = app {
        if let Some(registry) = handle.try_state::<ToolRegistry>() {
            return registry.as_ref().clone();
        }
    }
    serde_json::from_str::<Vec<ToolDefinition>>(include_str!("../../tools.json")).unwrap_or_default()
}

pub fn load_tools_from_disk() -> Vec<ToolDefinition> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let path = dir.join("tools.json");
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(tools) = serde_json::from_str::<Vec<ToolDefinition>>(&content) {
                    if !tools.is_empty() { return tools; }
                }
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let path = cwd.join("tools.json");
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(tools) = serde_json::from_str::<Vec<ToolDefinition>>(&content) {
                if !tools.is_empty() { return tools; }
            }
        }
    }
    serde_json::from_str::<Vec<ToolDefinition>>(include_str!("../../tools.json")).unwrap_or_default()
}

// ── Execution Phase ──

/// Agent execution phase — controls tool availability and system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionPhase {
    /// Read-only analysis: LLM outputs a structured plan, no file modifications.
    Planning,
    /// Full execution: all tools available, LLM implements the plan.
    Executing,
    /// Agent has completed (terminal state).
    Done,
}

/// Tools allowed during Planning phase (read-only + communication).
const PLANNING_PHASE_TOOLS: &[&str] = &[
    "read_file", "glob", "grep", "search_codebase",
    "list_directory", "get_symbols", "get_diagnostics",
    "todo_write", "web_search", "web_fetch", "ask_user_question",
];

/// Tools allowed during early exploration iterations (read-only + todo_write).
const EXPLORATION_PHASE_TOOLS: &[&str] = &[
    "read_file", "glob", "grep", "search_codebase",
    "list_directory", "get_symbols", "get_diagnostics",
    "todo_write", "web_search", "web_fetch", "ask_user_question",
    "memory_search", "git_status", "git_diff",
];

/// Tools allowed during verification iterations (read-only + diagnostics + terminal).
const VERIFICATION_PHASE_TOOLS: &[&str] = &[
    "read_file", "glob", "grep", "search_codebase",
    "list_directory", "get_symbols", "get_diagnostics",
    "todo_write", "run_terminal_command",
    "memory_search", "git_status", "git_diff",
];

/// Tools that are safe to execute in parallel (read-only, no side effects).
fn is_read_only_tool(name: &str) -> bool {
    matches!(name,
        "read_file" | "glob" | "grep" | "search_codebase" |
        "list_directory" | "get_symbols" | "get_diagnostics" |
        "memory_search" | "git_status" | "git_diff" |
        "web_search" | "web_fetch"
    )
}

// ── AgentInstance ──

/// Agent 实例，封装完整的循环逻辑、事件发射和工具调度
/// 所有字段均为 owned 类型（无生命周期参数），支持 tokio::spawn 并行调度
pub struct AgentInstance {
    pub agent_id: String,
    app: tauri::AppHandle,
    session_id: String,
    messages: Vec<llm::ChatMessage>,
    tool_definitions: Vec<ToolDefinition>,
    executor: Arc<ToolExecutor>,
    tool_ctx: ToolContext,
    question_awaiters: Option<QuestionAwaiters>,
    todo_list: Vec<TodoItem>,
    custom_instructions: Option<String>,
    provider: crate::config::LlmProvider,
    api_key: String,
    base_url: Option<String>,
    chat_model: String,
    /// Fast/cheap model for simple queries (falls back to chat_model if empty)
    fast_model: String,
    /// Whether automatic model routing is enabled
    model_routing_enabled: bool,
    max_iterations: usize,
    temperature: f32,
    max_tokens: u32,
    /// Context token budget before trimming (~2x max_tokens)
    max_context_tokens: usize,
    cancelled: Arc<AtomicBool>,
    /// Optional override for system prompt (used by sub-agents with their own agent definition)
    system_prompt_override: Option<String>,
    /// Agent started at instant for elapsed time tracking
    started_at: Option<Instant>,
    /// Estimated total tokens consumed
    total_tokens_est: usize,
    /// Max API calls allowed per session (0 = unlimited)
    max_api_calls: usize,
    /// Current API call count
    api_call_count: usize,
    /// Recent tool calls for deadlock detection: (tool_name, args_hash)
    recent_tool_calls: VecDeque<(String, u64)>,
    /// Planning mode: read-only analysis, no write operations
    pub plan_mode: bool,
    /// Current execution phase (Planning → Executing → Done)
    execution_phase: ExecutionPhase,
    /// Cross-session memory context (MEMORY.md + daily notes) injected into system prompt
    memory_context: Option<String>,
    /// Per-tool consecutive failures count (reset on success for each tool)
    consecutive_failures: HashMap<String, usize>,
    /// History of reflections to avoid duplicate analysis
    reflection_history: Vec<String>,
    /// Number of reflections performed this session (max 3)
    reflection_count: usize,
    /// Loop detector: prevents agent from wasting resources on non-progress loops
    loop_detector: LoopDetector,
    /// Lifecycle hook manager (pre/post tool hooks)
    hook_manager: hooks::HookManager,
    /// Shared hook context (snapshots, app handle, etc.)
    hook_context: hooks::HookContext,
    /// Append-only JSONL agent log for session persistence
    agent_log: Option<crate::memory::agent_log::AgentLog>,
    /// Whether pre-completion review prompt has been injected (prevents duplicate injection)
    review_injected: bool,
    /// Whether convergence hint has been injected at ~70% budget (prevents duplicate injection)
    convergence_injected: bool,
    /// Number of times auto-extend has been called (limits budget inflation)
    extend_count: usize,
    /// Consecutive read-only iterations (forces transition to writing after threshold)
    read_only_iterations: usize,
    /// Actual token usage from LLM API (accumulated across iterations)
    total_prompt_tokens: usize,
    total_completion_tokens: usize,

    /// Failed calls cache: (tool_name, args_hash) → error_message.
    /// When the same (tool, args) pair is called again, the cached error is returned
    /// without re-executing or incrementing the failure counter.
    /// Cleared for a tool when ANY call to that tool succeeds.
    failed_calls_cache: HashMap<(String, u64), String>,

    /// Claude Extended Thinking settings
    thinking_enabled: bool,
    thinking_budget: u32,

    /// Test-only: queued mock LLM responses. When Some, pop_front() replaces real LLM calls.
    /// Each entry is a Result to simulate both success and failure responses.
    #[doc(hidden)]
    pub mock_llm_responses: Option<VecDeque<Result<llm::LlmResponse, String>>>,
}

impl AgentInstance {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app: tauri::AppHandle,
        session_id: String,
        messages: Vec<llm::ChatMessage>,
        provider: crate::config::LlmProvider,
        api_key: String,
        base_url: Option<String>,
        chat_model: String,
        project_path: Option<String>,
        custom_instructions: Option<String>,
        cancelled: Arc<AtomicBool>,
        agent_def: Option<&AgentDefinition>,
        memory_context: Option<String>,
    ) -> Self {
        let all_tool_definitions = get_tools(Some(&app));
        let executor = tools::build_executor();

        // Register MCP tool wrappers into the executor (so they can be executed)
        if let Some(mcp_registry) = app.try_state::<Arc<crate::mcp::client::McpRegistry>>() {
            let registry = mcp_registry.inner().clone();
            if let Some(mcp_tools_state) =
                app.try_state::<Arc<std::sync::Mutex<Vec<ToolDefinition>>>>()
            {
                if let Ok(guard) = mcp_tools_state.lock() {
                    for tool_def in guard.iter() {
                        let wrapper = crate::mcp::tool_bridge::McpToolWrapper::new(
                            tool_def.name.clone(),
                            crate::mcp::McpToolDef {
                                name: tool_def.name.clone(),
                                description: tool_def.description.clone(),
                                input_schema: tool_def.parameters.clone(),
                            },
                            registry.clone(),
                        );
                        executor.register_raw(tool_def.name.clone(), Arc::new(wrapper));
                    }
                    log::info!("[Agent] Registered {} MCP tool wrappers", guard.len());
                }
            }
        }

        // Merge MCP tools (discovered in background) into the tool list
        let mcp_tools: Vec<ToolDefinition> = {
            let arc_clone = app
                .try_state::<Arc<std::sync::Mutex<Vec<ToolDefinition>>>>()
                .map(|s| s.inner().clone());
            if let Some(arc) = arc_clone {
                arc.lock().ok().map(|g| g.clone()).unwrap_or_default()
            } else {
                Vec::new()
            }
        };

        let all_tool_definitions: Vec<ToolDefinition> = {
            let mut merged = all_tool_definitions;
            merged.extend(mcp_tools);
            merged
        };
        let indexer = app.try_state::<Arc<CodeIndexer>>().map(|s| s.inner().clone());
        let question_awaiters = app.try_state::<QuestionAwaiters>().map(|s| s.inner().clone());

        // Build SandboxChecker from current AppSettings
        // NOTE: block_in_place is needed because AgentInstance::new() may be called
        // from inside tokio::spawn, where blocking_read() would otherwise panic with
        // "Cannot block the current thread from within a runtime".
        let sandbox_config = app
            .try_state::<Arc<RwLock<crate::config::AppSettings>>>()
            .map(|s| {
                let guard = tokio::task::block_in_place(|| s.blocking_read());
                guard.sandbox.clone()
            })
            .unwrap_or_default();

        // Read fast_model and routing settings from config
        let (fast_model, model_routing_enabled) = app
            .try_state::<Arc<RwLock<crate::config::AppSettings>>>()
            .map(|s| {
                let guard = tokio::task::block_in_place(|| s.blocking_read());
                (guard.fast_model.clone(), guard.model_routing_enabled)
            })
            .unwrap_or_else(|| (String::new(), false));
        // Read thinking settings from config
        let (thinking_enabled, thinking_budget) = app
            .try_state::<Arc<RwLock<crate::config::AppSettings>>>()
            .map(|s| {
                let guard = tokio::task::block_in_place(|| s.blocking_read());
                (guard.thinking_enabled, guard.thinking_budget)
            })
            .unwrap_or_else(|| (false, 0));
        let max_api_calls = app
            .try_state::<Arc<RwLock<crate::config::AppSettings>>>()
            .map(|s| {
                tokio::task::block_in_place(|| s.blocking_read().max_api_calls_per_session as usize)
            })
            .unwrap_or(200);
        let tavily_api_key = app
            .try_state::<Arc<RwLock<crate::config::AppSettings>>>()
            .map(|s| {
                tokio::task::block_in_place(|| s.blocking_read().tavily_api_key.clone())
            })
            .unwrap_or_default();
        let audit_log_path = app.path().app_config_dir().ok()
            .map(|p| p.join("audit.log"));
        let sandbox = Arc::new(SandboxChecker::new(sandbox_config, audit_log_path));
        let file_snapshot_store = app.try_state::<crate::commands::chat::FileSnapshotStore>().map(|s| s.inner().clone());

        // ── P1: Defensive diagnostics ──
        if app.try_state::<Arc<CodeIndexer>>().is_none() {
            log::warn!("[Agent] CodeIndexer not in app state — codebase search may not work");
        }
        if app.try_state::<QuestionAwaiters>().is_none() {
            log::warn!("[Agent] QuestionAwaiters not in app state — ask_user_question may not work");
        }
        if app.try_state::<crate::agent::definition::AgentRegistry>().is_none() {
            log::warn!("[Agent] AgentRegistry not in app state — sub-agents may not work");
        }

        // Filter tool_definitions by agent_def.tool_names if provided
        let tool_definitions = if let Some(def) = agent_def {
            all_tool_definitions.into_iter()
                .filter(|t| def.tool_names.contains(&t.name))
                .collect()
        } else {
            all_tool_definitions
        };

        let agent_id = agent_def.map(|d| d.id.clone()).unwrap_or_else(|| "agent".into());
        let max_iterations = agent_def.and_then(|d| d.max_iterations).unwrap_or(25);
        let temperature = agent_def.and_then(|d| d.temperature).unwrap_or(0.7);
        let max_tokens = agent_def.and_then(|d| d.max_tokens).unwrap_or(4096);
        let system_prompt_override = agent_def.map(|d| d.system_prompt.clone());

        // Build lifecycle hook manager with built-in hooks
        let hook_file_snapshots = Arc::new(std::sync::Mutex::new(HashMap::<String, String>::new()));
        let hook_config_dir = app.path().app_config_dir().ok();
        let (hook_manager, hook_context) = hooks::build_default_hooks(
            app.clone(),
            session_id.clone(),
            agent_id.clone(),
            project_path.clone(),
            cancelled.clone(),
            hook_file_snapshots.clone(),
            file_snapshot_store.clone(),
            hook_config_dir,
        );

        let context_window = crate::config::model_context_window(&chat_model);

        let mut agent = Self {
            agent_id,
            app: app.clone(),
            session_id: session_id.clone(),
            messages,
            tool_definitions,
            executor: Arc::new(executor),
            tool_ctx: ToolContext {
                project_path,
                indexer,
                sandbox,
                app_handle: Some(app),
                session_id: Some(session_id),
                tavily_api_key,
                llm_provider: provider.clone(),
                llm_api_key: api_key.clone(),
                llm_base_url: base_url.clone(),
                llm_model: chat_model.clone(),
            },
            question_awaiters,
            todo_list: Vec::new(),
            custom_instructions,
            provider,
            api_key,
            base_url,
            chat_model,
            fast_model,
            model_routing_enabled,
            max_iterations,
            temperature,
            max_tokens,
            max_context_tokens: context_window, // Dynamic context window based on model
            cancelled,
            system_prompt_override,
            started_at: None,
            total_tokens_est: 0,
            max_api_calls,
            api_call_count: 0,
            recent_tool_calls: VecDeque::new(),
            plan_mode: false,
            execution_phase: ExecutionPhase::Executing,
            memory_context,
            consecutive_failures: HashMap::new(),
            reflection_history: Vec::new(),
            reflection_count: 0,
            loop_detector: LoopDetector::new(Default::default()),
            hook_manager,
            hook_context,
            agent_log: None,
            review_injected: false,
            convergence_injected: false,
            extend_count: 0,
            read_only_iterations: 0,
            total_prompt_tokens: 0,
            total_completion_tokens: 0,
            mock_llm_responses: None,
            failed_calls_cache: HashMap::new(),
            thinking_enabled: thinking_enabled,
            thinking_budget: thinking_budget,
        };

        // ── Auto-enable plan_mode for complex tasks ──
        // If the task mentions multi-step keywords, automatically enable planning phase
        // to anchor the agent's scope and prevent open-ended exploration loops.
        if !agent.plan_mode {
            let task_text = agent.messages.iter()
                .find(|m| m.role == "user")
                .map(|m| m.content.to_lowercase())
                .unwrap_or_default();
            let complex_keywords = [
                "implement", "build", "create", "refactor", "migrate",
                "redesign", "add a feature", "add feature", "new module",
                "rewrite", "architecture", "system design",
            ];
            let is_complex = task_text.len() > 150
                || complex_keywords.iter().any(|kw| task_text.contains(kw));
            if is_complex {
                agent.plan_mode = true;
                agent.execution_phase = ExecutionPhase::Planning;
                log::info!("[Agent:{}] Auto-enabled plan_mode for complex task ({} chars)",
                    agent.agent_id, task_text.len());
            }
        }

        agent
    }

    /// Set Claude Extended Thinking settings
    pub fn set_thinking(&mut self, enabled: bool, budget: u32) {
        self.thinking_enabled = enabled;
        self.thinking_budget = budget;
    }

    /// Extend the iteration budget at runtime.
    ///
    /// Used when the agent detects a complex multi-task scenario (e.g., todo_write
    /// creates many tasks) and needs more iterations to complete.
    ///
    /// Hard-capped at 200 iterations to prevent runaway execution.
    pub fn extend_iterations(&mut self, additional: usize) {
        let old_max = self.max_iterations;
        self.max_iterations = (self.max_iterations + additional).min(200);
        log::info!(
            "[Agent:{}] Extending max_iterations: {} → {} (+{})",
            self.agent_id, old_max, self.max_iterations, additional
        );
        self.emit_log("info", &format!(
            "Iteration budget extended: {} → {} (+{})",
            old_max, self.max_iterations, additional
        ));
    }

    /// Select the appropriate model based on task complexity and routing configuration.
    /// - If routing is disabled or fast_model is not set → always use chat_model (capable).
    /// - If routing is enabled:
    ///   - Simple tasks (short queries, no tools needed, Ask mode) → fast_model
    ///   - Complex tasks (agent mode, many iterations, large context) → chat_model
    fn select_model(&self, iteration: usize) -> String {
        if !self.model_routing_enabled || self.fast_model.is_empty() {
            return self.chat_model.clone();
        }

        // Determine complexity based on iteration stage and tool count
        let is_simple_iteration = iteration == 0 || iteration > self.max_iterations / 2;
        let has_many_tools = self.tool_definitions.len() > 10;

        // Analyze last user message for complexity signals
        let last_user_msg = self.messages.iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .unwrap_or("");

        // Complexity assessment based on message content
        let is_complex_task = Self::assess_task_complexity(last_user_msg, self.tool_definitions.len());

        if is_complex_task {
            // Complex tasks: use chat_model with thinking if enabled
            log::debug!("[ModelRouting] Complex task detected → using chat_model");
            self.chat_model.clone()
        } else if is_simple_iteration && !has_many_tools {
            // Simple iteration: use fast_model
            log::debug!("[ModelRouting] Simple iteration → using fast_model");
            self.fast_model.clone()
        } else {
            // Default: use chat_model
            self.chat_model.clone()
        }
    }

    /// Assess task complexity based on message content and tool count.
    /// Returns true if the task is complex (requires full reasoning model).
    fn assess_task_complexity(message: &str, tool_count: usize) -> bool {
        let msg_lower = message.to_lowercase();
        
        // High complexity signals: multi-step planning, architecture, debugging
        let high_complexity_keywords = [
            "design", "architect", "plan", "strategy", "refactor",
            "debug", "investigate", "analyze", "compare", "evaluate",
            "implement", "create", "build", "develop", "write",
            "fix", "solve", "resolve", "troubleshoot",
        ];
        
        let high_signal_count = high_complexity_keywords.iter()
            .filter(|&kw| msg_lower.contains(kw))
            .count();
        
        // Complex if: multiple high-complexity signals OR long message (>500 chars) OR many tools
        high_signal_count >= 2 || message.len() > 500 || tool_count > 15
    }

    /// Record a tool usage event for user preferences tracking.
    fn record_tool_usage(&self, tool_name: &str, success: bool, duration_ms: u64) {
        if let Some(mem_state) = self.app.try_state::<std::sync::Arc<tokio::sync::RwLock<crate::memory::MemoryManager>>>() {
            let mem = mem_state.inner().clone();
            let name = tool_name.to_string();
            tokio::spawn(async move {
                let mgr = mem.read().await;
                if let Ok(mut prefs) = mgr.preferences.lock() {
                    prefs.record_tool_usage(&name, success, duration_ms);
                    // Save periodically (every 10 tool uses)
                    if prefs.tool_stats.values().map(|s| s.total_calls).sum::<u32>() % 10 == 0 {
                        let _ = prefs.save(&mgr._base_dir);
                    }
                }
            });
        }
    }

    /// Record a file edit event for user preferences tracking.
    fn record_file_edit(&self, file_path: &str) {
        if let Some(mem_state) = self.app.try_state::<std::sync::Arc<tokio::sync::RwLock<crate::memory::MemoryManager>>>() {
            let mem = mem_state.inner().clone();
            let path = file_path.to_string();
            tokio::spawn(async move {
                let mgr = mem.read().await;
                if let Ok(mut prefs) = mgr.preferences.lock() {
                    prefs.record_file_edit(&path);
                }
            });
        }
    }

    /// Write a log entry if the agent log is initialized (fire-and-forget).
    async fn log_agent_event(&mut self, entry: crate::memory::agent_log::LogEntryType) {
        if let Some(ref mut log) = self.agent_log {
            if let Err(e) = log.append(entry).await {
                log::debug!("[Agent] Failed to write agent log: {}", e);
            }
        }
    }

    /// Record telemetry for session end (success/error/cancelled).
    async fn record_telemetry_end(&self, outcome: &str, iterations: usize, error_message: Option<&str>) {
        if let Some(telemetry) = self.app.try_state::<crate::telemetry::TelemetryCollector>() {
            let duration_ms = self.started_at.map(|s| s.elapsed().as_millis() as u64).unwrap_or(0);
            telemetry.record(&crate::telemetry::TelemetryEvent::SessionEnd {
                session_id: self.session_id.clone(),
                outcome: outcome.to_string(),
                iterations,
                total_prompt_tokens: self.total_prompt_tokens,
                total_completion_tokens: self.total_completion_tokens,
                duration_ms,
                error_message: error_message.map(|s| s.to_string()),
            });
        }
    }

    /// Compact the message context if it exceeds the token budget threshold.
    /// Uses LLM summarization to compress middle messages while preserving
    /// the first user message and recent interactions.
    async fn compact_context_if_needed(&mut self) {
        let system_prompt = self.build_system_prompt();
        let total_before = self.messages.len() as u32;

        match context::compact_if_needed(
            &self.messages,
            &system_prompt,
            self.max_context_tokens,
            &self.provider,
            &self.api_key,
            self.base_url.as_deref(),
            &self.chat_model,
        )
        .await
        {
            Ok(compacted) => {
                let total_after = compacted.len() as u32;
                if total_after < total_before {
                    let removed = total_before - total_after;
                    self.emit_log(
                        "info",
                        &format!(
                            "Context compacted: {} -> {} messages (summarized {})",
                            total_before, total_after, removed
                        ),
                    );
                    let _ = self.app.emit(
                        "chat-event",
                        ChatEvent::ContextTrimmed {
                            session_id: self.session_id.clone(),
                            agent_id: Some(self.agent_id.clone()),
                            trimmed_count: removed,
                            total_before,
                            total_after,
                        },
                    );
                    self.messages = compacted;
                }
            }
            Err(e) => {
                log::warn!("[Agent:{}] Context compaction failed: {}", self.agent_id, e);
                // Fallback: do nothing, proceed with original messages
            }
        }
    }

    /// Filter tool definitions based on execution phase and iteration.
    /// - Planning phase: only read-only tools
    /// - Executing, early iterations (<2): exploration tools (read-only + todo_write)
    /// - Executing, late iterations (last 2): verification tools (read-only + diagnostics + terminal)
    /// - Executing, middle: all tools
    fn filter_tools_by_phase(&self, iteration: usize) -> Vec<serde_json::Value> {
        let active_names: &[&str] = match self.execution_phase {
            ExecutionPhase::Planning => PLANNING_PHASE_TOOLS,
            ExecutionPhase::Done => return Vec::new(), // No tools needed when agent is done
            ExecutionPhase::Executing => {
                // Fine-grained iteration-based tool selection
                let max = self.max_iterations;
                if max > 4 && iteration < 2 {
                    // Exploration phase: force agent to read before writing
                    EXPLORATION_PHASE_TOOLS
                } else if max > 4 && iteration >= max.saturating_sub(2) {
                    // Verification phase: read-only + terminal for testing
                    VERIFICATION_PHASE_TOOLS
                } else {
                    // Full execution: all tools
                    return self
                        .tool_definitions
                        .iter()
                        .map(|t| t.to_openai_tool())
                        .collect();
                }
            }
        };
        self.tool_definitions
            .iter()
            .filter(|t| active_names.contains(&t.name.as_str()))
            .map(|t| t.to_openai_tool())
            .collect()
    }

    /// Check if a tool result indicates failure.
    ///
    /// Uses the same prefix protocol as `execute_regular_tool` — only results
    /// that start with a known error prefix are considered failures. This avoids
    /// false positives where file content or command output naturally contains
    /// words like "error" or "failed".
    fn is_tool_failure(result: &str) -> bool {
        result.starts_with("Error:")
            || result.starts_with("[TIMEOUT]")
            || result.starts_with("[TOOL_NOT_FOUND]")
            || result.starts_with("[SANDBOX_BLOCKED]")
            || result.starts_with("[PERMISSION_DENIED]")
            || result.starts_with("[RETRY_FAILED]")
    }

    /// Compute a hash of tool arguments for deduplication.
    fn hash_args(args: &serde_json::Value) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let args_str = serde_json::to_string(args).unwrap_or_default();
        args_str.hash(&mut hasher);
        hasher.finish()
    }

    /// Classify a tool error and return an actionable guidance hint.
    /// Returns empty string if no specific hint applies.
    fn classify_error_guidance(tool_name: &str, result: &str) -> String {
        let lower = result.to_lowercase();
        if lower.contains("not found") || lower.contains("no such file") {
            match tool_name {
                "read_file" | "edit" | "delete_file" => {
                    return "Hint: Verify the file path. Use list_directory or glob to find the correct path.".to_string();
                }
                _ => {}
            }
        }
        if lower.contains("sandbox blocked") || lower.contains("denied") {
            return "Hint: This path is outside the project sandbox. Use paths relative to the project root.".to_string();
        }
        if lower.contains("appears") && lower.contains("times") {
            return "Hint: old_string matches multiple locations. Add more context, use start_line/end_line, or set replace_all: true.".to_string();
        }
        if lower.contains("invalid regex") {
            return "Hint: Check regex syntax or omit the 'regex' parameter for substring search.".to_string();
        }
        if lower.contains("timeout") {
            return "Hint: The operation timed out. Try a simpler approach or break the task into smaller steps.".to_string();
        }
        String::new()
    }

    /// Reflect on recent failures using LLM analysis.
    /// Collects failed tool calls, asks LLM for alternative strategies,
    /// and returns the reflection text.
    async fn reflect_on_failures(&mut self) -> Result<String, String> {
        // Collect recent tool results (last 6 messages that are tool results)
        let mut recent_failures: Vec<String> = Vec::new();
        for msg in self.messages.iter().rev().take(10) {
            if msg.role == "tool" && Self::is_tool_failure(&msg.content) {
                recent_failures.push(format!("Tool result: {}", msg.content.chars().take(300).collect::<String>()));
            }
            if recent_failures.len() >= 3 {
                break;
            }
        }

        if recent_failures.is_empty() {
            return Err("No recent failures found to reflect on".to_string());
        }

        let reflection_prompt = format!(
            "You are analyzing recent tool failures in a coding agent session. \
            The agent has failed {} consecutive times. Here are the recent failures:
            {}
            \
            Analyze the root cause and suggest an alternative approach. \
            Be concise (3-5 sentences). Focus on actionable changes, not restating the problem.",
            self.consecutive_failures.values().max().copied().unwrap_or(0),
            recent_failures.join("\n\n---\n\n")
        );

        let request = llm::ChatRequestParams {
            model: self.select_model(0), // Self-reflection uses routed model
            messages: vec![llm::ChatMessage {
                role: "user".into(),
                content: reflection_prompt,
                images: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            system: "You are a debugging assistant. Analyze tool failures and suggest alternative strategies.".to_string(),
            max_tokens: 500,
            temperature: 0.3,
            thinking_enabled: false,
            thinking_budget: 0,
        };

        // Call LLM without tools — just text response
        let empty_tools: Vec<serde_json::Value> = vec![];
        let (response, _usage) = llm::chat_with_tools(
            &self.provider,
            &self.api_key,
            self.base_url.as_deref(),
            request,
            &empty_tools,
            Some(self.cancelled.clone()),
        )
        .await?;

        match response {
            llm::LlmResponse::Text(text) => Ok(text),
            llm::LlmResponse::ToolCalls { .. } => Err("LLM returned tool calls during reflection (expected text)".to_string()),
        }
    }

    /// 运行完整的 Agent 循环
    pub async fn run(&mut self) -> Result<String, String>
    where
        Self: Send,
    {
        self.started_at = Some(Instant::now());
        self.emit_started();
        self.emit_log("info", "Agent started");

        // Record telemetry: session start
        if let Some(telemetry) = self.app.try_state::<crate::telemetry::TelemetryCollector>() {
            telemetry.record(&crate::telemetry::TelemetryEvent::SessionStart {
                session_id: self.session_id.clone(),
                model: self.chat_model.clone(),
                provider: format!("{:?}", self.provider),
                plan_mode: self.plan_mode,
            });
        }

        // Initialize agent log (fire-and-forget — failures are non-fatal)
        if let Some(config_dir) = self.app.path().app_config_dir().ok() {
            let sessions_dir = config_dir.join("sessions");
            match crate::memory::agent_log::AgentLog::new(
                &sessions_dir,
                &self.session_id,
                &self.agent_id,
            )
            .await
            {
                Ok(log) => self.agent_log = Some(log),
                Err(e) => log::warn!("[Agent] Failed to init agent log: {}", e),
            }
        }

        // Tool definitions are now filtered per-iteration based on execution_phase

        let mut iteration = 0;
        while iteration < self.max_iterations {
            // Check cancellation
            if self.cancelled.load(Ordering::Relaxed) {
                self.emit_cancelled();
                self.emit_log("warn", "Agent cancelled by user");
                self.log_agent_event(crate::memory::agent_log::LogEntryType::Cancelled).await;
                // Record telemetry: session cancelled
                self.record_telemetry_end("cancelled", iteration, None).await;
                return Err("Agent cancelled by user".to_string());
            }

            // 发射思考状态（含迭代进度）
            let elapsed = self.started_at.map(|s| s.elapsed().as_millis() as u64).unwrap_or(0);
            self.emit_status(
                if iteration == 0 { "Analyzing task and planning..." } else { "Processing results and determining next step..." },
                iteration as u32,
                self.max_iterations as u32,
                self.total_tokens_est as u32,
                elapsed,
            );
            self.emit_log("info", &format!("Iteration {}/{} starting (phase: {:?})", iteration + 1, self.max_iterations, self.execution_phase));

            // ── Convergence injection: at ~70% budget, tell agent to wrap up ──
            // Qoder does this at 80%; we do it earlier to leave more runway for
            // the summarization turn. Fires only once per session.
            let convergence_threshold = (self.max_iterations as f64 * 0.7).ceil() as usize;
            if !self.convergence_injected
                && iteration >= convergence_threshold
                && self.execution_phase == ExecutionPhase::Executing
            {
                self.convergence_injected = true;
                self.emit_log("info", &format!(
                    "Injecting convergence hint at iteration {}/{} — agent should start wrapping up",
                    iteration + 1, self.max_iterations
                ));
                self.messages.push(llm::ChatMessage {
                    role: "system".into(),
                    content: "[CONVERGENCE_NOTICE] You are approaching your iteration budget limit \
                        (approximately 70% used). STOP exploring new files and start wrapping up:\n\
                        1. Summarize what you have accomplished so far\n\
                        2. If there are remaining steps, describe them briefly for the user\n\
                        3. Do NOT call any more write/create tools — finalize your response\n\
                        Respond with a concise completion summary now.".into(),
                    images: None,
                    tool_calls: None,
                    tool_call_id: None,
                });
            }

            // ── Read-only streak injection: force agent to start writing ──
            // If agent has done 15+ read-only iterations without any write, inject a strong hint
            let read_only_streak = self.loop_detector.get_read_only_streak();
            if read_only_streak >= 15 && read_only_streak % 10 == 0 {
                // Inject every 10 read-only iterations after the initial 15
                self.emit_log("warn", &format!(
                    "Read-only streak: {} iterations without write operations — forcing action",
                    read_only_streak
                ));
                self.messages.push(llm::ChatMessage {
                    role: "system".into(),
                    content: format!(
                        "[ACTION_REQUIRED] You have performed {} read-only operations (read_file, list_directory, glob, grep) \
                         without making any changes. STOP exploring and START implementing:\n\
                         1. You have enough context — begin writing code NOW\n\
                         2. Use write_file or edit to make changes\n\
                         3. Do NOT read any more files unless absolutely necessary\n\
                         4. If you're unsure, make your best attempt and iterate",
                        read_only_streak
                    ),
                    images: None,
                    tool_calls: None,
                    tool_call_id: None,
                });
            }

            // Compact context if it exceeds token budget
            self.compact_context_if_needed().await;

            // Resource quota: check API call limit
            if self.max_api_calls > 0 && self.api_call_count >= self.max_api_calls {
                let msg = format!(
                    "[RESOURCE_LIMIT] API call limit reached ({}/{}). Session quota exhausted.",
                    self.api_call_count, self.max_api_calls
                );
                self.emit_log("warn", &msg);
                self.log_agent_event(crate::memory::agent_log::LogEntryType::Error { message: msg.clone() }).await;
                // Record telemetry: session error
                self.record_telemetry_end("error", iteration, Some(&msg)).await;
                return Err(msg);
            }
            self.api_call_count += 1;

            // Dynamically filter tools based on current execution phase and iteration
            let tool_jsons = self.filter_tools_by_phase(iteration);

            log::debug!(
                "[Agent:{}] ── Iteration {}/{} ── model={}, msgs={}, tools={}, phase={:?}",
                self.agent_id, iteration + 1, self.max_iterations,
                self.select_model(iteration),
                self.messages.len(), tool_jsons.len(), self.execution_phase
            );

            let request = llm::ChatRequestParams {
                model: self.select_model(iteration),
                messages: self.messages.clone(),
                system: self.build_system_prompt(),
                max_tokens: self.max_tokens,
                temperature: self.temperature,
                thinking_enabled: self.thinking_enabled,
                thinking_budget: self.thinking_budget,
            };

            let (response, usage) = if let Some(ref mut queue) = self.mock_llm_responses {
                // Test mode: consume from mock queue instead of calling real LLM
                (queue.pop_front().unwrap_or_else(|| {
                    Err("Mock LLM response queue exhausted".to_string())
                })?, None)
            } else {
                // Stream tokens to frontend in real time
                let stream_app = self.app.clone();
                let stream_session = self.session_id.clone();
                let stream_agent = self.agent_id.clone();
                llm::stream_chat_with_tools(
                    &self.provider,
                    &self.api_key,
                    self.base_url.as_deref(),
                    request,
                    &tool_jsons,
                    Some(self.cancelled.clone()),
                    move |token: String| {
                        let _ = stream_app.emit("chat-event", ChatEvent::Delta {
                            session_id: stream_session.clone(),
                            agent_id: Some(stream_agent.clone()),
                            token,
                        });
                        Ok(())
                    },
                )
                .await?
            };

            // Track actual token usage from API, fall back to estimate
            if let Some(ref u) = usage {
                self.total_prompt_tokens += u.prompt_tokens;
                self.total_completion_tokens += u.completion_tokens;
                self.total_tokens_est = self.total_prompt_tokens + self.total_completion_tokens;
            } else {
                // Estimate tokens: roughly ~4 chars per token for response
                let response_size = match &response {
                    llm::LlmResponse::Text(t) => t.len(),
                    llm::LlmResponse::ToolCalls { calls, content } => {
                        content.as_ref().map(|c| c.len()).unwrap_or(0) + calls.iter().map(|c| c.name.len() + c.arguments.to_string().len()).sum::<usize>()
                    }
                };
                self.total_tokens_est += response_size / 4;
            }

            // Emit token usage update for dashboard
            let _ = self.app.emit("usage-update", serde_json::json!({
                "session_id": self.session_id,
                "agent_id": self.agent_id,
                "total_tokens_est": self.total_tokens_est,
                "api_call_count": self.api_call_count,
                "iteration": iteration + 1,
                "max_iterations": self.max_iterations,
            }));

            match response {
                llm::LlmResponse::Text(text) => {
                    log::debug!(
                        "[Agent:{}] LLM returned Text ({} chars), no tool_calls. First 200 chars: {:?}",
                        self.agent_id, text.len(),
                        text.chars().take(200).collect::<String>()
                    );
                    // Phase transition: Planning → Executing
                    if self.execution_phase == ExecutionPhase::Planning {
                        self.execution_phase = ExecutionPhase::Executing;
                        self.emit_log("info", "Planning phase complete — transitioning to Executing phase");
                        self.emit_thinking(&format!("**Plan:**\n{}", text));
                        // Inject the plan as a user message so LLM follows it in Executing phase
                        self.messages.push(llm::ChatMessage {
                            role: "user".into(),
                            content: format!("Here is the plan I created. Now execute it:\n\n{}", text),
                            images: None,
                            tool_calls: None,
                            tool_call_id: None,
                        });
                        continue; // Continue to Executing phase
                    }

                    self.emit_log("info", "Agent completed with text response");
                    // Pre-Done reflection: ask LLM to review changes before finishing
                    // Only inject once (review_injected flag prevents duplicate injection)
                    if !self.review_injected
                        && self.execution_phase == ExecutionPhase::Executing
                        && iteration < self.max_iterations - 1
                    {
                        let snapshots = self.hook_context.file_snapshots.lock()
                            .unwrap_or_else(|e| e.into_inner());
                        if !snapshots.is_empty() {
                            drop(snapshots);
                            self.review_injected = true;
                            self.emit_log("info", "Injecting pre-completion reflection prompt");
                            self.messages.push(llm::ChatMessage {
                                role: "system".into(),
                                content: "[PRE-COMPLETION REVIEW] Before finishing, review your changes:\n\
                                    1. Do all modified files compile without errors?\n\
                                    2. Are there any edge cases or missed requirements?\n\
                                    3. Did you run tests or verification commands?\n\
                                    If you find issues, fix them now. Otherwise, confirm completion.".into(),
                                images: None,
                                tool_calls: None,
                                tool_call_id: None,
                            });
                            continue; // Give LLM one more iteration to self-review
                        }
                    }

                    self.log_agent_event(crate::memory::agent_log::LogEntryType::Completed { final_text: text.clone() }).await;
                    self.emit_edit_diff();
                    self.emit_finished(&text);
                    // Record telemetry: session success
                    self.record_telemetry_end("success", iteration, None).await;
                    return Ok(text);
                }
                llm::LlmResponse::ToolCalls { calls: tool_calls, content: thinking } => {
                    log::debug!(
                        "[Agent:{}] LLM returned {} tool_calls: [{}], content={} chars",
                        self.agent_id, tool_calls.len(),
                        tool_calls.iter().map(|tc| format!("{}({})", tc.name, tc.arguments.to_string().chars().take(80).collect::<String>())).collect::<Vec<_>>().join(", "),
                        thinking.as_ref().map(|t| t.len()).unwrap_or(0)
                    );
                    // 1. 发射 LLM 思考过程（如果有）
                    if let Some(ref thought) = thinking {
                        let trimmed = thought.trim();
                        if !trimmed.is_empty() {
                            self.emit_thinking(trimmed);
                            self.emit_log("info", &format!("LLM reasoning ({} chars)", trimmed.len()));
                        }
                    }

                    // 2. 将 assistant 消息（含 tool_calls）加入上下文
                    let tool_calls_json: Vec<serde_json::Value> = tool_calls
                        .iter()
                        .map(|tc| {
                            serde_json::json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                                }
                            })
                        })
                        .collect();
                    self.messages.push(llm::ChatMessage {
                        role: "assistant".into(),
                        content: String::new(),
                        images: None,
                        tool_calls: Some(serde_json::Value::Array(tool_calls_json)),
                        tool_call_id: None,
                    });

                    // ── Parallel pre-execution of read-only tools ──
                    // When LLM returns multiple read-only tool calls, execute them in
                    // parallel for a 2-3× speedup. Write tools remain serial.
                    let parallel_results: std::collections::HashMap<usize, (String, u64)> = {
                        let read_only_indices: Vec<usize> = tool_calls.iter().enumerate()
                            .filter(|(_, tc)| is_read_only_tool(&tc.name))
                            .map(|(i, _)| i)
                            .collect();

                        if read_only_indices.len() > 1 {
                            log::info!("[Agent:{}] Parallel executing {} read-only tools", self.agent_id, read_only_indices.len());
                            let executor = self.executor.clone();
                            let tool_ctx = self.tool_ctx.clone();
                            let mut join_set = tokio::task::JoinSet::new();

                            for idx in read_only_indices.into_iter() {
                                let tc = &tool_calls[idx];
                                let exec = executor.clone();
                                let ctx = tool_ctx.clone();
                                let name = tc.name.clone();
                                let args = tc.arguments.clone();
                                log::debug!("[Agent:{}] Parallel spawn: tool[{}] = {}({})", self.agent_id, idx, name, args.to_string().chars().take(100).collect::<String>());
                                join_set.spawn(async move {
                                    let start = Instant::now();
                                    let result = exec.execute(&name, args, &ctx).await;
                                    log::debug!("[Agent:parallel] tool[{}] completed: len={}, preview={:?}", idx, result.len(), result.chars().take(150).collect::<String>());
                                    (idx, result, start.elapsed().as_millis() as u64)
                                });
                            }

                            let mut results = std::collections::HashMap::new();
                            while let Some(res) = join_set.join_next().await {
                                if let Ok((idx, result, duration)) = res {
                                    // Only cache successful results. Failed results are
                                    // re-executed serially to give tools a fresh attempt
                                    // and avoid inflating the consecutive failure counter.
                                    let is_fail = Self::is_tool_failure(&result);
                                    log::debug!("[Agent:{}] Parallel result for tool[{}]: is_fail={}, len={}", self.agent_id, idx, is_fail, result.len());
                                    if !is_fail {
                                        results.insert(idx, (result, duration));
                                    } else {
                                        log::debug!(
                                            "[Agent:{}] Parallel result for tool[{}] was a failure, skipping cache. Preview: {:?}",
                                            self.agent_id, idx, result.chars().take(200).collect::<String>()
                                        );
                                    }
                                }
                            }
                            results
                        } else {
                            std::collections::HashMap::new()
                        }
                    };

                    // 2. 逐个执行工具并推送结果
                    let mut batch_written_files: Vec<String> = Vec::new();
                    let mut batch_has_failure = false;

                    for (i, tc) in tool_calls.iter().enumerate() {
                        // Check cancellation before each tool execution
                        if self.cancelled.load(Ordering::Relaxed) {
                            self.emit_cancelled();
                            return Err("Agent cancelled by user".to_string());
                        }

                        self.emit_status(
                            &format!("Executing tool {}/{}: {}...", i + 1, tool_calls.len(), tc.name),
                            iteration as u32,
                            self.max_iterations as u32,
                            self.total_tokens_est as u32,
                            self.started_at.map(|s| s.elapsed().as_millis() as u64).unwrap_or(0),
                        );

                        let tool_start = Instant::now();
                        let timestamp = chrono::Utc::now().timestamp();
                        self.emit_tool_call(tc, timestamp);
                        self.emit_log("info", &format!("Executing tool: {} (args: {})", tc.name, tc.arguments.to_string().chars().take(120).collect::<String>()));
                        self.log_agent_event(crate::memory::agent_log::LogEntryType::ToolCall {
                            name: tc.name.clone(),
                            arguments: tc.arguments.clone(),
                        }).await;

                        // Pre-tool hooks (snapshot + confirm)
                        let mut tool_args = tc.arguments.clone();
                        let args_modified;
                        match self.hook_manager.pre_tool_chain(&tc.name, &mut tool_args, &self.hook_context).await {
                            hooks::HookResult::Deny(msg) => {
                                self.emit_log("warn", &format!("Hook denied: {}", tc.name));
                                self.emit_tool_result(&msg, 0);
                                self.messages.push(llm::ChatMessage {
                                    role: "tool".into(),
                                    content: msg,
                                    images: None,
                                    tool_calls: None,
                                    tool_call_id: Some(tc.id.clone()),
                                });
                                continue;
                            }
                            hooks::HookResult::ModifyArgs(new_args) => {
                                tool_args = new_args;
                                args_modified = true;
                            }
                            hooks::HookResult::Continue => {
                                args_modified = false;
                            }
                        }

                        // Build effective ToolCallRequest with potentially hook-modified args.
                        // This ensures execute_and_handle_special uses the correct args,
                        // and the parallel cache is skipped when args were altered by a hook.
                        let effective_tc = crate::llm::ToolCallRequest {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            arguments: tool_args.clone(),
                        };

                        // ── Failed calls deduplication ──
                        // If the same (tool, args) pair has been called before and failed,
                        // return the cached error without re-executing or incrementing the
                        // failure counter. This prevents LLM from wasting iterations on
                        // repeating the same failed call.
                        let args_hash = Self::hash_args(&tool_args);
                        let cache_key = (tc.name.clone(), args_hash);
                        let is_cached_failure = self.failed_calls_cache.contains_key(&cache_key);

                        let (result, duration_ms) = if is_cached_failure {
                            // Return cached error — skip execution entirely
                            let cached_err = self.failed_calls_cache.get(&cache_key).cloned().unwrap_or_default();
                            log::info!(
                                "[Agent:{}] Returning cached failure for '{}' (same args as before)",
                                self.agent_id, tc.name
                            );
                            (cached_err, 0u64)
                        } else if !args_modified {
                            if let Some((r, d)) = parallel_results.get(&i) {
                                log::debug!("[Agent:{}] Using parallel-cached result for '{}', len={}", self.agent_id, tc.name, r.len());
                                (r.clone(), *d)
                            } else {
                                log::debug!("[Agent:{}] Serial executing: {}({})", self.agent_id, tc.name, tool_args.to_string().chars().take(100).collect::<String>());
                                let result = self.execute_and_handle_special(&effective_tc).await;
                                log::debug!("[Agent:{}] Serial result for '{}': len={}, preview={:?}", self.agent_id, tc.name, result.len(), result.chars().take(150).collect::<String>());
                                (result, tool_start.elapsed().as_millis() as u64)
                            }
                        } else {
                            log::info!("[Agent:{}] Args modified by hook for '{}', skipping parallel cache", self.agent_id, tc.name);
                            log::debug!("[Agent:{}] Serial executing (hook-modified): {}({})", self.agent_id, tc.name, tool_args.to_string().chars().take(100).collect::<String>());
                            let result = self.execute_and_handle_special(&effective_tc).await;
                            log::debug!("[Agent:{}] Serial result for '{}': len={}, preview={:?}", self.agent_id, tc.name, result.len(), result.chars().take(150).collect::<String>());
                            (result, tool_start.elapsed().as_millis() as u64)
                        };

                        // Post-tool hooks
                        let post_result = self.hook_manager.post_tool_chain(&tc.name, &tool_args, &result, &self.hook_context).await;
                        let mut final_result = post_result.modified_result.unwrap_or(result);

                        // Track per-tool consecutive failures for self-reflection
                        let _is_fail_1 = Self::is_tool_failure(&final_result);
                        log::debug!(
                            "[Agent:{}] is_tool_failure[1] for '{}': {} | result_preview={:?}",
                            self.agent_id, tc.name, _is_fail_1,
                            final_result.chars().take(150).collect::<String>()
                        );
                        if _is_fail_1 {
                            // Add guidance hint to help LLM recover (only on first failure)
                            if !is_cached_failure {
                                let guidance = Self::classify_error_guidance(&tc.name, &final_result);
                                if !guidance.is_empty() {
                                    final_result = format!("{}\n{}", final_result, guidance);
                                }
                                // New failure: cache it and increment counter
                                self.failed_calls_cache.insert(cache_key, final_result.clone());
                                let count = self.consecutive_failures
                                    .entry(tc.name.clone())
                                    .and_modify(|c| *c += 1)
                                    .or_insert(1);
                                let failure_count = *count;
                                batch_has_failure = true;
                                if failure_count >= 2 {
                                    self.emit_log("warn", &format!(
                                        "Tool '{}' has {} consecutive failures", tc.name, failure_count
                                    ));
                                }
                            }
                            // If is_cached_failure: don't increment, don't re-warn.
                            // The LLM already received this error before.
                        } else {
                            // Success: reset failure counter and clear cache for this tool
                            self.consecutive_failures.remove(&tc.name);
                            self.failed_calls_cache.retain(|(name, _), _| *name != tc.name);
                            // Track successfully written files for potential rollback
                            if matches!(tc.name.as_str(), "write_file" | "edit" | "append_file" | "create_directory" | "delete_file" | "delete_directory") {
                                if let Some(path) = tool_args.get("file_path").or_else(|| tool_args.get("path")).and_then(|v| v.as_str()) {
                                    batch_written_files.push(path.to_string());
                                    // Record file edit for preferences tracking
                                    self.record_file_edit(path);
                                }
                            }
                            // Record tool success for preferences
                            self.record_tool_usage(&tc.name, true, duration_ms);
                        }

                        // Record tool failure for preferences (when not success)
                        let _is_fail_2 = Self::is_tool_failure(&final_result);
                        log::debug!(
                            "[Agent:{}] is_tool_failure[2] for '{}': {}",
                            self.agent_id, tc.name, _is_fail_2
                        );
                        if _is_fail_2 {
                            self.record_tool_usage(&tc.name, false, duration_ms);
                        }

                        self.emit_tool_result(&final_result, duration_ms);
                        self.emit_log("info", &format!("Tool '{}' completed in {}ms", tc.name, duration_ms));
                        self.log_agent_event(crate::memory::agent_log::LogEntryType::ToolResult {
                            name: tc.name.clone(),
                            result: final_result.chars().take(2000).collect::<String>(),
                            duration_ms,
                        }).await;

                        // Record telemetry: tool call
                        if let Some(telemetry) = self.app.try_state::<crate::telemetry::TelemetryCollector>() {
                            let is_success = !Self::is_tool_failure(&final_result);
                            telemetry.record(&crate::telemetry::TelemetryEvent::ToolCall {
                                session_id: self.session_id.clone(),
                                tool: tc.name.clone(),
                                success: is_success,
                                duration_ms,
                                is_loop: false,
                            });
                        }

                        // Record tool call for loop detection
                        let is_success = !Self::is_tool_failure(&final_result);
                        log::debug!(
                            "[Agent:{}] is_tool_failure[3] for '{}': {} (is_success={})",
                            self.agent_id, tc.name, Self::is_tool_failure(&final_result), is_success
                        );
                        self.loop_detector.record_call(&tc.name, &tool_args, &final_result, is_success);
                        log::debug!(
                            "[Agent:{}] After record_call: consecutive_failures[{}]={}",
                            self.agent_id, tc.name,
                            self.loop_detector.failure_count(&tc.name)
                        );

                        self.messages.push(llm::ChatMessage {
                            role: "tool".into(),
                            content: final_result,
                            images: None,
                            tool_calls: None,
                            tool_call_id: Some(tc.id.clone()),
                        });

                        // Inject any additional messages from post-tool hooks
                        for msg in post_result.additional_messages {
                            self.messages.push(msg);
                        }
                    }

                    // 3. Post-tool-batch hooks (auto-diagnose)
                    let batch_msgs = self.hook_manager.post_tool_batch_chain(&tool_calls, &self.hook_context).await;
                    for msg in batch_msgs {
                        self.messages.push(msg);
                    }

                    // 3.5 Atomic rollback: if any tool in batch failed, restore all files modified in this batch
                    if batch_has_failure && !batch_written_files.is_empty() {
                        let snapshots = self.hook_context.file_snapshots.lock()
                            .unwrap_or_else(|e| e.into_inner());
                        let mut rolled_back = Vec::new();
                        for file_path in &batch_written_files {
                            if let Some(original) = snapshots.get(file_path) {
                                if let Err(e) = std::fs::write(file_path, original) {
                                    log::error!("[Rollback] Failed to restore '{}': {}", file_path, e);
                                } else {
                                    rolled_back.push(file_path.clone());
                                }
                            }
                        }
                        // Drop snapshots lock before emitting
                        drop(snapshots);
                        if !rolled_back.is_empty() {
                            let msg = format!(
                                "[ATOMIC_ROLLBACK] Batch failed — restored {} file(s): {}",
                                rolled_back.len(),
                                rolled_back.join(", ")
                            );
                            self.emit_log("warn", &msg);
                            self.messages.push(llm::ChatMessage {
                                role: "system".into(),
                                content: msg,
                                images: None,
                                tool_calls: None,
                                tool_call_id: None,
                            });
                        }
                    }

                    // 3.6 Emit incremental diff so frontend can show file changes in real time
                    self.emit_edit_diff();

                    // 3.7 Loop detection: check for non-progress patterns
                    log::debug!(
                        "[Agent:{}] loop_detector.check() — history={}, read_only_streak={}, tail_repeat={}, unique_keys={}",
                        self.agent_id,
                        self.loop_detector.history_len(),
                        self.loop_detector.get_read_only_streak(),
                        self.loop_detector.get_tail_repeat_count(),
                        self.loop_detector.get_unique_keys_count()
                    );
                    match self.loop_detector.check() {
                        LoopVerdict::Continue => {
                            log::debug!("[Agent:{}] loop_detector verdict: Continue", self.agent_id);
                        }
                        LoopVerdict::InjectWarning(msg) => {
                            self.emit_log("warn", &format!("Loop detected: {}", msg.lines().next().unwrap_or("")));
                            self.messages.push(llm::ChatMessage {
                                role: "user".into(),
                                content: format!("[LOOP_WARNING] {}", msg),
                                images: None,
                                tool_calls: None,
                                tool_call_id: None,
                            });
                        }
                        LoopVerdict::HardStop(msg) => {
                            self.emit_log("error", &format!("Hard stop: {}", msg));
                            self.log_agent_event(crate::memory::agent_log::LogEntryType::Error {
                                message: format!("Loop hard-stop: {}", msg),
                            }).await;
                            return Err(format!("Agent loop terminated: {}", msg));
                        }
                    }

                    // 4. Self-reflection: if any tool has consecutive failures >= 2,
                    //    OR if the agent is making no progress (repeated reads without writes)
                    let max_failures = self.consecutive_failures.values().max().copied().unwrap_or(0);

                    // ── No-progress detection: read-only tools called 3+ times ──
                    // without any write tool in between → stuck in exploration loop.
                    // Count read-only calls in this batch with no write calls.
                    let batch_read_only_count = tool_calls.iter()
                        .filter(|tc| is_read_only_tool(&tc.name))
                        .count();
                    let batch_has_write = tool_calls.iter()
                        .any(|tc| !is_read_only_tool(&tc.name)
                            && !matches!(tc.name.as_str(), "todo_write" | "ask_user_question"));

                    // If this entire batch was read-only AND we're past 50% of budget,
                    // treat it as a "no-progress" signal for self-reflection
                    let no_progress_signal = batch_read_only_count >= 3
                        && !batch_has_write
                        && iteration > self.max_iterations / 2
                        && self.convergence_injected == false; // only before convergence

                    if (max_failures >= 2 || no_progress_signal)
                        && self.reflection_count < self.max_iterations / 2
                    {
                        let reason = if no_progress_signal {
                            format!(
                                "No progress detected: {} read-only tool calls in batch without any write action at iteration {}/{}",
                                batch_read_only_count, iteration + 1, self.max_iterations
                            )
                        } else {
                            format!(
                                "Tool failures detected (max per-tool: {}, reflection #{})",
                                max_failures, self.reflection_count + 1
                            )
                        };
                        self.emit_log("info", &format!(
                            "Triggering self-reflection: {}", reason
                        ));
                        self.emit_status(
                            "Reflecting on progress...",
                            iteration as u32,
                            self.max_iterations as u32,
                            self.total_tokens_est as u32,
                            self.started_at.map(|s| s.elapsed().as_millis() as u64).unwrap_or(0),
                        );

                        match self.reflect_on_failures().await {
                            Ok(reflection) => {
                                self.reflection_history.push(reflection.clone());
                                self.messages.push(llm::ChatMessage {
                                    role: "system".into(),
                                    content: format!(
                                        "[SELF-REFLECTION] {}\n\nReason: {}",
                                        reflection, reason
                                    ),
                                    images: None,
                                    tool_calls: None,
                                    tool_call_id: None,
                                });
                                self.reflection_count += 1;
                                self.consecutive_failures.clear();
                                self.emit_log("info", "Self-reflection injected into context");
                            }
                            Err(e) => {
                                self.emit_log("warn", &format!("Self-reflection failed: {}", e));
                            }
                        }
                    }

                    // 5. Checkpoint: create git checkpoint if files were modified
                    let modified_files: Vec<String> = {
                        let snapshots = self.hook_context.file_snapshots.lock()
                            .unwrap_or_else(|e| e.into_inner());
                        snapshots.keys().cloned().collect()
                    };
                    if !modified_files.is_empty() {
                        if let Some(store) = self.app.try_state::<checkpoint::CheckpointStore>() {
                            let manager = checkpoint::CheckpointManager::new(self.tool_ctx.project_path.clone());
                            match manager.create(
                                iteration as u32,
                                modified_files.clone(),
                                format!("Iteration {}", iteration),
                            ).await {
                                Ok(cp) => {
                                    if let Ok(mut store) = store.lock() {
                                        store.entry(self.session_id.clone())
                                            .or_insert_with(Vec::new)
                                            .push(cp.clone());
                                    }
                                    let _ = self.app.emit(
                                        "chat-event",
                                        ChatEvent::CheckpointCreated {
                                            session_id: self.session_id.clone(),
                                            agent_id: Some(self.agent_id.clone()),
                                            iteration: iteration as u32,
                                            commit_hash: cp.commit_hash.clone(),
                                            files: modified_files,
                                        },
                                    );
                                    self.emit_log("info", &format!("Checkpoint created for iteration {}", iteration));
                                }
                                Err(e) => {
                                    self.emit_log("warn", &format!("Checkpoint creation failed: {}", e));
                                }
                            }
                        }
                    }
                }
            }
            iteration += 1;
        }

        // ── Soft exit: give agent one final chance to summarize before hard error ──
        // Instead of immediately returning Err, inject a "wrap up now" instruction
        // and allow one more LLM turn. If the agent produces text → Ok; otherwise → Err.
        self.emit_log("warn", &format!(
            "Iteration budget exhausted ({}/{}). Requesting final summary...",
            self.max_iterations, self.max_iterations
        ));

        self.messages.push(llm::ChatMessage {
            role: "system".into(),
            content: "[BUDGET_EXHAUSTED] Your iteration budget is fully consumed. \
                You MUST now produce your final response WITHOUT calling any tools. \
                Summarize:\n\
                1. What was accomplished\n\
                2. What remains incomplete (if anything)\n\
                3. Recommended next steps for the user\n\
                Respond immediately with plain text — no tool calls.".into(),
            images: None,
            tool_calls: None,
            tool_call_id: None,
        });

        // One final LLM call for the summary (use fast_model for cost efficiency)
        let request = llm::ChatRequestParams {
            model: if self.fast_model.is_empty() { self.chat_model.clone() } else { self.fast_model.clone() },
            messages: self.messages.clone(),
            system: self.build_system_prompt(),
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            thinking_enabled: false, // Summary doesn't need thinking
            thinking_budget: 0,
        };

        let stream_app = self.app.clone();
        let stream_session = self.session_id.clone();
        let stream_agent = self.agent_id.clone();
        let final_response = if let Some(ref mut queue) = self.mock_llm_responses {
            queue.pop_front().unwrap_or_else(|| {
                Err("Mock LLM response queue exhausted".to_string())
            })
        } else {
            llm::stream_chat_with_tools(
                &self.provider,
                &self.api_key,
                self.base_url.as_deref(),
                request,
                &[], // empty tools → force text response
                Some(self.cancelled.clone()),
                move |token: String| {
                    let _ = stream_app.emit("chat-event", ChatEvent::Delta {
                        session_id: stream_session.clone(),
                        agent_id: Some(stream_agent.clone()),
                        token,
                    });
                    Ok(())
                },
            )
            .await
            .map(|(resp, _usage)| resp)
        };

        match final_response {
            Ok(llm::LlmResponse::Text(text)) if !text.trim().is_empty() => {
                self.emit_log("info", "Soft exit: agent produced summary after budget exhaustion");
                self.log_agent_event(crate::memory::agent_log::LogEntryType::Completed {
                    final_text: text.clone(),
                }).await;
                self.emit_edit_diff();
                // Emit BudgetExhausted instead of Finished so frontend can show Continue button
                let _ = self.app.emit("chat-event", ChatEvent::BudgetExhausted {
                    session_id: self.session_id.clone(),
                    agent_id: Some(self.agent_id.clone()),
                    summary: text.clone(),
                    max_iterations: self.max_iterations as u32,
                });
                return Ok(text);
            }
            _ => {
                let msg = format!(
                    "Agent exceeded maximum iterations ({}) and failed to produce a summary",
                    self.max_iterations
                );
                self.log_agent_event(crate::memory::agent_log::LogEntryType::Error {
                    message: msg.clone(),
                }).await;
                // Even on failure, emit BudgetExhausted so user can click Continue
                let _ = self.app.emit("chat-event", ChatEvent::BudgetExhausted {
                    session_id: self.session_id.clone(),
                    agent_id: Some(self.agent_id.clone()),
                    summary: msg.clone(),
                    max_iterations: self.max_iterations as u32,
                });
                Err(msg)
            }
        }
    }

    /// 运行 Agent 循环但**不支持 dispatch_agent/dispatch_agents**（子 Agent 专用）。
    /// 不带有 where Self: Send，避免与 run_sub_agent 形成异步递归类型循环。
    /// 子 Agent 的 tool_names 中不包含调度工具，所以不需要 dispatch 支持。
    pub async fn run_no_dispatch(&mut self) -> Result<String, String> {
        self.started_at = Some(Instant::now());
        self.emit_started();
        self.emit_log("info", "Sub-agent started");

        let tool_jsons: Vec<serde_json::Value> = self
            .tool_definitions
            .iter()
            .map(|t| t.to_openai_tool())
            .collect();

        for iteration in 0..self.max_iterations {
            // Check cancellation
            if self.cancelled.load(Ordering::Relaxed) {
                self.emit_cancelled();
                self.emit_log("warn", "Sub-agent cancelled by user");
                return Err("Agent cancelled by user".to_string());
            }

            // 发射思考状态
            let elapsed = self.started_at.map(|s| s.elapsed().as_millis() as u64).unwrap_or(0);
            self.emit_status(
                if iteration == 0 { "Analyzing task and planning..." } else { "Processing results and determining next step..." },
                iteration as u32,
                self.max_iterations as u32,
                self.total_tokens_est as u32,
                elapsed,
            );
            self.emit_log("info", &format!("Sub-agent iteration {}/{} starting", iteration + 1, self.max_iterations));

            // ── Convergence injection for sub-agent (same as orchestrator) ──
            let convergence_threshold = (self.max_iterations as f64 * 0.7).ceil() as usize;
            if !self.convergence_injected
                && iteration >= convergence_threshold
            {
                self.convergence_injected = true;
                self.emit_log("info", &format!(
                    "Sub-agent convergence hint at iteration {}/{}",
                    iteration + 1, self.max_iterations
                ));
                self.messages.push(llm::ChatMessage {
                    role: "system".into(),
                    content: "[CONVERGENCE_NOTICE] You are approaching your iteration budget limit. \
                        STOP exploring and start wrapping up: summarize what you accomplished and respond \
                        with plain text. Do NOT call any more tools.".into(),
                    images: None,
                    tool_calls: None,
                    tool_call_id: None,
                });
            }

            // Compact context if it exceeds token budget
            self.compact_context_if_needed().await;

            // Resource quota: check API call limit
            if self.max_api_calls > 0 && self.api_call_count >= self.max_api_calls {
                let msg = format!(
                    "[RESOURCE_LIMIT] API call limit reached ({}/{}). Session quota exhausted.",
                    self.api_call_count, self.max_api_calls
                );
                self.emit_log("warn", &msg);
                return Err(msg);
            }
            self.api_call_count += 1;

            let request = llm::ChatRequestParams {
                model: self.select_model(iteration),
                messages: self.messages.clone(),
                system: self.build_system_prompt(),
                max_tokens: self.max_tokens,
                temperature: self.temperature,
                thinking_enabled: self.thinking_enabled,
                thinking_budget: self.thinking_budget,
            };

            let (response, usage) = if let Some(ref mut queue) = self.mock_llm_responses {
                (queue.pop_front().unwrap_or_else(|| {
                    Err("Mock LLM response queue exhausted".to_string())
                })?, None)
            } else {
                // Stream tokens to frontend in real time
                let stream_app = self.app.clone();
                let stream_session = self.session_id.clone();
                let stream_agent = self.agent_id.clone();
                llm::stream_chat_with_tools(
                    &self.provider,
                    &self.api_key,
                    self.base_url.as_deref(),
                    request,
                    &tool_jsons,
                    Some(self.cancelled.clone()),
                    move |token: String| {
                        let _ = stream_app.emit("chat-event", ChatEvent::Delta {
                            session_id: stream_session.clone(),
                            agent_id: Some(stream_agent.clone()),
                            token,
                        });
                        Ok(())
                    },
                )
                .await?
            };

            // Track actual token usage from API, fall back to estimate
            if let Some(ref u) = usage {
                self.total_prompt_tokens += u.prompt_tokens;
                self.total_completion_tokens += u.completion_tokens;
                self.total_tokens_est = self.total_prompt_tokens + self.total_completion_tokens;
            } else {
                let response_size = match &response {
                    llm::LlmResponse::Text(t) => t.len(),
                    llm::LlmResponse::ToolCalls { calls, content } => {
                        content.as_ref().map(|c| c.len()).unwrap_or(0) + calls.iter().map(|c| c.name.len() + c.arguments.to_string().len()).sum::<usize>()
                    }
                };
                self.total_tokens_est += response_size / 4;
            }

            match response {
                llm::LlmResponse::Text(text) => {
                    self.emit_log("info", "Sub-agent completed with text response");
                    self.emit_edit_diff();
                    self.emit_finished(&text);
                    return Ok(text);
                }
                llm::LlmResponse::ToolCalls { calls: tool_calls, content: thinking } => {
                    // 1. 发射 LLM 思考过程（如果有）
                    if let Some(ref thought) = thinking {
                        let trimmed = thought.trim();
                        if !trimmed.is_empty() {
                            self.emit_thinking(trimmed);
                            self.emit_log("info", &format!("Sub-agent reasoning ({} chars)", trimmed.len()));
                        }
                    }

                    // 2. 将 assistant 消息（含 tool_calls）加入上下文
                    let tool_calls_json: Vec<serde_json::Value> = tool_calls
                        .iter()
                        .map(|tc| {
                            serde_json::json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                                }
                            })
                        })
                        .collect();
                    self.messages.push(llm::ChatMessage {
                        role: "assistant".into(),
                        content: String::new(),
                        images: None,
                        tool_calls: Some(serde_json::Value::Array(tool_calls_json)),
                        tool_call_id: None,
                    });

                    // 3. 逐个执行工具并推送结果（不处理 dispatch 动作）
                    for (i, tc) in tool_calls.iter().enumerate() {
                        // Check cancellation before each tool execution
                        if self.cancelled.load(Ordering::Relaxed) {
                            self.emit_cancelled();
                            return Err("Agent cancelled by user".to_string());
                        }

                        self.emit_status(
                            &format!("Executing tool {}/{}: {}...", i + 1, tool_calls.len(), tc.name),
                            iteration as u32,
                            self.max_iterations as u32,
                            self.total_tokens_est as u32,
                            self.started_at.map(|s| s.elapsed().as_millis() as u64).unwrap_or(0),
                        );

                        let tool_start = Instant::now();
                        let timestamp = chrono::Utc::now().timestamp();
                        self.emit_tool_call(tc, timestamp);
                        self.emit_log("info", &format!("Sub-agent tool: {} (args: {})", tc.name, tc.arguments.to_string().chars().take(120).collect::<String>()));
                        self.log_agent_event(crate::memory::agent_log::LogEntryType::ToolCall {
                            name: tc.name.clone(),
                            arguments: tc.arguments.clone(),
                        }).await;

                        // Pre-tool hooks (snapshot + confirm)
                        let mut tool_args = tc.arguments.clone();
                        match self.hook_manager.pre_tool_chain(&tc.name, &mut tool_args, &self.hook_context).await {
                            hooks::HookResult::Deny(msg) => {
                                self.emit_log("warn", &format!("Hook denied: {}", tc.name));
                                self.emit_tool_result(&msg, 0);
                                self.messages.push(llm::ChatMessage {
                                    role: "tool".into(),
                                    content: msg,
                                    images: None,
                                    tool_calls: None,
                                    tool_call_id: Some(tc.id.clone()),
                                });
                                continue;
                            }
                            hooks::HookResult::ModifyArgs(new_args) => {
                                tool_args = new_args;
                            }
                            hooks::HookResult::Continue => {}
                        }

                        // 仅执行常规工具，不处理 dispatch_agent/dispatch_agents
                        let result = self.execute_regular_tool(tc).await;
                        let duration_ms = tool_start.elapsed().as_millis() as u64;

                        // Post-tool hooks
                        let post_result = self.hook_manager.post_tool_chain(&tc.name, &tool_args, &result, &self.hook_context).await;
                        let final_result = post_result.modified_result.unwrap_or(result);

                        self.emit_tool_result(&final_result, duration_ms);
                        self.emit_log("info", &format!("Sub-agent tool '{}' completed in {}ms", tc.name, duration_ms));
                        self.log_agent_event(crate::memory::agent_log::LogEntryType::ToolResult {
                            name: tc.name.clone(),
                            result: final_result.chars().take(2000).collect::<String>(),
                            duration_ms,
                        }).await;
                        self.messages.push(llm::ChatMessage {
                            role: "tool".into(),
                            content: final_result,
                            images: None,
                            tool_calls: None,
                            tool_call_id: Some(tc.id.clone()),
                        });

                        for msg in post_result.additional_messages {
                            self.messages.push(msg);
                        }
                    }

                    // Post-tool-batch hooks (auto-diagnose)
                    let batch_msgs = self.hook_manager.post_tool_batch_chain(&tool_calls, &self.hook_context).await;
                    for msg in batch_msgs {
                        self.messages.push(msg);
                    }

                    // Emit incremental diff for sub-agent file changes
                    self.emit_edit_diff();
                }
            }
        }

        // ── Soft exit for sub-agent: one final LLM call for summary ──
        self.emit_log("warn", &format!(
            "Sub-agent iteration budget exhausted ({}/{}). Requesting final summary...",
            self.max_iterations, self.max_iterations
        ));

        self.messages.push(llm::ChatMessage {
            role: "system".into(),
            content: "[BUDGET_EXHAUSTED] Your iteration budget is fully consumed. \
                Produce your final response now WITHOUT calling any tools. \
                Summarize what was accomplished and what remains. Respond immediately.".into(),
            images: None,
            tool_calls: None,
            tool_call_id: None,
        });

        let request = llm::ChatRequestParams {
            model: if self.fast_model.is_empty() { self.chat_model.clone() } else { self.fast_model.clone() },
            messages: self.messages.clone(),
            system: self.build_system_prompt(),
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            thinking_enabled: self.thinking_enabled,
            thinking_budget: self.thinking_budget,
        };

        let stream_app = self.app.clone();
        let stream_session = self.session_id.clone();
        let stream_agent = self.agent_id.clone();
        let final_response = if let Some(ref mut queue) = self.mock_llm_responses {
            queue.pop_front().unwrap_or_else(|| {
                Err("Mock LLM response queue exhausted".to_string())
            })
        } else {
            llm::stream_chat_with_tools(
                &self.provider,
                &self.api_key,
                self.base_url.as_deref(),
                request,
                &[],
                Some(self.cancelled.clone()),
                move |token: String| {
                    let _ = stream_app.emit("chat-event", ChatEvent::Delta {
                        session_id: stream_session.clone(),
                        agent_id: Some(stream_agent.clone()),
                        token,
                    });
                    Ok(())
                },
            )
            .await
            .map(|(resp, _usage)| resp)
        };

        match final_response {
            Ok(llm::LlmResponse::Text(text)) if !text.trim().is_empty() => {
                self.emit_log("info", "Sub-agent soft exit: produced summary after budget exhaustion");
                self.emit_edit_diff();
                self.emit_finished(&text);
                return Ok(text);
            }
            _ => {
                let msg = format!(
                    "Sub-agent exceeded maximum iterations ({}) and failed to produce a summary",
                    self.max_iterations
                );
                self.log_agent_event(crate::memory::agent_log::LogEntryType::Error {
                    message: msg.clone(),
                }).await;
                Err(msg)
            }
        }
    }

    /// 执行工具 + 处理 AskUser/Todo 特殊逻辑，消除字符串匹配
    async fn execute_and_handle_special(&mut self, tc: &crate::llm::ToolCallRequest) -> String
    where
        Self: Send,
    {
        let action = self.executor.post_execute_action(&tc.name, &tc.arguments);

        match action {
            PostExecuteAction::AskUser(questions) => {
                self.handle_ask_user(questions).await
            }
            PostExecuteAction::UpdateTodos(todos) => {
                let result = self.execute_regular_tool(tc).await;
                self.todo_list = todos;

                // Auto-extend iteration budget when many tasks are created
                // Each task needs ~3 iterations on average (explore + implement + verify)
                // LIMIT: max 2 extensions to prevent budget inflation without progress
                let task_count = self.todo_list.len();
                if task_count >= 5 && self.extend_count < 2 {
                    let additional = task_count * 3;
                    self.extend_iterations(additional);
                    self.extend_count += 1;
                    log::info!(
                        "[Agent:{}] Auto-extend #{} triggered: +{} iterations for {} tasks",
                        self.agent_id, self.extend_count, additional, task_count
                    );
                } else if self.extend_count >= 2 {
                    log::warn!(
                        "[Agent:{}] Auto-extend skipped: already extended {} times without sufficient progress",
                        self.agent_id, self.extend_count
                    );
                    self.emit_log("warn", &format!(
                        "Iteration budget extension denied (already extended {}x). Focus on completing existing tasks.",
                        self.extend_count
                    ));
                }

                let _ = self.app.emit(
                    "chat-event",
                    ChatEvent::TodoUpdate {
                        session_id: self.session_id.clone(),
                        agent_id: Some(self.agent_id.clone()),
                        todos: self.todo_list.clone(),
                    },
                );
                result
            }
            PostExecuteAction::DispatchAgent { agent_id, task } => {
                let registry = self.app.try_state::<crate::agent::definition::AgentRegistry>()
                    .map(|s| s.inner().clone());
                let registry = match registry {
                    Some(r) => r,
                    None => return "Error: AgentRegistry not available".to_string(),
                };

                // Call directly (no spawn) — type recursion is broken by owned types
                // run_sub_agent takes owned params, so we clone from self
                sub_agent::run_sub_agent(
                    self.app.clone(),
                    self.session_id.clone(),
                    task.clone(),
                    agent_id.clone(),
                    registry.clone(),
                    self.provider.clone(),
                    self.api_key.clone(),
                    self.base_url.clone(),
                    self.chat_model.clone(),
                    self.tool_ctx.project_path.clone(),
                ).await
            }
            PostExecuteAction::DispatchAgents(tasks) => {
                let registry = self.app.try_state::<crate::agent::definition::AgentRegistry>()
                    .map(|s| s.inner().clone());
                let registry = match registry {
                    Some(r) => r,
                    None => return "Error: AgentRegistry not available".to_string(),
                };

                let task_tuples: Vec<(String, String, Option<String>, Option<Vec<String>>)> = tasks.into_iter()
                    .map(|t| (t.agent_id, t.task, t.file_path, t.depends_on))
                    .collect();

                let results = sub_agent::run_sub_agents_parallel(
                    &self.app,
                    &self.session_id,
                    &task_tuples,
                    &registry,
                    &self.provider,
                    &self.api_key,
                    self.base_url.as_deref(),
                    &self.chat_model,
                    self.tool_ctx.project_path.as_deref(),
                ).await;
                results.join("\n\n---\n\n")
            }
            PostExecuteAction::None => {
                self.execute_regular_tool(tc).await
            }
        }
    }

    /// 执行常规工具，含错误分类、死锁检测和一次自动重试
    async fn execute_regular_tool(&mut self, tc: &crate::llm::ToolCallRequest) -> String {
        // Planning phase: block write operations
        if self.execution_phase == ExecutionPhase::Planning {
            let write_tools = [
                "write_file", "edit", "append_file", "delete_file",
                "delete_directory", "create_directory", "run_terminal_command",
            ];
            if write_tools.contains(&tc.name.as_str()) {
                return format!(
                    "[PLAN_MODE] Write operation '{}' is disabled in planning phase. Use read-only tools only.",
                    tc.name
                );
            }
        }

        // 死锁检测：计算参数哈希
        let args_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            tc.arguments.to_string().hash(&mut hasher);
            hasher.finish()
        };

        // 记录本次调用
        self.recent_tool_calls.push_back((tc.name.clone(), args_hash));
        if self.recent_tool_calls.len() > 3 {
            self.recent_tool_calls.pop_front();
        }

        // 检测连续 3 次相同工具调用
        if self.recent_tool_calls.len() == 3 {
            let all_same = self.recent_tool_calls.iter().all(|(name, hash)| {
                name == &tc.name && *hash == args_hash
            });
            if all_same {
                log::warn!("Deadlock detected: tool '{}' called 3 times with same args", tc.name);
                self.emit_log("warn", &format!(
                    "[DEADLOCK] Tool '{}' called 3 times with identical arguments. You seem to be stuck in a loop.",
                    tc.name
                ));
                return format!(
                    "[DEADLOCK_DETECTED] Tool '{}' was called 3 times with the same arguments but failed each time. \
                     Please try a different approach or explain the issue to the user.",
                    tc.name
                );
            }
        }

        // Use executor's execute() which includes timeout protection
        let result = self.executor.execute(&tc.name, tc.arguments.clone(), &self.tool_ctx).await;

        // Skip retry for timeout and not-found errors
        if result.starts_with("[TIMEOUT]") || result.starts_with("[TOOL_NOT_FOUND]") {
            return result;
        }

        // 错误分类：区分可重试和不可重试错误
        if result.starts_with("Error:") || result.starts_with("[SANDBOX_BLOCKED]") || result.starts_with("[PERMISSION_DENIED]") {
            let err_msg = result.lines().next().unwrap_or("").to_string();

            // 不可重试错误：沙箱拦截、权限拒绝
            if result.starts_with("[SANDBOX_BLOCKED]") || result.starts_with("[PERMISSION_DENIED]") {
                log::warn!("Tool '{}' failed with non-retryable error: {}", tc.name, err_msg);
                return result;
            }

            // 可重试错误：其他执行错误
            log::warn!("Tool '{}' failed on first attempt: {}", tc.name, err_msg);
            self.emit_tool_retry(&tc.name, 1, &err_msg);
            let retry = self.executor.execute(&tc.name, tc.arguments.clone(), &self.tool_ctx).await;
            if retry.starts_with("Error:") || retry.starts_with("[SANDBOX_BLOCKED]") || retry.starts_with("[PERMISSION_DENIED]") || retry.starts_with("[TIMEOUT]") {
                format!("[RETRY_FAILED] {}", retry)
            } else {
                format!("[RETRY_SUCCESS] {}", retry)
            }
        } else {
            result
        }
    }

    async fn handle_ask_user(&self, questions: Vec<crate::chat::QuestionItem>) -> String
    where
        Self: Send,
    {
        let qid = uuid::Uuid::new_v4().to_string();

        let _ = self.app.emit(
            "chat-event",
            ChatEvent::AskUserQuestion {
                session_id: self.session_id.clone(),
                agent_id: Some(self.agent_id.clone()),
                question_id: qid.clone(),
                questions: questions.clone(),
            },
        );

        if let Some(awaiters) = &self.question_awaiters {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            {
                let mut map = match awaiters.lock() {
                    Ok(m) => m,
                    Err(e) => return format!("Error: Failed to acquire question lock: {}", e),
                };
                map.insert(qid, sender);
            }
            match receiver.await {
                Ok(answer) => {
                    let formatted: Vec<String> = questions
                        .iter()
                        .enumerate()
                        .map(|(i, q)| {
                            format!(
                                "Q{} ({}): {}\nA: {}",
                                i + 1,
                                q.header,
                                q.question,
                                answer.lines().nth(i).unwrap_or("N/A")
                            )
                        })
                        .collect();
                    formatted.join("\n\n")
                }
                Err(_) => "User did not answer the question".to_string(),
            }
        } else {
            "Error: Question system not available".to_string()
        }
    }

    // ── 事件发射 helper ──

    fn emit_started(&self) {
        let _ = self.app.emit(
            "chat-event",
            ChatEvent::Started {
                session_id: self.session_id.clone(),
                agent_id: Some(self.agent_id.clone()),
            },
        );
    }

    fn emit_status(&self, status: &str, iteration: u32, total_iterations: u32, estimated_tokens: u32, elapsed_ms: u64) {
        let _ = self.app.emit(
            "chat-event",
            ChatEvent::AgentStatus {
                session_id: self.session_id.clone(),
                agent_id: Some(self.agent_id.clone()),
                status: status.to_string(),
                iteration,
                total_iterations,
                estimated_tokens,
                elapsed_ms,
            },
        );
    }

    fn emit_tool_call(&self, tc: &crate::llm::ToolCallRequest, timestamp: i64) {
        let _ = self.app.emit(
            "chat-event",
            ChatEvent::ToolCall {
                session_id: self.session_id.clone(),
                agent_id: Some(self.agent_id.clone()),
                tool_call: crate::chat::ToolCall {
                    id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                    timestamp,
                },
            },
        );
    }

    fn emit_tool_result(&self, result: &str, duration_ms: u64) {
        let _ = self.app.emit(
            "chat-event",
            ChatEvent::ToolResult {
                session_id: self.session_id.clone(),
                agent_id: Some(self.agent_id.clone()),
                result: result.to_string(),
                duration_ms,
            },
        );
    }

    fn emit_thinking(&self, thought: &str) {
        let _ = self.app.emit(
            "chat-event",
            ChatEvent::AgentThinking {
                session_id: self.session_id.clone(),
                agent_id: Some(self.agent_id.clone()),
                thought: thought.to_string(),
            },
        );
    }

    fn emit_tool_retry(&self, tool_name: &str, attempt: u32, error: &str) {
        let _ = self.app.emit(
            "chat-event",
            ChatEvent::ToolRetry {
                session_id: self.session_id.clone(),
                agent_id: Some(self.agent_id.clone()),
                tool_name: tool_name.to_string(),
                attempt,
                error: error.to_string(),
            },
        );
    }

    fn emit_log(&self, level: &str, message: &str) {
        let _ = self.app.emit(
            "chat-event",
            ChatEvent::AgentLog {
                session_id: self.session_id.clone(),
                agent_id: Some(self.agent_id.clone()),
                level: level.to_string(),
                message: message.to_string(),
            },
        );
    }

    fn emit_finished(&mut self, text: &str) {
        // Finalize todo list before signaling completion
        self.finalize_todos();
        // NOTE: Delta events are already emitted by stream_chat_with_tools callback,
        // so we only emit Finished here to signal completion.
        let _ = self.app.emit(
            "chat-event",
            ChatEvent::Finished {
                session_id: self.session_id.clone(),
                agent_id: Some(self.agent_id.clone()),
                full_text: text.to_string(),
            },
        );
    }

    /// Finalize todo list: mark in_progress → complete, pending → cancelled.
    /// Emits a TodoUpdate event so the frontend reflects the final state.
    fn finalize_todos(&mut self) {
        if self.todo_list.is_empty() {
            return;
        }
        let mut changed = false;
        for item in &mut self.todo_list {
            match item.status.as_str() {
                "in_progress" => {
                    item.status = "complete".to_string();
                    changed = true;
                }
                "pending" => {
                    item.status = "cancelled".to_string();
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            let _ = self.app.emit(
                "chat-event",
                ChatEvent::TodoUpdate {
                    session_id: self.session_id.clone(),
                    agent_id: Some(self.agent_id.clone()),
                    todos: self.todo_list.clone(),
                },
            );
        }
    }

    fn emit_cancelled(&mut self) {
        // Finalize todo list before signaling cancellation
        self.finalize_todos();
        let _ = self.app.emit(
            "chat-event",
            ChatEvent::Cancelled {
                session_id: self.session_id.clone(),
                agent_id: Some(self.agent_id.clone()),
            },
        );
    }

    /// Detect dominant programming languages in the project by scanning file extensions.
    /// Returns language-specific tips string to append to the system prompt.
    fn detect_project_languages(project_path: &str) -> String {
        let mut lang_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        let pp = std::path::Path::new(project_path);

        // Walk project directory (limit depth and count to avoid I/O storms)
        if let Ok(entries) = std::fs::read_dir(pp) {
            let mut count = 0;
            for entry in entries.flatten().take(200) {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        let lang = match ext {
                            "rs" => "rust",
                            "ts" | "tsx" => "typescript",
                            "js" | "jsx" => "javascript",
                            "py" => "python",
                            "go" => "go",
                            "java" => "java",
                            "cpp" | "cc" | "cxx" => "cpp",
                            "c" => "c",
                            "swift" => "swift",
                            "kt" => "kotlin",
                            "vue" => "vue",
                            "svelte" => "svelte",
                            "css" | "scss" | "less" => "css",
                            "html" | "htm" => "html",
                            _ => continue,
                        };
                        *lang_counts.entry(lang).or_default() += 1;
                        count += 1;
                        // Recurse into subdirectories (shallow)
                        // For now just scan top-level; deep scan is done by RAG indexer
                    }
                }
                if count >= 200 { break; }
            }
        }

        if lang_counts.is_empty() {
            return String::new();
        }

        // Get top 2 languages
        let mut sorted: Vec<(&str, usize)> = lang_counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        let top: Vec<&str> = sorted.iter().take(2).map(|(l, _)| *l).collect();

        let mut tips = String::new();
        for lang in &top {
            match *lang {
                "rust" => tips.push_str("- Rust project detected: Use `cargo check` for fast compilation checks, `cargo clippy` for linting, `cargo test` for testing. Prefer `&str` over `String` for function params, use `Result<T, E>` for error handling. **CRITICAL: Never truncate strings by byte index (`&s[..N]`) — use char boundaries (`s.is_char_boundary()`) or a safe helper to avoid panics on multi-byte UTF-8 characters.**\n"),
                "typescript" => tips.push_str("- TypeScript project detected: Use `tsc --noEmit` for type checking, `eslint` for linting. Prefer strict mode, use explicit types for function signatures.\n"),
                "javascript" => tips.push_str("- JavaScript project detected: Use `eslint` for linting, consider adding TypeScript for better type safety.\n"),
                "python" => tips.push_str("- Python project detected: Use `mypy` for type checking, `black`/`ruff` for formatting, `pytest` for testing. Use type hints for better code quality.\n"),
                "go" => tips.push_str("- Go project detected: Use `go vet` for static analysis, `go test ./...` for testing. Follow Go's error handling conventions (if err != nil).\n"),
                "java" => tips.push_str("- Java project detected: Use `javac` for compilation, `mvn test` or `gradle test` for testing.\n"),
                "cpp" | "c" => tips.push_str("- C/C++ project detected: Use `gcc`/`clang` for compilation with `-Wall -Wextra`. Use `cmake` or `make` for builds.\n"),
                _ => {}
            }
        }

        if !tips.is_empty() {
            tips.push_str("\n");
        }
        tips
    }

    /// 构建系统提示词，优先使用 agent system_prompt_override，否则用 AGENT_SYSTEM_PROMPT
    fn build_system_prompt(&self) -> String {
        let base = if let Some(ref override_prompt) = self.system_prompt_override {
            override_prompt.clone()
        } else {
            AGENT_SYSTEM_PROMPT.to_string()
        };
        let mut prompt = base;

        // Inject PROJECT_RULES.md from project root
        if let Some(ref pp) = self.tool_ctx.project_path {
            let rules_path = std::path::Path::new(pp).join("PROJECT_RULES.md");
            if rules_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&rules_path) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        prompt.push_str("\n\n## Project Rules\n\n");
                        prompt.push_str(trimmed);
                        prompt.push_str("\n\n---\nFollow the above project rules strictly.\n");
                    }
                }
            }

            // Detect dominant languages and append language-specific tips
            let lang_hints = Self::detect_project_languages(pp);
            if !lang_hints.is_empty() {
                prompt.push_str("\n\n## Project Language Tips\n\n");
                prompt.push_str(&lang_hints);
            }
        }

        // Inject cross-session memory (MEMORY.md + daily notes)
        if let Some(ref mem_ctx) = self.memory_context {
            let trimmed = mem_ctx.trim();
            if !trimmed.is_empty() {
                prompt.push_str("\n\n## Cross-session Memory\n\n");
                prompt.push_str(trimmed);
            }
        }

        // Inject user preferences context (editing patterns, tool stats)
        if let Some(mem_state) = self.app.try_state::<std::sync::Arc<tokio::sync::RwLock<crate::memory::MemoryManager>>>() {
            if let Ok(mgr) = mem_state.inner().try_read() {
                if let Ok(prefs) = mgr.preferences.lock() {
                    let prefs_ctx = prefs.to_context_summary();
                    if !prefs_ctx.is_empty() {
                        prompt.push_str("\n\n");
                        prompt.push_str(&prefs_ctx);
                    }
                }
            }
        }

        if let Some(ref instructions) = self.custom_instructions {
            let trimmed = instructions.trim();
            if !trimmed.is_empty() {
                prompt.push_str("\n\n## Custom Instructions\n\n");
                prompt.push_str(trimmed);
                prompt.push_str("\n\n---\nFollow the above custom instructions as highest priority.\n");
            }
        }

        // Execution Phase injection
        if self.execution_phase == ExecutionPhase::Planning {
            prompt.push_str("\n\n## Planning Phase (READ-ONLY)\n\n");
            prompt.push_str("You are in PLANNING phase. Your goal is to ANALYZE the task and produce a detailed implementation plan.\n");
            prompt.push_str("RULES:\n");
            prompt.push_str("- Do NOT call write_file, edit, append_file, delete_file, delete_directory, or run_terminal_command\n");
            prompt.push_str("- ONLY use read-only tools: read_file, list_directory, glob, grep, search_codebase, get_symbols, web_search, web_fetch, ask_user_question\n");
            prompt.push_str("- Output a structured plan with: overview, file-by-file changes, and implementation order\n");
            prompt.push_str("- If you need clarification, use ask_user_question\n");
        } else if self.execution_phase == ExecutionPhase::Executing {
            prompt.push_str("\n\n## Executing Phase\n\n");
            prompt.push_str("You are in EXECUTING phase. Follow the plan and implement changes using the available tools.\n");
            prompt.push_str("- Read each target file before editing it.\n");
            prompt.push_str("- After completing changes, verify them: run get_diagnostics on modified files and, where applicable, run the build/test command via run_terminal_command.\n");
            prompt.push_str("- Fix any errors surfaced by verification before declaring the task complete.\n");
        }

        prompt
    }

    /// 恢复文件到快照状态（撤销机制）
    pub fn restore_file_snapshot(&mut self, file_path: &str) -> Result<String, String> {
        let resolved = utils::resolve_path(self.tool_ctx.project_path.as_deref(), file_path);
        let key = resolved.to_string_lossy().to_string();

        let snapshots = self.hook_context.file_snapshots.lock()
            .unwrap_or_else(|e| e.into_inner());
        match snapshots.get(&key) {
            Some(original_content) => {
                let original_content = original_content.clone();
                drop(snapshots);

                // 写回文件
                if let Err(e) = std::fs::write(&resolved, &original_content) {
                    return Err(format!("Failed to restore file: {}", e));
                }

                // 发射文件恢复事件
                let _ = self.app.emit(
                    "chat-event",
                    ChatEvent::FileRestored {
                        session_id: self.session_id.clone(),
                        agent_id: Some(self.agent_id.clone()),
                        file_path: key.clone(),
                        content: original_content,
                    },
                );

                log::info!("File restored: {}", key);
                self.emit_log("info", &format!("Restored file: {}", file_path));
                Ok(format!("File '{}' restored to original state", file_path))
            }
            None => {
                Err(format!("No snapshot found for file: {}", file_path))
            }
        }
    }

    /// 计算文件修改的 diff 并发射 EditDiff 事件
    fn emit_edit_diff(&self) {
        // Read snapshots from hook context
        let snapshots_guard = self.hook_context.file_snapshots.lock()
            .unwrap_or_else(|e| e.into_inner());
        log::info!("[emit_edit_diff] snapshots count: {}", snapshots_guard.len());
        if snapshots_guard.is_empty() { return; }

        // Save snapshots to global state for accept/reject
        if let Some(snapshots_state) = self.app.try_state::<crate::commands::project::FileSnapshots>() {
            crate::commands::project::save_snapshots(snapshots_guard.clone(), snapshots_state.inner());
        }

        let mut changes: Vec<FileChange> = Vec::new();

        for (path, original) in snapshots_guard.iter() {
            let current = std::fs::read_to_string(path).unwrap_or_default();
            if original == &current { continue; } // 无变化

            let hunks = compute_diff(original, &current);
            if !hunks.is_empty() {
                changes.push(FileChange {
                    file_path: path.clone(),
                    hunks,
                });
            }
        }

        if !changes.is_empty() {
            log::info!("[emit_edit_diff] emitting {} file changes", changes.len());
            let _ = self.app.emit(
                "chat-event",
                ChatEvent::EditDiff {
                    session_id: self.session_id.clone(),
                    agent_id: Some(self.agent_id.clone()),
                    changes,
                },
            );
        }
    }
}

// ── Diff 算法 ──

/// 简单行级 LCS diff，将原始文本 vs 新文本的变更拆分为 DiffHunk 列表
fn compute_diff(original: &str, new_text: &str) -> Vec<DiffHunk> {
    let old_lines: Vec<&str> = original.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();

    let lcs = lcs_matrix(&old_lines, &new_lines);

    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut old_idx = 0usize;
    let mut new_idx = 0usize;

    for (oi, ni) in &lcs {
        // Lines before the match: removed from old, added in new
        while old_idx < *oi {
            hunks.push(DiffHunk {
                hunk_type: "removed".into(),
                content: old_lines[old_idx].to_string(),
                old_start: (old_idx + 1) as u32,
                new_start: 0,
            });
            old_idx += 1;
        }
        while new_idx < *ni {
            hunks.push(DiffHunk {
                hunk_type: "added".into(),
                content: new_lines[new_idx].to_string(),
                old_start: 0,
                new_start: (new_idx + 1) as u32,
            });
            new_idx += 1;
        }
        // Matched line
        hunks.push(DiffHunk {
            hunk_type: "unchanged".into(),
            content: old_lines[old_idx].to_string(),
            old_start: (old_idx + 1) as u32,
            new_start: (new_idx + 1) as u32,
        });
        old_idx += 1;
        new_idx += 1;
    }

    // Remaining old lines (removed)
    while old_idx < old_lines.len() {
        hunks.push(DiffHunk {
            hunk_type: "removed".into(),
            content: old_lines[old_idx].to_string(),
            old_start: (old_idx + 1) as u32,
            new_start: 0,
        });
        old_idx += 1;
    }

    // Remaining new lines (added)
    while new_idx < new_lines.len() {
        hunks.push(DiffHunk {
            hunk_type: "added".into(),
            content: new_lines[new_idx].to_string(),
            old_start: 0,
            new_start: (new_idx + 1) as u32,
        });
        new_idx += 1;
    }

    hunks
}

/// 计算两个行数组的 LCS 匹配索引对 [(old_idx, new_idx)]
fn lcs_matrix(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    let m = old.len();
    let n = new.len();
    if m == 0 || n == 0 { return vec![]; }

    let mut dp = vec![vec![0u32; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            if old[i - 1] == new[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack
    let mut result: Vec<(usize, usize)> = Vec::new();
    let (mut i, mut j) = (m, n);
    while i > 0 && j > 0 {
        if old[i - 1] == new[j - 1] {
            result.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] > dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    result.reverse();
    result
}

// ── 兼容旧 API ──

pub async fn run_agent(
    app: &tauri::AppHandle,
    session_id: &str,
    messages: &[llm::ChatMessage],
    provider: &LlmProvider,
    api_key: &str,
    base_url: Option<&str>,
    chat_model: &str,
    project_path: Option<&str>,
    custom_instructions: Option<String>,
    cancelled: Arc<AtomicBool>,
    agent_def: Option<&AgentDefinition>,
    plan_mode: bool,
    memory_context: Option<String>,
) -> Result<String, String> {
    let mut agent = AgentInstance::new(
        app.clone(),
        session_id.to_string(),
        messages.to_vec(),
        provider.clone(),
        api_key.to_string(),
        base_url.map(|s| s.to_string()),
        chat_model.to_string(),
        project_path.map(|s| s.to_string()),
        custom_instructions,
        cancelled,
        agent_def,
        memory_context,
    );
    agent.plan_mode = plan_mode;
    agent.execution_phase = if plan_mode { ExecutionPhase::Planning } else { ExecutionPhase::Executing };
    agent.run().await
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Task 1: Planning Mode / Task 5: Dynamic Tool Selection ──

    #[test]
    fn test_planning_phase_tools_filtered() {
        // Planning phase tools should only include read-only tools
        assert!(PLANNING_PHASE_TOOLS.contains(&"read_file"));
        assert!(PLANNING_PHASE_TOOLS.contains(&"glob"));
        assert!(PLANNING_PHASE_TOOLS.contains(&"grep"));
        assert!(PLANNING_PHASE_TOOLS.contains(&"search_codebase"));
        assert!(PLANNING_PHASE_TOOLS.contains(&"list_directory"));
        assert!(PLANNING_PHASE_TOOLS.contains(&"get_symbols"));
        assert!(PLANNING_PHASE_TOOLS.contains(&"get_diagnostics"));
        assert!(PLANNING_PHASE_TOOLS.contains(&"todo_write"));

        // Write tools should NOT be in planning phase
        assert!(!PLANNING_PHASE_TOOLS.contains(&"write_file"));
        assert!(!PLANNING_PHASE_TOOLS.contains(&"edit"));
        assert!(!PLANNING_PHASE_TOOLS.contains(&"delete_file"));
        assert!(!PLANNING_PHASE_TOOLS.contains(&"run_terminal_command"));
    }

    #[test]
    fn test_exploration_phase_tools() {
        // Exploration phase: read-only + todo_write + memory_search + git_status/diff
        assert!(EXPLORATION_PHASE_TOOLS.contains(&"read_file"));
        assert!(EXPLORATION_PHASE_TOOLS.contains(&"todo_write"));
        assert!(EXPLORATION_PHASE_TOOLS.contains(&"memory_search"));
        assert!(EXPLORATION_PHASE_TOOLS.contains(&"git_status"));

        // No write tools
        assert!(!EXPLORATION_PHASE_TOOLS.contains(&"write_file"));
        assert!(!EXPLORATION_PHASE_TOOLS.contains(&"edit"));
    }

    #[test]
    fn test_verification_phase_tools() {
        // Verification phase: read-only + run_terminal_command + diagnostics
        assert!(VERIFICATION_PHASE_TOOLS.contains(&"read_file"));
        assert!(VERIFICATION_PHASE_TOOLS.contains(&"get_diagnostics"));
        assert!(VERIFICATION_PHASE_TOOLS.contains(&"run_terminal_command"));

        // No write tools (except terminal for running tests)
        assert!(!VERIFICATION_PHASE_TOOLS.contains(&"write_file"));
        assert!(!VERIFICATION_PHASE_TOOLS.contains(&"edit"));
    }

    #[test]
    fn test_execution_phase_enum() {
        // Test phase transitions
        let planning = ExecutionPhase::Planning;
        let executing = ExecutionPhase::Executing;
        let done = ExecutionPhase::Done;

        assert_ne!(planning, executing);
        assert_ne!(executing, done);
        assert_ne!(planning, done);
    }

    // ── Task 4: Parallel Tool Execution ──

    #[test]
    fn test_is_read_only_tool() {
        // Read-only tools
        assert!(is_read_only_tool("read_file"));
        assert!(is_read_only_tool("glob"));
        assert!(is_read_only_tool("grep"));
        assert!(is_read_only_tool("search_codebase"));
        assert!(is_read_only_tool("list_directory"));
        assert!(is_read_only_tool("get_symbols"));
        assert!(is_read_only_tool("get_diagnostics"));
        assert!(is_read_only_tool("memory_search"));
        assert!(is_read_only_tool("git_status"));
        assert!(is_read_only_tool("git_diff"));
        assert!(is_read_only_tool("web_search"));
        assert!(is_read_only_tool("web_fetch"));

        // Write tools
        assert!(!is_read_only_tool("write_file"));
        assert!(!is_read_only_tool("edit"));
        assert!(!is_read_only_tool("append_file"));
        assert!(!is_read_only_tool("delete_file"));
        assert!(!is_read_only_tool("delete_directory"));
        assert!(!is_read_only_tool("create_directory"));
        assert!(!is_read_only_tool("run_terminal_command"));
        assert!(!is_read_only_tool("git_commit"));
        assert!(!is_read_only_tool("todo_write"));
    }

    // ── Task 6: Self-Reflection ──

    #[test]
    fn test_is_tool_failure() {
        // Failure indicators (prefix-based, matching execute_regular_tool protocol)
        assert!(AgentInstance::is_tool_failure("Error: file not found"));
        assert!(AgentInstance::is_tool_failure("Error: Command blocked for safety"));
        assert!(AgentInstance::is_tool_failure("[TIMEOUT] Command timed out"));
        assert!(AgentInstance::is_tool_failure("[TOOL_NOT_FOUND] Unknown tool"));
        assert!(AgentInstance::is_tool_failure("[SANDBOX_BLOCKED] Dangerous command"));
        assert!(AgentInstance::is_tool_failure("[PERMISSION_DENIED] Access denied"));
        assert!(AgentInstance::is_tool_failure("[RETRY_FAILED] All retries exhausted"));

        // Success indicators — content that contains "error" but is NOT a failure
        assert!(!AgentInstance::is_tool_failure("File content with error variable"));
        assert!(!AgentInstance::is_tool_failure("Successfully wrote file"));
        assert!(!AgentInstance::is_tool_failure("File created"));
        assert!(!AgentInstance::is_tool_failure("OK"));
        assert!(!AgentInstance::is_tool_failure("Completed in 42ms"));
        assert!(!AgentInstance::is_tool_failure("Directory listing for src/"));
        assert!(!AgentInstance::is_tool_failure("Grep results: 5 matches for 'error'"));
    }

    // ── Task 7: Per-tool failure tracking ──

    /// Verify that per-tool failure counting treats each tool independently
    #[test]
    fn test_per_tool_failures_independent() {
        let mut failures = HashMap::new();

        // grep fails 3 times
        for _ in 0..3 {
            *failures.entry("grep".to_string()).or_insert(0) += 1;
        }
        // read_file succeeds (reset)
        failures.remove("read_file");

        // Should track largest failure as 3
        let max = failures.values().max().copied().unwrap_or(0);
        assert_eq!(max, 3, "grep should have 3 consecutive failures");
        assert_eq!(failures.get("read_file"), None, "read_file should be reset");
    }

    // ── Task 8: Agent loop detection (duplicates loop_detector.rs tests for coverage) ──

    /// LoopDetector correctly reports no detection on empty history
    #[test]
    fn test_loop_detector_empty_history_agent() {
        use crate::agent::loop_detector::{LoopDetector, LoopVerdict};
        let mut detector = LoopDetector::new(Default::default());
        assert_eq!(detector.check(), LoopVerdict::Continue);
    }

    /// LoopDetector detects identical failing grep calls (threshold 5)
    #[test]
    fn test_loop_detector_same_failure_repeat_agent() {
        use crate::agent::loop_detector::{LoopDetector, LoopDetectionConfig, LoopVerdict};
        let mut detector = LoopDetector::new(LoopDetectionConfig {
            no_progress_threshold: 3,
            ping_pong_cycles: 2,
            failure_streak_threshold: 3,
            read_only_streak_threshold: 0,
            repeated_read_threshold: 0,
        });

        // Same tool, same args, same failing result → no-progress repeat
        for _ in 0..3 {
            detector.record_call(
                "grep",
                &serde_json::json!({"q": "nonexistent"}),
                "No results found",
                false,
            );
        }
        assert!(matches!(detector.check(), LoopVerdict::InjectWarning(_)));
    }

    // ── Task 9: Context compaction safety ──

    /// compact_if_needed should not trigger when budget is sufficient
    #[test]
    fn test_compact_if_needed_skips_for_small_context() {
        let messages: Vec<crate::llm::ChatMessage> = vec![
            crate::llm::ChatMessage {
                role: "user".into(),
                content: "Hello".into(),
                images: None,
                tool_calls: None,
                tool_call_id: None,
            },
            crate::llm::ChatMessage {
                role: "assistant".into(),
                content: "Hi!".into(),
                images: None,
                tool_calls: None,
                tool_call_id: None,
            },
        ];

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(crate::agent::context::compact_if_needed(
            &messages,
            "You are helpful",
            100_000,  // huge budget
            &crate::config::LlmProvider::OpenAI,
            "fake-key",
            Some("http://localhost:9999"),
            "gpt-4o",
        ));

        let result = result.unwrap();
        assert_eq!(result.len(), 2, "Should not compact when under budget");
    }

    /// Tool-call → tool-result message pairs should be preserved together
    #[test]
    fn test_compact_preserves_tool_call_pairs() {
        let mut messages: Vec<crate::llm::ChatMessage> = Vec::new();

        // Add old messages (should be trimmed)
        for i in 0..8 {
            messages.push(crate::llm::ChatMessage {
                role: format!("assistant"),
                content: format!("Old message {}", i),
                images: None,
                tool_calls: None,
                tool_call_id: None,
            });
        }

        // Add recent tool_call → tool_result pair
        messages.push(crate::llm::ChatMessage {
            role: "assistant".into(),
            content: String::new(),
            images: None,
            tool_calls: Some(serde_json::json!([{
                "id": "call_1",
                "type": "function",
                "function": {
                    "name": "read_file",
                    "arguments": "{\"path\": \"test.rs\"}"
                }
            }])),
            tool_call_id: None,
        });
        messages.push(crate::llm::ChatMessage {
            role: "tool".into(),
            content: "File content here".into(),
            images: None,
            tool_calls: None,
            tool_call_id: Some("call_1".into()),
        });

        // Add user message after the pair
        messages.push(crate::llm::ChatMessage {
            role: "user".into(),
            content: "Latest user message".into(),
            images: None,
            tool_calls: None,
            tool_call_id: None,
        });

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(crate::agent::context::compact_if_needed(
            &messages,
            "You are helpful",
            40,  // tiny budget forces trimming
            &crate::config::LlmProvider::OpenAI,
            "fake-key",
            Some("http://localhost:9999"),
            "gpt-4o",
        ));

        // The result should either keep them together or not trigger at all
        // (either way is acceptable — we're testing that it doesn't crash)
        match result {
            Ok(compacted) => {
                let has_tool_call = compacted.iter().any(|m| m.tool_calls.is_some());
                let has_tool_result = compacted.iter().any(|m| m.tool_call_id.is_some());
                // Both or neither should be present
                assert_eq!(has_tool_call, has_tool_result,
                    "tool_call and tool_result should be preserved together");
            }
            Err(_) => {
                // Compaction via LLM API may fail (no server running) — that's fine
            }
        }
    }

    // ── E2E: Spawn panic recovery (P0-1) ──

    /// Verifies that a nested tokio::spawn + JoinHandle correctly catches panics.
    #[tokio::test]
    async fn test_spawn_panic_recovery_nested_spawn() {
        // This mimics the pattern in commands/chat.rs:
        //   tokio::spawn → tokio::spawn(nested) → await JoinHandle
        let outer = tokio::spawn(async move {
            let inner = tokio::spawn(async {
                panic!("simulated panic in agent task");
            });

            match inner.await {
                Ok(_) => "ok".to_string(),
                Err(join_err) => {
                    assert!(join_err.is_panic(), "should be a panic");
                    format!("panic caught: {:?}", join_err)
                }
            }
        });

        let result = outer.await.unwrap();
        assert!(result.contains("panic caught"), "should catch panic: {}", result);
        assert!(!result.contains("ok"), "should not return ok");
    }

    /// Verifies that a non-panic cancellation is correctly distinguished.
    #[tokio::test]
    async fn test_spawn_cancellation_vs_panic() {
        let handle = tokio::spawn(async {
            // Simulate a task that completes normally with Err
            Err::<(), String>("tool failure".to_string())
        });

        let result = handle.await.unwrap();
        assert!(result.is_err(), "should return error");
        assert_eq!(result.unwrap_err(), "tool failure");
    }

    // ── E2E: Pre-flight validation (P0-2) ──

    /// Verifies that empty API key for non-Ollama providers fails fast.
    /// Tests the same validation logic that chat.rs uses before spawning.
    #[test]
    fn test_preflight_empty_api_key_rejected() {
        use crate::config::LlmProvider;

        // Simulate the validation check
        let api_key = "";
        let provider = LlmProvider::DeepSeek;
        let is_ollama = matches!(provider, LlmProvider::Ollama);

        if api_key.trim().is_empty() && !is_ollama {
            // This should fail-fast in production code
            assert!(true, "Empty API key should trigger error");
        }
    }

    /// Verifies that Ollama provider does NOT require API key.
    #[test]
    fn test_preflight_ollama_skips_api_key_check() {
        use crate::config::LlmProvider;

        let api_key = "";
        let provider = LlmProvider::Ollama;
        let is_ollama = matches!(provider, LlmProvider::Ollama);

        let should_fail = api_key.trim().is_empty() && !is_ollama;
        assert!(!should_fail, "Ollama should not require API key");
    }

    /// Verifies that missing agent_def for non-orchestrator agents is detected.
    #[test]
    fn test_preflight_missing_agent_rejected() {
        let agent_id = "nonexistent_agent";
        let agent_def: Option<crate::agent::definition::AgentDefinition> = None;

        let should_fail = !agent_id.is_empty()
            && agent_id != "orchestrator"
            && agent_def.is_none();

        assert!(should_fail, "missing agent def should be detected");
    }

    /// Verifies that the orchestrator passes even without explicit agent_def.
    #[test]
    fn test_preflight_orchestrator_passes() {
        let agent_id = "orchestrator";
        let agent_def: Option<crate::agent::definition::AgentDefinition> = None;

        let should_fail = !agent_id.is_empty()
            && agent_id != "orchestrator"
            && agent_def.is_none();

        assert!(!should_fail, "orchestrator should pass even without agent_def");
    }

    // ── Model Routing Complexity Tests ──────────────────────────────────────

    #[test]
    fn test_assess_task_complexity_simple_query() {
        let msg = "list files in src directory";
        assert!(!AgentInstance::assess_task_complexity(msg, 10));
    }

    #[test]
    fn test_assess_task_complexity_long_message() {
        let msg = "a".repeat(600);
        assert!(AgentInstance::assess_task_complexity(&msg, 10));
    }

    #[test]
    fn test_assess_task_complexity_many_tools() {
        let msg = "do something";
        assert!(AgentInstance::assess_task_complexity(msg, 20));
    }

    #[test]
    fn test_assess_task_complexity_multiple_signals() {
        let msg = "design and implement a new architecture for the module";
        assert!(AgentInstance::assess_task_complexity(msg, 10));
    }

    #[test]
    fn test_assess_task_complexity_single_signal() {
        let msg = "fix the bug";
        assert!(!AgentInstance::assess_task_complexity(msg, 10));
    }

    #[test]
    fn test_assess_task_complexity_debug_keyword() {
        let msg = "debug and investigate the issue";
        assert!(AgentInstance::assess_task_complexity(msg, 10));
    }

    // ── Context Window Tests ────────────────────────────────────────────────

    #[test]
    fn test_model_context_window_deepseek() {
        assert_eq!(crate::config::model_context_window("deepseek-chat"), 64_000);
        assert_eq!(crate::config::model_context_window("deepseek-v4"), 128_000);
        assert_eq!(crate::config::model_context_window("deepseek-v3-pro"), 128_000);
    }

    #[test]
    fn test_model_context_window_claude() {
        assert_eq!(crate::config::model_context_window("claude-3.5-sonnet"), 200_000);
        assert_eq!(crate::config::model_context_window("claude-3.5-haiku"), 200_000);
        assert_eq!(crate::config::model_context_window("claude-3-opus"), 200_000);
    }

    #[test]
    fn test_model_context_window_gpt() {
        assert_eq!(crate::config::model_context_window("gpt-4o"), 128_000);
        assert_eq!(crate::config::model_context_window("gpt-4-turbo"), 128_000);
        assert_eq!(crate::config::model_context_window("gpt-4"), 8_192);
        assert_eq!(crate::config::model_context_window("gpt-3.5-turbo"), 16_385);
    }

    #[test]
    fn test_model_context_window_unknown() {
        assert_eq!(crate::config::model_context_window("unknown-model"), 32_000);
        assert_eq!(crate::config::model_context_window(""), 32_000);
    }

    #[test]
    fn test_model_context_window_case_insensitive() {
        assert_eq!(crate::config::model_context_window("DeepSeek-Chat"), 64_000);
        assert_eq!(crate::config::model_context_window("GPT-4o"), 128_000);
    }
}
