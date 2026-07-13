use crate::chat::TodoItem;
use super::{PostExecuteAction, Tool, ToolContext};

pub struct TodoWrite;

#[async_trait::async_trait]
impl Tool for TodoWrite {
    fn name(&self) -> &str {
        "todo_write"
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> String {
        let arr = match args["todos"].as_array() {
            Some(a) => a,
            None => return "Error: 'todos' must be an array".to_string(),
        };
        let total = arr.len();
        let complete = arr.iter().filter(|t| t["status"].as_str() == Some("complete")).count();
        let in_progress = arr.iter().filter(|t| t["status"].as_str() == Some("in_progress")).count();
        let pending = arr.iter().filter(|t| t["status"].as_str() == Some("pending")).count();
        format!(
            "Todo list updated: {} total ({} complete, {} in-progress, {} pending)",
            total, complete, in_progress, pending
        )
    }

    fn post_execute_action(&self, args: &serde_json::Value) -> PostExecuteAction {
        if let Some(arr) = args["todos"].as_array() {
            let todos: Vec<TodoItem> = arr
                .iter()
                .filter_map(|t| {
                    Some(TodoItem {
                        id: t["id"].as_str()?.to_string(),
                        content: t["content"].as_str()?.to_string(),
                        status: t["status"].as_str()?.to_string(),
                    })
                })
                .collect();
            PostExecuteAction::UpdateTodos(todos)
        } else {
            PostExecuteAction::None
        }
    }
}
