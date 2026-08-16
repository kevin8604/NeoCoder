use super::{Tool, ToolContext};
use crate::agent::utils::resolve_path;
use std::process::Command;

pub struct GitBlame;

#[async_trait::async_trait]
impl Tool for GitBlame {
    fn name(&self) -> &str {
        "git_blame"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let raw_path = args["file_path"].as_str().unwrap_or("");
        let file_path = resolve_path(ctx.project_path.as_deref(), raw_path);

        if let Err(e) = ctx
            .sandbox
            .check_path(&file_path, ctx.project_path.as_deref(), false)
        {
            return format!("Error: Sandbox blocked: {}", e);
        }
        if !file_path.exists() {
            return format!("Error: File not found: {}", file_path.display());
        }

        let project_path = ctx.project_path.as_deref().unwrap_or(".");
        let output = Command::new("git")
            .args(["blame", "--date=short", "-w"])
            .arg(&file_path)
            .current_dir(project_path)
            .output();

        match output {
            Ok(o) if o.status.success() => {
                let blame = String::from_utf8_lossy(&o.stdout);
                if blame.trim().is_empty() {
                    "No blame information available.".to_string()
                } else {
                    let lines: Vec<&str> = blame.lines().take(100).collect();
                    if lines.len() < blame.lines().count() {
                        format!(
                            "Blame for {} (first 100 lines):\n{}",
                            file_path.display(),
                            lines.join("\n")
                        )
                    } else {
                        format!("Blame for {}:\n{}", file_path.display(), lines.join("\n"))
                    }
                }
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                format!("Error: git blame failed: {}", err.trim())
            }
            Err(e) => format!("Error executing git blame: {}", e),
        }
    }
}
