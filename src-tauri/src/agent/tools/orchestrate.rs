use crate::agent::tools::{ToolContext, PostExecuteAction, DispatchTask};

/// `dispatch_agent` 工具 —— 串行调度一个子 Agent
pub struct DispatchAgent;

#[async_trait::async_trait]
impl super::Tool for DispatchAgent {
    fn name(&self) -> &str {
        "dispatch_agent"
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> String {
        let agent_id = args.get("agent_id")
            .and_then(|v| v.as_str())
            .unwrap_or("code_writer")
            .to_string();
        let task = args.get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if task.is_empty() {
            return "Error: task is required".to_string();
        }

        format!("Dispatching agent '{}' with task: {}", agent_id, task)
    }

    fn post_execute_action(&self, args: &serde_json::Value) -> PostExecuteAction {
        let agent_id = args.get("agent_id")
            .and_then(|v| v.as_str())
            .unwrap_or("code_writer")
            .to_string();
        let task = args.get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        PostExecuteAction::DispatchAgent { agent_id, task }
    }
}

/// `dispatch_agents` 工具 —— 并行调度多个子 Agent
pub struct DispatchAgents;

#[async_trait::async_trait]
impl super::Tool for DispatchAgents {
    fn name(&self) -> &str {
        "dispatch_agents"
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> String {
        let tasks = args.get("tasks")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter().map(|t| {
                    let agent_id = t.get("agent_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("code_writer")
                        .to_string();
                    let task = t.get("task")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let file_path = t.get("file_path")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let depends_on = t.get("depends_on")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Vec<_>>()
                        );
                    (agent_id, task, file_path, depends_on)
                }).collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if tasks.is_empty() {
            return "Error: tasks array is required and must not be empty".to_string();
        }

        let result_str = tasks.iter()
            .map(|(id, task, _fp, _deps)| format!("Agent '{}': \"{}\"", id, task))
            .collect::<Vec<_>>()
            .join(", ");

        format!("Dispatching {} agents: {}", tasks.len(), result_str)
    }

    fn post_execute_action(&self, args: &serde_json::Value) -> PostExecuteAction {
        let tasks = args.get("tasks")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter().map(|t| {
                    let agent_id = t.get("agent_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("code_writer")
                        .to_string();
                    let task = t.get("task")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let file_path = t.get("file_path")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let depends_on = t.get("depends_on")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Vec<_>>()
                        );
                    DispatchTask { agent_id, task, file_path, depends_on }
                }).collect()
            })
            .unwrap_or_default();
        PostExecuteAction::DispatchAgents(tasks)
    }
}
