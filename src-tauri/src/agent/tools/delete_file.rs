use crate::agent::utils::resolve_path;
use super::{Tool, ToolContext};

pub struct DeleteFile;

#[async_trait::async_trait]
impl Tool for DeleteFile {
    fn name(&self) -> &str {
        "delete_file"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let raw = args["path"].as_str().unwrap_or("");
        let path = resolve_path(ctx.project_path.as_deref(), raw);
        if let Err(e) = ctx.sandbox.check_path(&path, ctx.project_path.as_deref(), true) {
            return format!("Error: Sandbox blocked: {}", e);
        }

        // Gather info before deletion
        let info = std::fs::metadata(&path).ok();
        let size = info.as_ref().map(|m| m.len()).unwrap_or(0);
        let line_count = std::fs::read_to_string(&path)
            .ok()
            .map(|c| c.lines().count())
            .unwrap_or(0);

        let size_str = if size < 1024 {
            format!("{}B", size)
        } else if size < 1024 * 1024 {
            format!("{:.1}KB", size as f64 / 1024.0)
        } else {
            format!("{:.1}MB", size as f64 / (1024.0 * 1024.0))
        };

        match std::fs::remove_file(&path) {
            Ok(()) => format!(
                "Deleted file: {} ({}, {} lines)",
                path.display(), size_str, line_count
            ),
            Err(e) => format!("Error deleting file {}: {}", path.display(), e),
        }
    }
}
