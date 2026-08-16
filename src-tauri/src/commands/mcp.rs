//! MCP (Model Context Protocol) management commands.
//!
//! Provides Tauri commands for listing, connecting, and disconnecting
//! MCP servers. Servers are persisted in `mcp_servers.json` in the
//! app config directory.

use std::sync::Arc;
use tauri::{Manager, State};

use crate::agent::ToolDefinition;
use crate::mcp::McpServerConfig;
use crate::mcp::client::McpRegistry;

/// Status info for a single MCP server.
#[derive(serde::Serialize, Clone)]
pub struct McpServerStatus {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub enabled: bool,
    pub connected: bool,
    pub tool_count: usize,
}

/// List all configured MCP servers (from mcp_servers.json) with connection status.
#[tauri::command]
pub async fn list_mcp_servers(
    app: tauri::AppHandle,
    mcp_registry: State<'_, Arc<McpRegistry>>,
) -> Result<Vec<McpServerStatus>, String> {
    let config_path = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let servers_path = config_path.join("mcp_servers.json");

    let configs: Vec<McpServerConfig> = if servers_path.exists() {
        let content = std::fs::read_to_string(&servers_path)
            .map_err(|e| format!("Failed to read mcp_servers.json: {}", e))?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        vec![]
    };

    let connected = mcp_registry.list_connected_servers().await;

    let mut statuses = Vec::new();
    for cfg in configs {
        let is_connected = connected.contains(&cfg.name);
        let tool_count = if is_connected {
            mcp_registry.tool_count_for_server(&cfg.name).await
        } else {
            0
        };

        statuses.push(McpServerStatus {
            name: cfg.name,
            command: cfg.command,
            args: cfg.args,
            enabled: cfg.enabled,
            connected: is_connected,
            tool_count,
        });
    }
    Ok(statuses)
}

/// Connect to an MCP server: save config, spawn process, discover tools.
#[tauri::command]
pub async fn connect_mcp_server(
    app: tauri::AppHandle,
    mcp_registry: State<'_, Arc<McpRegistry>>,
    mcp_tools_state: State<'_, Arc<std::sync::Mutex<Vec<ToolDefinition>>>>,
    config: McpServerConfig,
) -> Result<usize, String> {
    let server_name = config.name.clone();

    // Connect via registry
    let tool_count = mcp_registry.connect(config.clone()).await?;

    // Save to mcp_servers.json
    let config_path = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let servers_path = config_path.join("mcp_servers.json");

    let mut configs: Vec<McpServerConfig> = if servers_path.exists() {
        let content = std::fs::read_to_string(&servers_path).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        vec![]
    };

    // Update or insert config
    if let Some(existing) = configs.iter_mut().find(|c| c.name == server_name) {
        *existing = config;
    } else {
        configs.push(config);
    }

    let json = serde_json::to_string_pretty(&configs)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;
    std::fs::write(&servers_path, json)
        .map_err(|e| format!("Failed to write mcp_servers.json: {}", e))?;

    // Update MCP tool definitions for future agent instances
    let schemas = mcp_registry.get_tool_schemas().await;
    let tools: Vec<ToolDefinition> = schemas
        .iter()
        .filter_map(|schema| {
            let func = schema.get("function")?;
            Some(ToolDefinition {
                name: func.get("name")?.as_str()?.to_string(),
                description: func.get("description")?.as_str()?.to_string(),
                parameters: func.get("parameters")?.clone(),
            })
        })
        .collect();

    log::info!(
        "[MCP] Connected to '{}', {} tools available",
        server_name,
        tool_count
    );

    if let Ok(mut guard) = mcp_tools_state.lock() {
        *guard = tools;
    }

    Ok(tool_count)
}

/// Disconnect from an MCP server and remove all its tools.
#[tauri::command]
pub async fn disconnect_mcp_server(
    app: tauri::AppHandle,
    mcp_registry: State<'_, Arc<McpRegistry>>,
    mcp_tools_state: State<'_, Arc<std::sync::Mutex<Vec<ToolDefinition>>>>,
    server_name: String,
) -> Result<usize, String> {
    let removed = mcp_registry.disconnect(&server_name).await?;

    // Update config file
    let config_path = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let servers_path = config_path.join("mcp_servers.json");

    if servers_path.exists() {
        let content = std::fs::read_to_string(&servers_path).unwrap_or_default();
        let configs: Vec<McpServerConfig> = serde_json::from_str(&content).unwrap_or_default();
        let updated: Vec<_> = configs
            .into_iter()
            .filter(|c| c.name != server_name)
            .collect();
        let json = serde_json::to_string_pretty(&updated)
            .map_err(|e| format!("Failed to serialize: {}", e))?;
        std::fs::write(&servers_path, json).map_err(|e| format!("Failed to write: {}", e))?;
    }

    // Update MCP tool definitions
    let schemas = mcp_registry.get_tool_schemas().await;
    let tools: Vec<ToolDefinition> = schemas
        .iter()
        .filter_map(|schema| {
            let func = schema.get("function")?;
            Some(ToolDefinition {
                name: func.get("name")?.as_str()?.to_string(),
                description: func.get("description")?.as_str()?.to_string(),
                parameters: func.get("parameters")?.clone(),
            })
        })
        .collect();

    if let Ok(mut guard) = mcp_tools_state.lock() {
        *guard = tools;
    }

    Ok(removed)
}
