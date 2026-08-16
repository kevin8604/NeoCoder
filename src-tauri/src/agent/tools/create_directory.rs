use super::{Tool, ToolContext};
use crate::fs_service::FileService;

pub struct CreateDirectory;

#[async_trait::async_trait]
impl Tool for CreateDirectory {
    fn name(&self) -> &str {
        "create_directory"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let raw = args["path"].as_str().unwrap_or("");
        let path = FileService::resolve(ctx.project_path.as_deref(), raw);
        match FileService::create_dir_all(&path, ctx.project_path.as_deref(), Some(&ctx.sandbox)) {
            Ok(()) => format!("Created directory: {}", path.display()),
            Err(e) => format!("Error creating directory {}: {}", path.display(), e),
        }
    }
}
