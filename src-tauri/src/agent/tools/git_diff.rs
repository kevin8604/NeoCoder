use super::{Tool, ToolContext};

/// Git diff tool: shows unstaged changes for a specific file or all changes.
pub struct GitDiff;

#[async_trait::async_trait]
impl Tool for GitDiff {
    fn name(&self) -> &str {
        "git_diff"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let work_dir = ctx.project_path.as_deref().unwrap_or(".");
        let file_path = args["file_path"].as_str();
        let staged = args["staged"].as_bool().unwrap_or(false);

        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("diff");

        if staged {
            cmd.arg("--cached");
        }

        if let Some(fp) = file_path {
            if !fp.is_empty() {
                cmd.arg("--");
                cmd.arg(fp);
            }
        } else {
            // No file specified — show stat summary
            cmd.arg("--stat");
        }

        cmd.current_dir(work_dir);

        let output = tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output()).await;

        match output {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !out.status.success() {
                    return format!("git diff failed: {}", stderr.trim());
                }
                if stdout.is_empty() {
                    if let Some(fp) = file_path {
                        format!("No changes detected for: {}", fp)
                    } else {
                        "No unstaged changes detected.".to_string()
                    }
                } else {
                    // Truncate very long diffs
                    if stdout.len() > 8000 {
                        let head = crate::agent::utils::safe_truncate(&stdout, 6000);
                        format!(
                            "{}\n\n... [TRUNCATED: {} chars omitted] ...",
                            head,
                            stdout.len() - 6000
                        )
                    } else {
                        stdout.to_string()
                    }
                }
            }
            Ok(Err(e)) => format!("Failed to execute git diff: {}", e),
            Err(_) => "git diff timed out (15s)".to_string(),
        }
    }
}
