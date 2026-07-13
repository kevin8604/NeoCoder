use super::{Tool, ToolContext};

/// Git status tool: shows working tree status (modified, staged, untracked files).
pub struct GitStatus;

#[async_trait::async_trait]
impl Tool for GitStatus {
    fn name(&self) -> &str {
        "git_status"
    }

    async fn execute(&self, _args: serde_json::Value, ctx: &ToolContext) -> String {
        let work_dir = ctx.project_path.as_deref().unwrap_or(".");

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            tokio::process::Command::new("git")
                .arg("status")
                .arg("--porcelain")
                .arg("--branch")
                .current_dir(work_dir)
                .output(),
        )
        .await;

        match output {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !out.status.success() {
                    return format!("git status failed: {}", stderr.trim());
                }
                if stdout.is_empty() {
                    "Working tree clean. No changes detected.".to_string()
                } else {
                    format!("Git status:\n{}", stdout)
                }
            }
            Ok(Err(e)) => format!("Failed to execute git status: {}", e),
            Err(_) => "git status timed out (15s)".to_string(),
        }
    }
}
