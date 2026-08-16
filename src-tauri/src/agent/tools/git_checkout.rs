use super::{Tool, ToolContext};

/// Git checkout tool: switches branches or restores working tree files.
pub struct GitCheckout;

#[async_trait::async_trait]
impl Tool for GitCheckout {
    fn name(&self) -> &str {
        "git_checkout"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let work_dir = ctx.project_path.as_deref().unwrap_or(".");
        let branch = args["branch"].as_str().unwrap_or("");
        let create = args["create"].as_bool().unwrap_or(false);
        let file_path = args["file"].as_str();

        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("checkout");

        if create {
            cmd.arg("-b");
        }

        if !branch.is_empty() {
            cmd.arg(branch);
        }

        // If a specific file is provided, restore it from HEAD
        if let Some(file) = file_path {
            cmd.arg("--");
            cmd.arg(file);
        }

        cmd.current_dir(work_dir);

        let output = tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output()).await;

        match output {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !out.status.success() {
                    let combined = format!("{}\n{}", stdout, stderr).trim().to_string();
                    format!("git checkout failed: {}", combined)
                } else {
                    if let Some(file) = file_path {
                        format!("Restored file: {}", file)
                    } else if create {
                        format!("Created and switched to branch: {}", branch)
                    } else {
                        format!("Switched to: {}", branch)
                    }
                }
            }
            Ok(Err(e)) => format!("Failed to execute git checkout: {}", e),
            Err(_) => "git checkout timed out (15s)".to_string(),
        }
    }
}
