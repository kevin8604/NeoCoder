use super::{Tool, ToolContext};
use tauri::Manager;

/// Search cross-session memory (MEMORY.md, daily notes, session history).
/// Uses the MemoryManager's semantic search to find relevant past learnings.
pub struct MemorySearch;

#[async_trait::async_trait]
impl Tool for MemorySearch {
    fn name(&self) -> &str {
        "memory_search"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let query = args["query"].as_str().unwrap_or("");
        if query.is_empty() {
            return "Error: search query is required".to_string();
        }
        let max_results = args["max_results"].as_u64().unwrap_or(5) as usize;

        // Access MemoryManager through Tauri state
        let app = match &ctx.app_handle {
            Some(a) => a,
            None => return "Error: app handle not available".to_string(),
        };

        // Try to get ChatState from app state
        let chat_state = match app.try_state::<crate::commands::chat::ChatState>() {
            Some(s) => s,
            None => return "Error: chat state not available".to_string(),
        };

        let memory = chat_state.memory.read().await;
        let manager = memory.memory_manager();

        match manager.search_memory(query, max_results) {
            Ok(results) if results.is_empty() => {
                format!("No memory results found for: '{}'", query)
            }
            Ok(results) => {
                let mut output = format!("Memory search results for '{}':\n\n", query);
                for (i, r) in results.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. [relevance: {:.2}] {}:{}\n   {}\n\n",
                        i + 1,
                        r.relevance,
                        r.file_path,
                        r.line_number,
                        r.line_content.chars().take(500).collect::<String>(),
                    ));
                }
                output
            }
            Err(e) => format!("Error searching memory: {}", e),
        }
    }
}
