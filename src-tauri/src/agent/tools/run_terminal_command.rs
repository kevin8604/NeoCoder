use super::{Tool, ToolContext};
use crate::terminal::{parse_error_locations, push_terminal_entry, run_one_shot};

pub struct RunTerminalCommand;

#[async_trait::async_trait]
impl Tool for RunTerminalCommand {
    fn name(&self) -> &str {
        "run_terminal_command"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let cmd = args["command"].as_str().unwrap_or("");
        if cmd.is_empty() {
            return "Error: command is required".to_string();
        }

        // 安全检查: sandbox command check (includes built-in + user-configured blocked commands)
        if let Err(reason) = ctx.sandbox.check_command(cmd) {
            log::warn!("Blocked dangerous command '{}': {}", cmd, reason);
            return format!("Error: Command blocked for safety: {}. If you believe this is a false positive, ask the user to run it manually.", reason);
        }

        let work_dir = ctx.project_path.as_deref().unwrap_or(".");

        let output = run_one_shot(cmd, work_dir, 30).await;

        match output {
            Ok(out) => {
                let stdout_str = &out.stdout;
                let stderr_str = &out.stderr;
                let exit_code = out.exit_code;

                let mut result = String::new();
                let max_output = 100 * 1024;
                if !stdout_str.is_empty() {
                    result.push_str("STDOUT:\n");
                    if stdout_str.len() > max_output {
                        result.push_str(&stdout_str[..max_output]);
                        result.push_str("\n... (output truncated at 100KB)");
                    } else {
                        result.push_str(stdout_str);
                    }
                }
                if !stderr_str.is_empty() {
                    if !result.is_empty() {
                        result.push_str("\n");
                    }
                    result.push_str("STDERR:\n");
                    if stderr_str.len() > max_output {
                        result.push_str(&stderr_str[..max_output]);
                        result.push_str("\n... (output truncated at 100KB)");
                    } else {
                        result.push_str(stderr_str);
                    }
                }
                result.push_str(&format!("\n\nExit code: {}", exit_code));

                // Append error summary if exit code != 0 or errors detected
                let error_summary = parse_error_locations(stderr_str, stdout_str);
                if !error_summary.is_empty() {
                    result.push_str(&error_summary);
                }

                if result.len() > max_output * 2 {
                    result = result[..max_output * 2].to_string();
                    result.push_str("\n... (total output truncated)");
                }

                // Push to terminal history for @error/@terminal feature
                push_terminal_entry(cmd, &result, exit_code);

                result
            }
            Err(e) => e,
        }
    }
}
