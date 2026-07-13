use std::path::Path;
use std::io::Write;
use crate::agent::utils::resolve_path;
use super::{Tool, ToolContext};

pub struct AppendFile;

#[async_trait::async_trait]
impl Tool for AppendFile {
    fn name(&self) -> &str {
        "append_file"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let raw = args["path"].as_str().unwrap_or("");
        let path = resolve_path(ctx.project_path.as_deref(), raw);
        if let Err(e) = ctx.sandbox.check_path(&path, ctx.project_path.as_deref(), true) {
            return format!("Error: Sandbox blocked: {}", e);
        }
        let contents = args["contents"].as_str().unwrap_or("");
        if let Some(parent) = Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // Count existing lines before appending
        let existing_lines = std::fs::read_to_string(&path)
            .ok()
            .map(|c| c.lines().count())
            .unwrap_or(0);
        let append_start_line = existing_lines + 1;

        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(mut file) => match file.write_all(contents.as_bytes()) {
                Ok(()) => {
                    let appended_lines = contents.lines().count();
                    format!(
                        "Successfully appended {} bytes ({} lines) to {} starting at line {}",
                        contents.len(), appended_lines, path.display(), append_start_line
                    )
                }
                Err(e) => format!("Error appending to {}: {}", path.display(), e),
            },
            Err(e) => format!("Error opening {}: {}", path.display(), e),
        }
    }
}
