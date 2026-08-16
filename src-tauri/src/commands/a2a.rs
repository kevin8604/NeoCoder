//! A2A commands — server status/config, remote agent discovery and invocation.
//!
//! Core logic is factored into pure functions (`a2a_status_from`,
//! `apply_a2a_config`) so the behavior is unit-testable without a Tauri app.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{Manager, State};
use tokio::sync::RwLock;

use crate::a2a::client::A2aClient;
use crate::a2a::{A2aAgentConfig, AgentCard};
use crate::commands::config::ConfigState;
use crate::config::AppSettings;

/// Runtime state for the A2A HTTP server (written by lib.rs setup).
#[derive(Default)]
pub struct A2aRuntimeState {
    pub running: std::sync::atomic::AtomicBool,
    pub port: std::sync::atomic::AtomicU16,
}

/// A2A status as shown in the frontend.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct A2aStatus {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub token_set: bool,
}

/// Derive the A2A status from settings + runtime flags (pure).
pub fn a2a_status_from(settings: &AppSettings, running: bool, port: u16) -> A2aStatus {
    A2aStatus {
        enabled: settings.a2a_server_enabled,
        running: settings.a2a_server_enabled && running,
        port: if settings.a2a_server_enabled { port } else { 0 },
        token_set: !settings.a2a_server_token.is_empty(),
    }
}

/// Parameters for `set_a2a_config` (all optional — only provided fields change).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SetA2aConfigParams {
    pub enabled: Option<bool>,
    pub port: Option<u16>,
    pub token: Option<String>,
    pub agents: Option<Vec<A2aAgentConfig>>,
}

/// Apply config params onto a settings copy (pure, testable).
pub fn apply_a2a_config(mut settings: AppSettings, params: &SetA2aConfigParams) -> AppSettings {
    if let Some(v) = params.enabled {
        settings.a2a_server_enabled = v;
    }
    if let Some(v) = params.port {
        settings.a2a_server_port = v;
    }
    if let Some(v) = &params.token {
        settings.a2a_server_token = v.clone();
    }
    if let Some(v) = &params.agents {
        settings.a2a_agents = v.clone();
    }
    settings
}

/// Current A2A server status.
#[tauri::command]
pub async fn get_a2a_status(
    app: tauri::AppHandle,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
) -> Result<A2aStatus, String> {
    let settings = settings.read().await;
    let (running, port) = app
        .try_state::<A2aRuntimeState>()
        .map(|s| {
            (
                s.running.load(std::sync::atomic::Ordering::SeqCst),
                s.port.load(std::sync::atomic::Ordering::SeqCst),
            )
        })
        .unwrap_or((false, 0));
    Ok(a2a_status_from(&settings, running, port))
}

/// Update A2A server config (persisted via ConfigState).
#[tauri::command]
pub async fn set_a2a_config(
    params: SetA2aConfigParams,
    config_state: State<'_, ConfigState>,
    settings_state: State<'_, Arc<RwLock<AppSettings>>>,
) -> Result<(), String> {
    let current = settings_state.read().await.clone();
    let updated = apply_a2a_config(current, &params);
    let manager = config_state.manager.write().await;
    manager.update_settings(updated).await
}

/// List configured remote agents.
#[tauri::command]
pub async fn list_remote_agents(
    settings: State<'_, Arc<RwLock<AppSettings>>>,
) -> Result<Vec<A2aAgentConfig>, String> {
    Ok(settings.read().await.a2a_agents.clone())
}

/// Discover a remote agent's Agent Card (returns full card for the frontend).
#[tauri::command]
pub async fn discover_remote_agent(
    url: String,
    token: Option<String>,
) -> Result<AgentCard, String> {
    let client = A2aClient::with_defaults(url, token.filter(|t| !t.is_empty()));
    client.discover().await.map_err(|e| e.to_string())
}

