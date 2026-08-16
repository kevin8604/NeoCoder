use super::{Tool, ToolContext};
use crate::agent::utils::SKIP_DIRS;

pub struct Glob;

#[async_trait::async_trait]
impl Tool for Glob {
    fn name(&self) -> &str {
        "glob"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let pattern = args["pattern"].as_str().unwrap_or("");
        if pattern.is_empty() {
            return "Error: glob pattern is required".to_string();
        }
        let base = args["path"]
            .as_str()
            .or(ctx.project_path.as_deref())
            .unwrap_or(".");
        // Normalize Cygwin/MSYS2 paths on Windows
        let base_owned = crate::agent::utils::normalize_cygwin_path(base);
        let base = base_owned.as_str();
        // Sandbox: check read access on base directory
        let base_path = std::path::Path::new(base);
        if let Err(e) = ctx
            .sandbox
            .check_path(base_path, ctx.project_path.as_deref(), false)
        {
            return format!("Error: Sandbox blocked: {}", e);
        }
        let full_pattern = format!(
            "{}/**/{}",
            base.trim_end_matches('/'),
            pattern.trim_start_matches('/')
        );
        let max_results = 200usize;
        let skip_set: std::collections::HashSet<&str> = SKIP_DIRS.iter().copied().collect();

        let mut results: Vec<String> = Vec::new();
        if let Ok(iter) = glob::glob(&full_pattern) {
            for entry in iter.flatten() {
                if results.len() >= max_results {
                    break;
                }
                let path_str = entry.to_string_lossy().to_string();
                let skip = path_str
                    .split(std::path::MAIN_SEPARATOR)
                    .any(|c| skip_set.contains(c));
                if !skip {
                    results.push(path_str);
                }
            }
        }
        if results.is_empty() {
            format!("No files matching pattern '{}' found in {}", pattern, base)
        } else {
            let mut output = format!(
                "Files matching '{}' ({} results):\n",
                pattern,
                results.len()
            );
            for r in &results {
                output.push_str(&format!("  {}\n", r));
            }
            if results.len() >= max_results {
                output.push_str(&format!("\n... truncated at {} results", max_results));
            }
            output
        }
    }
}
