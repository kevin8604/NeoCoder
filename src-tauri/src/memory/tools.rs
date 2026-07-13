use crate::memory::MemoryManager;

/// Read MEMORY.md or a specific memory file.
pub struct MemoryRead;

impl MemoryRead {
    pub fn execute(_args: serde_json::Value, manager: &MemoryManager) -> String {
        match manager.read_long_term() {
            Ok(content) => {
                if content.is_empty() {
                    "No long-term memory stored yet.".to_string()
                } else {
                    format!("--- MEMORY.md ---\n{}\n--- end ---", content)
                }
            }
            Err(e) => format!("Error reading memory: {}", e),
        }
    }
}

/// Write or append to MEMORY.md. Args: { "content": "...", "action": "overwrite|append", "section": "..." }
pub struct MemoryWrite;

impl MemoryWrite {
    pub fn execute(args: serde_json::Value, manager: &MemoryManager) -> String {
        let content = args.get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let action = args.get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("overwrite");
        let section = args.get("section")
            .and_then(|v| v.as_str());

        if content.is_empty() {
            return "Error: content is required".to_string();
        }

        match action {
            "append" => {
                let sec = section.unwrap_or("Notes");
                match manager.append_long_term(sec, content) {
                    Ok(()) => format!("Appended to MEMORY.md (section: {})", sec),
                    Err(e) => format!("Error writing to memory: {}", e),
                }
            }
            _ => {
                match manager.write_long_term(content) {
                    Ok(()) => "MEMORY.md updated successfully.".to_string(),
                    Err(e) => format!("Error writing to memory: {}", e),
                }
            }
        }
    }
}

/// Search memory files by keyword.
pub struct MemorySearch;

impl MemorySearch {
    pub fn execute(args: serde_json::Value, manager: &MemoryManager) -> String {
        let query = args.get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let max_results = args.get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as usize;

        if query.is_empty() {
            return "Error: query is required".to_string();
        }

        match manager.search_memory(query, max_results) {
            Ok(results) => {
                if results.is_empty() {
                    format!("No memory results found for: {}", query)
                } else {
                    let mut output = format!("Memory search results for '{}':\n\n", query);
                    for (i, r) in results.iter().enumerate() {
                        output.push_str(&format!(
                            "{}. [{}:{}] (score: {:.1})\n   {}\n\n",
                            i + 1, r.file_path, r.line_number, r.relevance, r.line_content.trim()
                        ));
                    }
                    output
                }
            }
            Err(e) => format!("Error searching memory: {}", e),
        }
    }
}
