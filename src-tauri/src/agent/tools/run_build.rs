//! run_build: read-only build verification tool for the Agent.
//!
//! Detects the project type and runs the matching build command, returning
//! exit code + truncated output + compiler error locations. Complements
//! run_tests in the "edit → verify → fix" loop.

use super::{Tool, ToolContext};
use crate::terminal::{parse_error_locations, push_terminal_entry, run_one_shot};
use std::path::Path;

pub struct RunBuild;

/// Detect a build command for the given project directory.
fn detect_build_command(dir: &str) -> Option<String> {
    let p = Path::new(dir);
    if p.join("Cargo.toml").exists() {
        return Some("cargo build".to_string());
    }
    if p.join("package.json").exists() {
        return Some("npm run build".to_string());
    }
    if p.join("go.mod").exists() {
        return Some("go build ./...".to_string());
    }
    if p.join("pyproject.toml").exists() {
        return Some("python -m build".to_string());
    }
    if p.join("Makefile").exists() {
        return Some("make".to_string());
    }
    None
}

#[async_trait::async_trait]
impl Tool for RunBuild {
    fn name(&self) -> &str {
        "run_build"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let work_dir = args["directory"]
            .as_str()
            .filter(|d| !d.is_empty())
            .or(ctx.project_path.as_deref())
            .unwrap_or(".");

        if !Path::new(work_dir).is_dir() {
            return format!("Error: directory '{}' does not exist", work_dir);
        }

        let command = args["command"]
            .as_str()
            .filter(|c| !c.is_empty())
            .map(|c| c.to_string())
            .or_else(|| detect_build_command(work_dir));

        let Some(command) = command else {
            return format!(
                "Error: no build system detected in '{}' (no Cargo.toml / package.json / \
                 go.mod / pyproject.toml). Specify a `command` explicitly (e.g. \"cargo build --release\").",
                work_dir
            );
        };

        let output = run_one_shot(&command, work_dir, 300).await;

        match output {
            Ok(out) => {
                let stdout_str = &out.stdout;
                let stderr_str = &out.stderr;
                let exit_code = out.exit_code;

                let mut result = String::new();
                result.push_str(&format!("$ {}\nExit code: {}\n", command, exit_code));
                if exit_code == 0 {
                    result.push_str("Build succeeded.\n");
                }

                const MAX_OUTPUT: usize = 80 * 1024;
                if !stdout_str.is_empty() {
                    result.push_str("\n--- STDOUT ---\n");
                    let s = if stdout_str.len() > MAX_OUTPUT {
                        &stdout_str[..MAX_OUTPUT]
                    } else {
                        stdout_str
                    };
                    result.push_str(s);
                }
                if !stderr_str.is_empty() {
                    result.push_str("\n--- STDERR ---\n");
                    let s = if stderr_str.len() > MAX_OUTPUT {
                        &stderr_str[..MAX_OUTPUT]
                    } else {
                        stderr_str
                    };
                    result.push_str(s);
                }

                // Compiler error locations (file:line) for targeted fixes
                let errors = parse_error_locations(stderr_str, stdout_str);
                if !errors.is_empty() {
                    result.push_str(&errors);
                }

                push_terminal_entry(&command, &result, exit_code);
                result
            }
            Err(e) => e,
        }
    }
}
