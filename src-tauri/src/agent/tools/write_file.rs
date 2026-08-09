use crate::fs_service::FileService;
use super::{Tool, ToolContext};

pub struct WriteFile;

#[async_trait::async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let raw = args["path"].as_str().unwrap_or("");
        let path = FileService::resolve(ctx.project_path.as_deref(), raw);
        let create_only = args["create_only"].as_bool().unwrap_or(false);

        // Check if file exists for overwrite protection
        let existed = path.exists();
        if existed && create_only {
            return format!(
                "Error: File {} already exists and create_only is true. Use edit tool to modify existing files.",
                path.display()
            );
        }

        let contents = args["contents"].as_str().unwrap_or("");

        // Read old content for diff summary if overwriting
        let old_content = if existed {
            std::fs::read_to_string(&path).ok()
        } else {
            None
        };

        match FileService::write_text(&path, contents, ctx.project_path.as_deref(), Some(&ctx.sandbox), create_only) {
            Ok(()) => {
                let new_lines = contents.lines().count();
                if let Some(old) = old_content {
                    let old_lines = old.lines().count();
                    let diff = new_lines as i64 - old_lines as i64;
                    let diff_str = if diff > 0 {
                        format!("+{} lines", diff)
                    } else if diff < 0 {
                        format!("{} lines", diff)
                    } else {
                        "same line count".to_string()
                    };
                    format!(
                        "Successfully overwrote {} ({} bytes, {} lines, {})",
                        path.display(), contents.len(), new_lines, diff_str
                    )
                } else {
                    format!(
                        "Successfully created {} ({} bytes, {} lines)",
                        path.display(), contents.len(), new_lines
                    )
                }
            }
            Err(e) => format!("Error writing to {}: {}", path.display(), e),
        }
    }
}
