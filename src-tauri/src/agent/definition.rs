use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Agent 定义 —— 描述一个可运行的 Agent 类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub tool_names: Vec<String>,
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_iterations: Option<usize>,
    pub max_tokens: Option<u32>,
}

/// Agent 注册表 —— 全局共享的所有可用 Agent 定义
pub type AgentRegistry = Arc<Vec<AgentDefinition>>;

/// 从默认路径加载 agents.json，fallback 到内嵌默认值
/// 同时合并用户自定义 agents（从 app_config_dir/custom_agents.json）
pub fn load_agents_from_disk() -> Vec<AgentDefinition> {
    let mut agents = load_agents_from_disk_except_custom();

    // Also try to load user-custom agents from app config dir
    if let Some(proj_dirs) = directories::ProjectDirs::from("com", "neocoder", "NeoCoder") {
        let custom_path = proj_dirs.config_dir().join("custom_agents.json");
        if let Ok(content) = std::fs::read_to_string(&custom_path)
            && let Ok(custom_agents) = serde_json::from_str::<Vec<AgentDefinition>>(&content)
        {
            for custom in custom_agents {
                if let Some(existing) = agents.iter_mut().find(|a| a.id == custom.id) {
                    *existing = custom;
                } else {
                    agents.push(custom);
                }
            }
        }
    }

    agents
}

/// 从默认路径加载 agents.json（不含用户自定义 agents）
/// 这个函数供 get_all_agents 使用，内置 agents 和自定义 agents 分别加载后合并
pub fn load_agents_from_disk_except_custom() -> Vec<AgentDefinition> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let path = dir.join("agents.json");
        if let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(agents) = serde_json::from_str::<Vec<AgentDefinition>>(&content)
            && !agents.is_empty()
        {
            return agents;
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let path = cwd.join("agents.json");
        if let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(agents) = serde_json::from_str::<Vec<AgentDefinition>>(&content)
            && !agents.is_empty()
        {
            return agents;
        }
    }
    // Fallback to embedded default agents
    default_agents()
}

/// 根据 agent_id 查找定义
pub fn find_agent(registry: &AgentRegistry, agent_id: &str) -> Option<AgentDefinition> {
    registry.iter().find(|a| a.id == agent_id).cloned()
}

/// 默认 Agent 定义（内嵌 fallback）
pub fn default_agents() -> Vec<AgentDefinition> {
    vec![
        AgentDefinition {
            id: "orchestrator".into(),
            name: "Orchestrator".into(),
            description: "Master agent that analyzes tasks and delegates to specialized sub-agents".into(),
            system_prompt: r#"You are an Orchestrator AI — a master agent that coordinates specialized sub-agents to accomplish complex tasks.

            ## Workflow
            1. Analyze the user's request carefully
            2. Break down complex tasks into independent sub-tasks
            3. Use dispatch_agent or dispatch_agents to delegate work to specialized agents
            4. Collect results and synthesize them into a complete response

            ## Guidelines
            - For single-file changes or simple questions, handle them yourself using available tools
            - For complex multi-file changes, use dispatch_agents to run tasks in parallel
            - For sequential work (design -> implement -> review), use dispatch_agent sequentially
            - Prefer handling the task directly; only delegate when the work is genuinely large or benefits from a specialized agent (avoid over-delegation of trivial tasks)
            - When synthesizing sub-agent results, resolve conflicts explicitly and present one coherent final answer with file paths and a brief summary of what changed
            - Do not dispatch an agent if you can handle the task directly"#.into(),
            tool_names: vec![
                "read_file".into(), "write_file".into(), "edit".into(), "glob".into(),
                "grep".into(), "search_codebase".into(), "list_directory".into(),
                "run_terminal_command".into(), "todo_write".into(), "web_search".into(),
                "web_fetch".into(), "get_symbols".into(), "get_diagnostics".into(),
                "dispatch_agent".into(), "dispatch_agents".into(), "a2a_invoke".into(),
            ],
            model: None,
            temperature: None,
            max_iterations: Some(200),
            max_tokens: None,
        },
        AgentDefinition {
            id: "code_writer".into(),
            name: "Code Writer".into(),
            description: "Specialized in reading, writing and editing code files".into(),
            system_prompt: r#"You are a Code Writer agent — specialized in reading, writing, and editing source code.

            ## Guidelines
            - Read files first to understand existing code before making changes (never edit a file you haven't read)
            - Prefer the Edit tool over write_file for precise changes
            - Make minimal, targeted changes; preserve existing indentation and style; do not refactor unrelated code
            - For large new files (>150 lines), write a skeleton first, then use edit to fill in sections incrementally
            - After editing, use get_diagnostics to verify the file compiles/lints cleanly, and fix any errors before finishing
            - When truncating strings in Rust, use char boundaries (never byte slicing like &s[..N]) to avoid UTF-8 panics
            - Do NOT run terminal commands or search the web
            - Focus only on the task assigned to you"#.into(),
            tool_names: vec![
                "read_file".into(), "write_file".into(), "append_file".into(),
                "edit".into(), "delete_file".into(), "glob".into(),
                "grep".into(), "search_codebase".into(), "list_directory".into(),
                "create_directory".into(), "delete_directory".into(),
                "get_symbols".into(), "get_diagnostics".into(),
            ],
            model: None,
            temperature: Some(0.6),
            max_iterations: Some(10),
            max_tokens: Some(4096),
        },
        AgentDefinition {
            id: "debugger".into(),
            name: "Debugger".into(),
            description: "Specialized in debugging, error analysis and root cause investigation".into(),
            system_prompt: r#"You are a Debugger agent — specialized in diagnosing bugs, analyzing errors, and finding root causes.

        ## Guidelines
        - Read relevant source files thoroughly
        - Use grep and search_codebase to trace code paths
        - Use run_terminal_command to run tests or build the project
        - Use get_diagnostics to check for compilation/lint errors
        - Do NOT modify files unless absolutely necessary for debugging
        - Provide a clear root cause analysis and fix recommendations"#.into(),
            tool_names: vec![
                "read_file".into(), "glob".into(), "grep".into(),
                "search_codebase".into(), "list_directory".into(),
                "run_terminal_command".into(), "get_symbols".into(),
                "get_diagnostics".into(), "web_search".into(), "web_fetch".into(),
            ],
            model: None,
            temperature: Some(0.5),
            max_iterations: Some(8),
            max_tokens: None,
        },
        AgentDefinition {
            id: "reviewer".into(),
            name: "Code Reviewer".into(),
            description: "Specialized in code review, quality assessment and best practices".into(),
            system_prompt: r#"You are a Code Reviewer agent — specialized in reviewing code for quality, correctness, and best practices.

            ## Guidelines
            - Read files and analyze code structure
            - Check for: bugs, security issues, performance problems, style violations
            - Use grep and search_codebase to understand the broader codebase context
            - Do NOT modify any files — you are read-only
            - Classify each finding by severity: [P0] critical (crashes, data loss, security), [P1] major (logic bugs, resource leaks), [P2] moderate (edge cases, performance), [P3] minor (style, naming)
            - Provide detailed feedback with specific file paths and line references, plus a concrete fix suggestion for each finding"#.into(),
            tool_names: vec![
                "read_file".into(), "glob".into(), "grep".into(),
                "search_codebase".into(), "list_directory".into(),
                "get_symbols".into(), "get_diagnostics".into(),
            ],
            model: None,
            temperature: Some(0.3),
            max_iterations: Some(5),
            max_tokens: None,
        },
    ]
}
