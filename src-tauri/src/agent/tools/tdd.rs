//! tdd tool: start/stop/inspect the TDD state machine for the current session.
//!
//! Phase transitions are driven by `TddGateHook` from run_tests outcomes, so
//! the agent only needs to call this to enter/exit TDD mode.

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolContext};

pub struct TddTool;

/// Detect the project's test command (mirrors run_tests).
fn detect_test_command(dir: &str) -> Option<String> {
    let p = std::path::Path::new(dir);
    if p.join("Cargo.toml").exists() {
        return Some("cargo test".to_string());
    }
    if p.join("package.json").exists() {
        return Some("npm test".to_string());
    }
    if p.join("pyproject.toml").exists()
        || p.join("pytest.ini").exists()
        || p.join("setup.py").exists()
    {
        return Some("python -m pytest -q".to_string());
    }
    if p.join("go.mod").exists() {
        return Some("go test ./...".to_string());
    }
    None
}

#[async_trait]
impl Tool for TddTool {
    fn name(&self) -> &str {
        "tdd"
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> String {
        let Some(session_id) = ctx.session_id.as_deref() else {
            return "[ERROR] tdd requires an active session (no session_id in context)."
                .to_string();
        };
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("start");

        match action {
            "start" => {
                // Allow restarting from scratch
                let test_command = args
                    .get("test_command")
                    .and_then(|v| v.as_str())
                    .filter(|c| !c.is_empty())
                    .map(|c| c.to_string())
                    .or_else(|| ctx.project_path.as_deref().and_then(detect_test_command));
                crate::agent::tdd::start(session_id, test_command)
            }
            "stop" => crate::agent::tdd::stop(session_id),
            "status" => crate::agent::tdd::status(session_id),
            other => format!(
                "[ERROR] Unknown tdd action '{}'. Valid actions: start, status, stop.",
                other
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_commands() {
        // Only sanity-check the Cargo branch with a temp dir
        let dir = std::env::temp_dir().join(format!("nee-tdd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        assert_eq!(
            detect_test_command(dir.to_str().unwrap()).as_deref(),
            Some("cargo test")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