/// Invoke a remote agent manually (debug / manual trigger from the frontend).
#[tauri::command]
pub async fn invoke_remote_agent(
    url: String,
    task: String,
    mode: Option<String>,
    timeout_secs: Option<u64>,
    token: Option<String>,
    skill: Option<String>,
) -> Result<String, String> {
    let client = A2aClient::new(
        url,
        token.filter(|t| !t.is_empty()),
        Duration::from_millis(500),
        Duration::from_secs(timeout_secs.unwrap_or(120).max(1)),
    );
    client
        .invoke(
            &task,
            mode.as_deref().unwrap_or("sync"),
            skill.as_deref().filter(|s| !s.is_empty()),
        )
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_a2a_status_from_not_running() {
        let settings = AppSettings::default();
        let status = a2a_status_from(&settings, false, 0);
        assert_eq!(
            status,
            A2aStatus {
                enabled: false,
                running: false,
                port: 0,
                token_set: false,
            }
        );
    }

    #[test]
    fn test_a2a_status_from_enabled_but_not_started() {
        let settings = AppSettings {
            a2a_server_enabled: true,
            a2a_server_port: 41234,
            ..Default::default()
        };
        let status = a2a_status_from(&settings, false, 0);
        assert!(status.enabled);
        assert!(!status.running);
        assert_eq!(status.port, 0);
    }

    #[test]
    fn test_a2a_status_from_running() {
        let settings = AppSettings {
            a2a_server_enabled: true,
            a2a_server_port: 41234,
            a2a_server_token: "t".into(),
            ..Default::default()
        };
        let status = a2a_status_from(&settings, true, 41234);
        assert!(status.running);
        assert_eq!(status.port, 41234);
        assert!(status.token_set);
    }

    #[test]
    fn test_apply_a2a_config_and_persist() {
        // 1) 应用参数（纯函数）
        let base = AppSettings::default();
        let params = SetA2aConfigParams {
            enabled: Some(true),
            port: Some(43210),
            token: Some("abc".into()),
            agents: Some(vec![A2aAgentConfig {
                name: "remote1".into(),
                url: "http://127.0.0.1:9999".into(),
                description: "d".into(),
            }]),
        };
        let updated = apply_a2a_config(base.clone(), &params);
        assert!(updated.a2a_server_enabled);
        assert_eq!(updated.a2a_server_port, 43210);
        assert_eq!(updated.a2a_server_token, "abc");
        assert_eq!(updated.a2a_agents.len(), 1);

        // 2) 部分更新：只改 enabled，其他保持
        let partial = SetA2aConfigParams {
            enabled: Some(false),
            port: None,
            token: None,
            agents: None,
        };
        let updated2 = apply_a2a_config(updated, &partial);
        assert!(!updated2.a2a_server_enabled);
        assert_eq!(updated2.a2a_server_port, 43210);
        assert_eq!(updated2.a2a_agents.len(), 1);

        // 3) ConfigManager 持久化读回
        let dir = std::env::temp_dir().join("neocoder_a2a_cmd_test");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = crate::config::ConfigManager::new(dir.clone());
        let persist = async {
            manager.update_settings(updated2.clone()).await.unwrap();
            manager.get_settings().await
        };
        let read_back = tokio::runtime::Runtime::new().unwrap().block_on(persist);
        assert!(!read_back.a2a_server_enabled);
        assert_eq!(read_back.a2a_server_port, 43210);
        assert_eq!(read_back.a2a_server_token, "abc");
        assert_eq!(read_back.a2a_agents.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_app_settings_a2a_defaults() {
        let settings = AppSettings::default();
        assert!(!settings.a2a_server_enabled);
        assert_eq!(settings.a2a_server_port, 41234);
        assert_eq!(settings.a2a_server_token, "");
        assert!(settings.a2a_agents.is_empty());

        // 旧配置文件（无 a2a 字段）反序列化不报错
        let legacy = json!({
            "llm_provider": "deepseek",
            "completion_model": "deepseek-chat",
            "chat_model": "deepseek-chat",
            "embedding_model": "e",
            "completion_enabled": true,
            "trigger_debounce_ms": 300,
            "max_context_tokens": 8192,
            "max_prefix_lines": 80,
            "max_suffix_lines": 40,
            "ignore_patterns": [],
            "custom_instructions": "",
            "project_paths": [],
            "theme": "Dark"
        });
        let parsed: AppSettings = serde_json::from_value(legacy).unwrap();
        assert!(!parsed.a2a_server_enabled);
        assert_eq!(parsed.a2a_server_port, 41234);
        assert!(parsed.a2a_agents.is_empty());
    }
}
