//! run_tests: read-only test execution tool for the Agent.
//!
//! Detects the project type (Cargo / npm / pytest / go) and runs the matching
//! test command. This closes the "edit → verify → fix" loop together with
//! AutoDiagnoseHook: after editing code, the agent can immediately run tests
//! without interactive confirmation (unlike run_terminal_command).

use super::{Tool, ToolContext};
use crate::terminal::{parse_error_locations, push_terminal_entry, run_one_shot};
use std::path::Path;

pub struct RunTests;

/// Detect a test command for the given project directory.
fn detect_test_command(dir: &str) -> Option<String> {
    let p = Path::new(dir);
    if p.join("Cargo.toml").exists() {
        return Some("cargo test".to_string());
    }
    if p.join("package.json").exists() {
        // Prefer the standard npm test script; fall back to jest/vitest directly
        return Some("npm test".to_string());
    }
    if p.join("pyproject.toml").exists() || p.join("pytest.ini").exists() || p.join("setup.py").exists() {
        return Some("python -m pytest -q".to_string());
    }
    if p.join("go.mod").exists() {
        return Some("go test ./...".to_string());
    }
    if p.join("Makefile").exists() {
        return Some("make test".to_string());
    }
    None
}

/// Summarize test output: extract "test result:" lines (Rust) or pass/fail totals.
fn summarize_test_output(stdout: &str) -> String {
    let mut summary = String::new();

    // Rust: "test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
    for line in stdout.lines() {
        let lower = line.to_lowercase();
        if lower.contains("test result:") {
            summary.push_str(line.trim());
            summary.push('\n');
        }
    }
    // pytest: "12 passed, 0 failed in 1.23s"
    for line in stdout.lines() {
        let lower = line.to_lowercase();
        if (lower.contains("passed") || lower.contains("failed")) && lower.contains(" in ") && lower.ends_with('s') {
            summary.push_str(line.trim());
            summary.push('\n');
        }
    }
    // npm/jest: "Tests: 12 passed, 12 total"
    for line in stdout.lines() {
        let lower = line.to_lowercase();
        if lower.starts_with("tests:") {
            summary.push_str(line.trim());
            summary.push('\n');
        }
    }
    summary
}

#[async_trait::async_trait]
impl Tool for RunTests {
    fn name(&self) -> &str {
        "run_tests"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let work_dir = args["directory"].as_str()
            .filter(|d| !d.is_empty())
            .or(ctx.project_path.as_deref())
            .unwrap_or(".");

        if !Path::new(work_dir).is_dir() {
            return format!("Error: directory '{}' does not exist", work_dir);
        }

        let command = args["command"].as_str()
            .filter(|c| !c.is_empty())
            .map(|c| c.to_string())
            .or_else(|| detect_test_command(work_dir));

        let Some(command) = command else {
            return format!(
                "Error: no test framework detected in '{}' (no Cargo.toml / package.json / \
                 pyproject.toml / go.mod). Specify a `command` explicitly (e.g. \"cargo test --lib\").",
                work_dir
            );
        };

        let output = run_one_shot(&command, work_dir, 180).await;

        match output {
            Ok(out) => {
                let stdout_str = &out.stdout;
                let stderr_str = &out.stderr;
                let exit_code = out.exit_code;

                let mut result = String::new();
                result.push_str(&format!("$ {}\nExit code: {}\n", command, exit_code));

                // Test summary line(s) extracted from stdout
                let summary = summarize_test_output(stdout_str);
                if !summary.is_empty() {
                    result.push_str("\n--- Test Summary ---\n");
                    result.push_str(&summary);
                }

                const MAX_OUTPUT: usize = 80 * 1024;
                if !stdout_str.is_empty() {
                    result.push_str("\n--- STDOUT (truncated to 80KB) ---\n");
                    let s = if stdout_str.len() > MAX_OUTPUT { &stdout_str[..MAX_OUTPUT] } else { stdout_str };
                    result.push_str(s);
                }
                if !stderr_str.is_empty() {
                    result.push_str("\n--- STDERR ---\n");
                    let s = if stderr_str.len() > MAX_OUTPUT { &stderr_str[..MAX_OUTPUT] } else { stderr_str };
                    result.push_str(s);
                }

                // Compiler-style error locations for the AutoDiagnoseHook loop
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
