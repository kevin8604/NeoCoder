use std::process::Command;
use super::{Tool, ToolContext};

pub struct GitLog;

#[async_trait::async_trait]
impl Tool for GitLog {
    fn name(&self) -> &str {
        "git_log"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let max_count = args["max_count"].as_u64().unwrap_or(20);
        let project_path = ctx.project_path.as_deref().unwrap_or(".");

        let output = Command::new("git")
            .args(["log", "--oneline", "-n", &max_count.to_string()])
            .current_dir(project_path)
            .output();

        match output {
            Ok(o) if o.status.success() => {
                let log = String::from_utf8_lossy(&o.stdout);
                if log.trim().is_empty() {
                    "No commits found.".to_string()
                } else {
                    format!("Recent commits:\n{}", log)
                }
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                format!("Error: git log failed: {}", err.trim())
            }
            Err(e) => format!("Error executing git log: {}", e),
        }
    }
}
