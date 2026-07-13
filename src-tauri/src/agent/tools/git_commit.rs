use super::{Tool, ToolContext};

/// Git commit tool: stages changes and creates a commit.
pub struct GitCommit;

#[async_trait::async_trait]
impl Tool for GitCommit {
    fn name(&self) -> &str {
        "git_commit"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let message = args["message"].as_str().unwrap_or("");
        if message.is_empty() {
            return "Error: commit message is required".to_string();
        }

        let work_dir = ctx.project_path.as_deref().unwrap_or(".");
        let add_all = args["add_all"].as_bool().unwrap_or(true);

        // Step 1: Stage files if add_all is true
        if add_all {
            let add_output = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                tokio::process::Command::new("git")
                    .arg("add")
                    .arg("-A")
                    .current_dir(work_dir)
                    .output(),
            )
            .await;

            match add_output {
                Ok(Ok(out)) => {
                    if !out.status.success() {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        return format!("git add -A failed: {}", stderr.trim());
                    }
                }
                Ok(Err(e)) => return format!("Failed to execute git add: {}", e),
                Err(_) => return "git add timed out (15s)".to_string(),
            }
        }

        // Step 2: Commit
        let commit_output = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            tokio::process::Command::new("git")
                .arg("commit")
                .arg("-m")
                .arg(message)
                .current_dir(work_dir)
                .output(),
        )
        .await;

        match commit_output {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !out.status.success() {
                    let combined = format!("{}\n{}", stdout, stderr).trim().to_string();
                    return format!("git commit failed: {}", combined);
                }
                // Extract commit hash from output
                let hash = extract_commit_hash(&stdout);
                match hash {
                    Some(h) => format!("Committed successfully: {} ({})", message, h),
                    None => format!("Committed: {}", stdout.trim()),
                }
            }
            Ok(Err(e)) => format!("Failed to execute git commit: {}", e),
            Err(_) => "git commit timed out (15s)".to_string(),
        }
    }
}

/// Extract the short commit hash from git commit output.
/// Output typically contains: "[main abc1234] message"
fn extract_commit_hash(output: &str) -> Option<String> {
    for line in output.lines() {
        if line.starts_with('[') {
            // Format: [branch hash] message
            if let Some(close) = line.find(']') {
                let inner = &line[1..close];
                let parts: Vec<&str> = inner.split_whitespace().collect();
                if parts.len() >= 2 {
                    return Some(parts[1].to_string());
                }
            }
        }
    }
    None
}
