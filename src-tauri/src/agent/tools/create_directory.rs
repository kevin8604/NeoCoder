use crate::agent::utils::resolve_path;
use super::{Tool, ToolContext};

pub struct CreateDirectory;

#[async_trait::async_trait]
impl Tool for CreateDirectory {
    fn name(&self) -> &str {
        "create_directory"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let raw = args["path"].as_str().unwrap_or("");
        let path = resolve_path(ctx.project_path.as_deref(), raw);
        // Sandbox: check write access
        if let Err(e) = ctx.sandbox.check_path(&path, ctx.project_path.as_deref(), true) {
            return format!("Error: Sandbox blocked: {}", e);
        }
        match std::fs::create_dir_all(&path) {
            Ok(()) => format!("Created directory: {}", path.display()),
            Err(e) => format!("Error creating directory {}: {}", path.display(), e),
        }
    }
}
