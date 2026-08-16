use super::{Tool, ToolContext};

/// Git push tool: pushes commits to the remote repository.
pub struct GitPush;

#[async_trait::async_trait]
impl Tool for GitPush {
    fn name(&self) -> &str {
        "git_push"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let work_dir = ctx.project_path.as_deref().unwrap_or(".");
        let remote = args["remote"].as_str().unwrap_or("origin");
        let branch = args["branch"].as_str();
        let force = args["force"].as_bool().unwrap_or(false);
        let set_upstream = args["set_upstream"].as_bool().unwrap_or(false);

        // Safety check: refuse force push to main/master
        if force {
            let current_branch = get_current_branch(work_dir);
            if matches!(current_branch.as_deref(), Some("main") | Some("master")) {
                return "Error: Force push to main/master branch is blocked for safety".to_string();
            }
        }

        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("push");
        cmd.arg(remote);

        if let Some(branch) = branch {
            cmd.arg(branch);
        }

        if force {
            cmd.arg("--force");
        }

        if set_upstream {
            cmd.arg("--set-upstream");
        }

        cmd.current_dir(work_dir);

        let output = tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output()).await;

        match output {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !out.status.success() {
                    let combined = format!("{}\n{}", stdout, stderr).trim().to_string();
                    format!("git push failed: {}", combined)
                } else {
                    let info = if stderr.trim().is_empty() {
                        stdout.trim().to_string()
                    } else {
                        stderr.trim().to_string() // git push writes progress to stderr
                    };
                    format!(
                        "Pushed successfully to {}/{}{}",
                        remote,
                        branch.unwrap_or("current branch"),
                        if info.is_empty() {
                            String::new()
                        } else {
                            format!("\n{}", info)
                        }
                    )
                }
            }
            Ok(Err(e)) => format!("Failed to execute git push: {}", e),
            Err(_) => "git push timed out (30s)".to_string(),
        }
    }
}

/// Get the current branch name
fn get_current_branch(work_dir: &str) -> Option<String> {
    std::process::Command::new("git")
        .arg("rev-parse")
        .arg("--abbrev-ref")
        .arg("HEAD")
        .current_dir(work_dir)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
}
