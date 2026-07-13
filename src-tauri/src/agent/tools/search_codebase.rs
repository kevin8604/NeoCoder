use super::{Tool, ToolContext};

pub struct SearchCodebase;

#[async_trait::async_trait]
impl Tool for SearchCodebase {
    fn name(&self) -> &str {
        "search_codebase"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let query = args["query"].as_str().unwrap_or("");
        let max_results = args["max_results"].as_u64().unwrap_or(10) as usize;
        if query.is_empty() {
            return "Error: search query is required".to_string();
        }
        if let Some(indexer) = &ctx.indexer {
            match indexer.hybrid_search(query, max_results).await {
                Ok(results) => {
                    if results.is_empty() {
                        format!("No search results found for: '{}'", query)
                    } else {
                        let mut output = format!(
                            "Search results for '{}' ({} results):\n",
                            query,
                            results.len()
                        );
                        for (i, r) in results.iter().enumerate() {
                            let lines = if r.chunk.start_line == r.chunk.end_line {
                                format!("line {}", r.chunk.start_line)
                            } else {
                                format!("lines {}-{}", r.chunk.start_line, r.chunk.end_line)
                            };
                            output.push_str(&format!(
                                "{}. {} ({}, score: {:.3})\n```\n{}\n```\n\n",
                                i + 1, r.chunk.file_path, lines, r.score, r.chunk.content
                            ));
                        }
                        output
                    }
                }
                Err(e) => format!("Search error: {}", e),
            }
        } else {
            "Error: Code indexer not available. Please index the project first.".to_string()
        }
    }
}
