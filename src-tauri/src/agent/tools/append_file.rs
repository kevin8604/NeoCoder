use super::{Tool, ToolContext};
use crate::fs_service::FileService;

pub struct AppendFile;

#[async_trait::async_trait]
impl Tool for AppendFile {
    fn name(&self) -> &str {
        "append_file"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let raw = args["path"].as_str().unwrap_or("");
        let path = FileService::resolve(ctx.project_path.as_deref(), raw);
        let contents = args["contents"].as_str().unwrap_or("");

        // Count existing lines before appending
        let existing_lines = std::fs::read_to_string(&path)
            .ok()
            .map(|c| c.lines().count())
            .unwrap_or(0);
        let append_start_line = existing_lines + 1;

        match FileService::append_text(
            &path,
            contents,
            ctx.project_path.as_deref(),
            Some(&ctx.sandbox),
        ) {
            Ok(()) => {
                let appended_lines = contents.lines().count();
                format!(
                    "Successfully appended {} bytes ({} lines) to {} starting at line {}",
                    contents.len(),
                    appended_lines,
                    path.display(),
                    append_start_line
                )
            }
            Err(e) => format!("Error appending to {}: {}", path.display(), e),
        }
    }
}
