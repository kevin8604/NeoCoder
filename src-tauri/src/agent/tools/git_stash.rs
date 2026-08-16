use super::{Tool, ToolContext};

/// Git stash tool: saves or restores uncommitted changes.
pub struct GitStash;

#[async_trait::async_trait]
impl Tool for GitStash {
    fn name(&self) -> &str {
        "git_stash"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let work_dir = ctx.project_path.as_deref().unwrap_or(".");
        let action = args["action"].as_str().unwrap_or("push");
        let message = args["message"].as_str();
        let include_untracked = args["include_untracked"].as_bool().unwrap_or(true);

        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("stash");

        match action {
            "push" | "save" => {
                cmd.arg("push");
                if include_untracked {
                    cmd.arg("--include-untracked");
                }
                if let Some(msg) = message {
                    cmd.arg("-m").arg(msg);
                }
            }
            "pop" => {
                cmd.arg("pop");
            }
            "apply" => {
                cmd.arg("apply");
            }
            "list" => {
                cmd.arg("list");
            }
            "drop" => {
                cmd.arg("drop");
            }
            "clear" => {
                cmd.arg("clear");
            }
            _ => {
                return format!(
                    "Unknown stash action: {}. Use: push, pop, apply, list, drop, clear",
                    action
                );
            }
        }

        cmd.current_dir(work_dir);

        let output = tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output()).await;

        match output {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !out.status.success() {
                    let combined = format!("{}\n{}", stdout, stderr).trim().to_string();
                    format!("git stash {} failed: {}", action, combined)
                } else {
                    let result = stdout.trim();
                    if result.is_empty() {
                        format!("git stash {} completed", action)
                    } else {
                        result.to_string()
                    }
                }
            }
            Ok(Err(e)) => format!("Failed to execute git stash: {}", e),
            Err(_) => "git stash timed out (15s)".to_string(),
        }
    }
}
