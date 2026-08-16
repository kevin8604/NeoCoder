use std::time::Duration;

use super::{Tool, ToolContext};
use crate::a2a::client::A2aClient;
use tauri::Manager;

/// A2A invocation tool — lets the agent call remote A2A agents.
///
/// Parameters:
/// - `url`: base URL (e.g. `http://127.0.0.1:41234`) or `/a2a` endpoint
/// - `agent`: (alternative to `url`) name of a remote agent configured in
///   Settings → A2A → Remote Agents; the URL is resolved from settings
/// - `task`: the task text to send
/// - `mode`: "sync" (default) | "poll" | "stream"
/// - `skill` (optional): skill/agent id to request on the remote side
///   (sent as `metadata.skillId`; see the remote Agent Card's skills)
/// - `timeout_secs`: max wait time, default 120
/// - `token` (optional): Bearer token override (falls back to local a2a_server_token)
pub struct A2aInvoke;

/// Resolve a configured remote agent's URL by name (pure, testable).
pub fn resolve_agent_url(
    agent_name: &str,
    agents: &[crate::a2a::A2aAgentConfig],
) -> Result<String, String> {
    match agents.iter().find(|a| a.name == agent_name) {
        Some(a) => Ok(a.url.clone()),
        None => {
            let names = agents
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "unknown remote agent '{}' (configured: {})",
                agent_name,
                if names.is_empty() {
                    "none".to_string()
                } else {
                    names
                }
            ))
        }
    }
}

#[async_trait::async_trait]
impl Tool for A2aInvoke {
    fn name(&self) -> &str {
        "a2a_invoke"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let url = args["url"].as_str().unwrap_or("");
        let agent_name = args["agent"].as_str().unwrap_or("");
        if url.is_empty() && agent_name.is_empty() {
            return "Error: url or agent parameter is required (A2A base URL, /a2a endpoint, or configured agent name)"
                .to_string();
        }
        let task = args["task"].as_str().unwrap_or("");
        if task.is_empty() {
            return "Error: task parameter is required".to_string();
        }
        let mode = args["mode"].as_str().unwrap_or("sync");
        let timeout_secs = args["timeout_secs"].as_u64().unwrap_or(120);
        let skill = args["skill"]
            .as_str()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());

        // Resolve the target URL: explicit `url` wins, otherwise look up the
        // configured remote agent by name.
        let resolved_url = if !url.is_empty() {
            url.to_string()
        } else {
            let agents: Vec<crate::a2a::A2aAgentConfig> = ctx
                .app_handle
                .as_ref()
                .and_then(|app| {
                    app.try_state::<std::sync::Arc<tokio::sync::RwLock<crate::config::AppSettings>>>()
                        .map(|s| {
                            let guard = tokio::task::block_in_place(|| s.blocking_read());
                            guard.a2a_agents.clone()
                        })
                })
                .unwrap_or_default();
            match resolve_agent_url(agent_name, &agents) {
                Ok(u) => u,
                Err(msg) => {
                    return format!(
                        "Error: {} — configure it in Settings → A2A → Remote Agents first",
                        msg
                    );
                }
            }
        };

        // Token: explicit parameter wins, otherwise reuse local A2A server token
        let token = args["token"]
            .as_str()
            .map(|t| t.to_string())
            .filter(|t| !t.is_empty())
            .or_else(|| {
                ctx.app_handle.as_ref().and_then(|app| {
                    app.try_state::<std::sync::Arc<tokio::sync::RwLock<crate::config::AppSettings>>>()
                        .map(|s| {
                            let guard = tokio::task::block_in_place(|| s.blocking_read());
                            guard.a2a_server_token.clone()
                        })
                        .filter(|t| !t.is_empty())
                })
            });

        let client = A2aClient::new(
            resolved_url,
            token,
            Duration::from_millis(500),
            Duration::from_secs(timeout_secs.max(1)),
        );

        match client.invoke(task, mode, skill.as_deref()).await {
            Ok(summary) => summary,
            Err(e) => format!("Error: A2A invocation failed: {}", e),
        }
    }
}
