use super::{Tool, ToolContext};
use std::process::Command;

pub struct GitBranch;

#[async_trait::async_trait]
impl Tool for GitBranch {
    fn name(&self) -> &str {
        "git_branch"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let all = args["all"].as_bool().unwrap_or(false);
        let project_path = ctx.project_path.as_deref().unwrap_or(".");

        let mut cmd = Command::new("git");
        cmd.arg("branch");
        if all {
            cmd.arg("-a");
        }
        cmd.current_dir(project_path);
        let output = cmd.output();

        match output {
            Ok(o) if o.status.success() => {
                let branches = String::from_utf8_lossy(&o.stdout);
                if branches.trim().is_empty() {
                    "No branches found.".to_string()
                } else {
                    format!("Branches:\n{}", branches)
                }
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                format!("Error: git branch failed: {}", err.trim())
            }
            Err(e) => format!("Error executing git branch: {}", e),
        }
    }
}
