use super::{Tool, ToolContext};
use std::collections::VecDeque;
use std::sync::Mutex;

pub struct RunTerminalCommand;

// ── Terminal output history (global cache, max 10 entries) ──
struct TerminalEntry {
    command: String,
    output: String,
    exit_code: i32,
}

static TERMINAL_HISTORY: std::sync::LazyLock<Mutex<VecDeque<TerminalEntry>>> =
    std::sync::LazyLock::new(|| Mutex::new(VecDeque::new()));

pub fn push_terminal_entry(command: &str, output: &str, exit_code: i32) {
    if let Ok(mut history) = TERMINAL_HISTORY.lock() {
        if history.len() >= 10 {
            history.pop_front();
        }
        history.push_back(TerminalEntry {
            command: command.to_string(),
            output: output.to_string(),
            exit_code,
        });
    }
}

pub fn get_recent_terminal(n: usize) -> Vec<(String, String, i32)> {
    let history = TERMINAL_HISTORY.lock().ok();
    match history {
        Some(h) => h.iter().rev().take(n).map(|e| {
            (e.command.clone(), e.output.clone(), e.exit_code)
        }).collect(),
        None => Vec::new(),
    }
}

pub fn get_error_summary() -> String {
    let history = TERMINAL_HISTORY.lock().ok();
    match history {
        Some(h) => {
            let mut summary = String::new();
            for entry in h.iter().rev().take(5) {
                if entry.exit_code != 0 || entry.output.to_lowercase().contains("error") || entry.output.to_lowercase().contains("fail") {
                    summary.push_str(&format!("$ {}\nExit: {}\n{}\n\n", entry.command, entry.exit_code,
                        if entry.output.len() > 2000 { crate::agent::utils::safe_truncate(&entry.output, 2000) } else { &entry.output }));
                }
            }
            if summary.is_empty() { "No recent errors found.".to_string() } else { summary }
        }
        None => "Terminal history unavailable.".to_string(),
    }
}

/// Parse common compiler/linter error patterns and extract file:line references
fn parse_error_locations(stderr: &str, stdout: &str) -> String {
    let combined = format!("{}\n{}", stdout, stderr);
    let mut errors: Vec<String> = Vec::new();

    for line in combined.lines() {
        let lower = line.to_lowercase();

        // Rust: error[E0308]: src/main.rs:42:5 or error: src/main.rs:42
        if lower.contains("error[") || (lower.starts_with("error") && lower.contains(".rs:")) {
            errors.push(format!("  {}", line.trim()));
            continue;
        }
        // TypeScript: src/foo.ts(12,5): error TS2322
        if lower.contains(".ts(") && lower.contains("error ts") {
            errors.push(format!("  {}", line.trim()));
            continue;
        }
        // JavaScript: src/foo.js:12:5
        if (lower.contains(".js:") || lower.contains(".jsx:")) && (lower.contains("error") || lower.contains("syntaxerror")) {
            errors.push(format!("  {}", line.trim()));
            continue;
        }
        // Python: File "foo.py", line 23
        if lower.contains("file \"") && lower.contains("line ") {
            errors.push(format!("  {}", line.trim()));
            continue;
        }
        // Go: ./main.go:15:2:
        if lower.contains(".go:") && (lower.contains("undefined") || lower.contains("cannot") || lower.contains("syntax")) {
            errors.push(format!("  {}", line.trim()));
            continue;
        }
        // Generic: error in file.ext:LINE
        if lower.contains("error") && (line.contains(": ") || line.contains(" at ")) {
            // Only include if it looks like it has a file path
            if line.contains('/') || line.contains('\\') || line.contains('.') {
                errors.push(format!("  {}", line.trim()));
            }
        }
    }

    if errors.is_empty() {
        return String::new();
    }

    // Deduplicate
    errors.sort();
    errors.dedup();
    let count = errors.len();
    let max_show = if errors.len() > 10 { 10 } else { errors.len() };

    let mut result = format!("\n--- Error Summary ({} found) ---\n", count);
    for e in errors.iter().take(max_show) {
        result.push_str(e);
        result.push('\n');
    }
    if count > max_show {
        result.push_str(&format!("  ... and {} more\n", count - max_show));
    }
    result
}

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

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            tokio::process::Command::new(if cfg!(target_os = "windows") { "cmd" } else { "sh" })
                .arg(if cfg!(target_os = "windows") { "/C" } else { "-c" })
                .arg(cmd)
                .current_dir(work_dir)
                .output(),
        )
        .await;

        match output {
            Ok(Ok(out)) => {
                let stdout_str = String::from_utf8_lossy(&out.stdout);
                let stderr_str = String::from_utf8_lossy(&out.stderr);
                let exit_code = out.status.code().unwrap_or(-1);

                let mut result = String::new();
                let max_output = 100 * 1024;
                if !out.stdout.is_empty() {
                    result.push_str("STDOUT:\n");
                    if stdout_str.len() > max_output {
                        result.push_str(&stdout_str[..max_output]);
                        result.push_str("\n... (output truncated at 100KB)");
                    } else {
                        result.push_str(&stdout_str);
                    }
                }
                if !out.stderr.is_empty() {
                    if !result.is_empty() {
                        result.push_str("\n");
                    }
                    result.push_str("STDERR:\n");
                    if stderr_str.len() > max_output {
                        result.push_str(&stderr_str[..max_output]);
                        result.push_str("\n... (output truncated at 100KB)");
                    } else {
                        result.push_str(&stderr_str);
                    }
                }
                result.push_str(&format!("\n\nExit code: {}", exit_code));

                // Append error summary if exit code != 0 or errors detected
                let error_summary = parse_error_locations(&stderr_str, &stdout_str);
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
            Ok(Err(e)) => format!("Failed to execute command '{}': {}", cmd, e),
            Err(_) => format!("Command '{}' timed out after 30 seconds", cmd),
        }
    }
}
