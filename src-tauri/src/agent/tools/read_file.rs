use crate::fs_service::FileService;
use super::{Tool, ToolContext};

pub struct ReadFile;

#[async_trait::async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let raw = args["path"].as_str().unwrap_or("");
        let path = FileService::resolve(ctx.project_path.as_deref(), raw);

        // Optional start_line / end_line (1-based, inclusive)
        let start_line = args["start_line"].as_u64().map(|v| v as usize);
        let end_line = args["end_line"].as_u64().map(|v| v as usize);

        match FileService::read_text(&path, ctx.project_path.as_deref(), Some(&ctx.sandbox)) {
            Ok(content) => {
                let all_lines: Vec<&str> = content.lines().collect();
                let total_lines = all_lines.len();

                // Determine effective range
                let s = start_line.unwrap_or(1).max(1);
                let e = end_line.unwrap_or(total_lines).min(total_lines);

                if s > total_lines {
                    return format!(
                        "Error: start_line {} exceeds file length ({} lines) for {}",
                        s, total_lines, path.display()
                    );
                }

                if s > e {
                    return format!(
                        "Error: start_line ({}) must not exceed end_line ({}) for {}",
                        s, e, path.display()
                    );
                }

                let is_partial = start_line.is_some() || end_line.is_some();
                let slice = &all_lines[(s - 1)..e];

                // Build output with line numbers
                let mut output = String::new();
                let header = if is_partial {
                    format!(
                        "File {} (lines {}-{} of {}):\n",
                        path.display(), s, e, total_lines
                    )
                } else {
                    format!("File {} ({} lines):\n", path.display(), total_lines)
                };
                output.push_str(&header);
                output.push_str("```\n");
                for (i, line) in slice.iter().enumerate() {
                    output.push_str(&format!("{:>4}\t{}\n", s + i, line));
                }
                output.push_str("```\n");

                // If no range specified and file is very large, add a hint
                if !is_partial && total_lines > 500 {
                    output.push_str(&format!(
                        "\n[Large file: {} lines. Use start_line/end_line to read specific ranges, e.g. {{\"path\": \"{}\", \"start_line\": 1, \"end_line\": 200}}]\n",
                        total_lines, raw
                    ));
                }

                output
            }
            Err(e) => format!("Error reading file {}: {}", path.display(), e),
        }
    }
}

