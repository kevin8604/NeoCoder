use tauri::{AppHandle, Manager};

use crate::agent::definition::AgentDefinition;

/// Save a custom agent definition to the user's config directory.
/// If an agent with the same id already exists, it will be updated.
#[tauri::command]
pub async fn save_agent(app: AppHandle, agent: AgentDefinition) -> Result<String, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to get config dir: {}", e))?;
    let custom_path = config_dir.join("custom_agents.json");

    // Load existing custom agents
    let mut agents: Vec<AgentDefinition> = if custom_path.exists() {
        match std::fs::read_to_string(&custom_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    // Update or insert
    if let Some(existing) = agents.iter_mut().find(|a| a.id == agent.id) {
        *existing = agent.clone();
    } else {
        agents.push(agent.clone());
    }

    // Save
    if let Some(parent) = custom_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config dir: {}", e))?;
    }
    let json = serde_json::to_string_pretty(&agents)
        .map_err(|e| format!("Failed to serialize agents: {}", e))?;
    std::fs::write(&custom_path, &json)
        .map_err(|e| format!("Failed to write agents file: {}", e))?;

    log::info!(
        "[Agent] Saved custom agent '{}' (total: {})",
        agent.id,
        agents.len()
    );
    Ok(agent.id)
}

/// Delete a custom agent by id.
#[tauri::command]
pub async fn delete_agent(app: AppHandle, agent_id: String) -> Result<(), String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to get config dir: {}", e))?;
    let custom_path = config_dir.join("custom_agents.json");

    if !custom_path.exists() {
        return Err(format!(
            "Agent '{}' not found (no custom agents file)",
            agent_id
        ));
    }

    let content = std::fs::read_to_string(&custom_path)
        .map_err(|e| format!("Failed to read agents file: {}", e))?;
    let mut agents: Vec<AgentDefinition> = serde_json::from_str(&content).unwrap_or_default();

    let before_len = agents.len();
    agents.retain(|a| a.id != agent_id);

    if agents.len() == before_len {
        return Err(format!("Agent '{}' not found in custom agents", agent_id));
    }

    let json = serde_json::to_string_pretty(&agents)
        .map_err(|e| format!("Failed to serialize agents: {}", e))?;
    std::fs::write(&custom_path, &json)
        .map_err(|e| format!("Failed to write agents file: {}", e))?;

    log::info!(
        "[Agent] Deleted custom agent '{}' (remaining: {})",
        agent_id,
        agents.len()
    );
    Ok(())
}

/// Get all agent definitions (built-in + custom).
#[tauri::command]
pub async fn get_all_agents(app: AppHandle) -> Result<Vec<AgentDefinition>, String> {
    // Load custom agents
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to get config dir: {}", e))?;
    let custom_path = config_dir.join("custom_agents.json");

    let custom_agents: Vec<AgentDefinition> = if custom_path.exists() {
        match std::fs::read_to_string(&custom_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    // Load built-in agents (from agents.json or defaults)
    let mut agents = crate::agent::definition::load_agents_from_disk_except_custom();

    // Merge custom agents (custom overrides built-in by id)
    for custom in custom_agents {
        if let Some(existing) = agents.iter_mut().find(|a| a.id == custom.id) {
            *existing = custom;
        } else {
            agents.push(custom);
        }
    }

    Ok(agents)
}

/// List available tool names for agent creation UI.
#[tauri::command]
pub async fn list_available_tools() -> Result<Vec<ToolInfo>, String> {
    Ok(vec![
        ToolInfo {
            name: "read_file".into(),
            description: "Read file contents".into(),
            category: "file".into(),
        },
        ToolInfo {
            name: "write_file".into(),
            description: "Create or overwrite a file".into(),
            category: "file".into(),
        },
        ToolInfo {
            name: "append_file".into(),
            description: "Append content to a file".into(),
            category: "file".into(),
        },
        ToolInfo {
            name: "delete_file".into(),
            description: "Delete a file".into(),
            category: "file".into(),
        },
        ToolInfo {
            name: "edit".into(),
            description: "Precise string replacement in a file".into(),
            category: "file".into(),
        },
        ToolInfo {
            name: "list_directory".into(),
            description: "List directory contents".into(),
            category: "file".into(),
        },
        ToolInfo {
            name: "create_directory".into(),
            description: "Create a directory".into(),
            category: "file".into(),
        },
        ToolInfo {
            name: "delete_directory".into(),
            description: "Delete a directory (recursive)".into(),
            category: "file".into(),
        },
        ToolInfo {
            name: "glob".into(),
            description: "Find files by glob pattern".into(),
            category: "search".into(),
        },
        ToolInfo {
            name: "grep".into(),
            description: "Search text patterns in files".into(),
            category: "search".into(),
        },
        ToolInfo {
            name: "search_codebase".into(),
            description: "Semantic code search (RAG)".into(),
            category: "search".into(),
        },
        ToolInfo {
            name: "get_symbols".into(),
            description: "Get symbol definitions in a file".into(),
            category: "lsp".into(),
        },
        ToolInfo {
            name: "get_diagnostics".into(),
            description: "Get compiler/linter diagnostics".into(),
            category: "lsp".into(),
        },
        ToolInfo {
            name: "run_terminal_command".into(),
            description: "Execute a shell command".into(),
            category: "system".into(),
        },
        ToolInfo {
            name: "todo_write".into(),
            description: "Create/update task list".into(),
            category: "meta".into(),
        },
        ToolInfo {
            name: "web_search".into(),
            description: "Search the web (DuckDuckGo)".into(),
            category: "web".into(),
        },
        ToolInfo {
            name: "web_fetch".into(),
            description: "Fetch and parse a web page".into(),
            category: "web".into(),
        },
        ToolInfo {
            name: "ask_user_question".into(),
            description: "Ask the user a question".into(),
            category: "meta".into(),
        },
        ToolInfo {
            name: "dispatch_agent".into(),
            description: "Dispatch a single sub-agent".into(),
            category: "meta".into(),
        },
        ToolInfo {
            name: "dispatch_agents".into(),
            description: "Dispatch multiple sub-agents".into(),
            category: "meta".into(),
        },
        ToolInfo {
            name: "memory_search".into(),
            description: "Search session/note memory".into(),
            category: "memory".into(),
        },
        ToolInfo {
            name: "git_status".into(),
            description: "Show git working tree status".into(),
            category: "git".into(),
        },
        ToolInfo {
            name: "git_diff".into(),
            description: "Show git changes (diff)".into(),
            category: "git".into(),
        },
        ToolInfo {
            name: "git_commit".into(),
            description: "Create a git commit".into(),
            category: "git".into(),
        },
        ToolInfo {
            name: "git_log".into(),
            description: "Show git commit history".into(),
            category: "git".into(),
        },
        ToolInfo {
            name: "git_blame".into(),
            description: "Show line-by-line git blame".into(),
            category: "git".into(),
        },
        ToolInfo {
            name: "git_branch".into(),
            description: "List/create/delete branches".into(),
            category: "git".into(),
        },
        ToolInfo {
            name: "generate_diagram".into(),
            description: "Generate an architecture diagram".into(),
            category: "meta".into(),
        },
    ])
}

/// Tool info for the frontend
#[derive(serde::Serialize, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub category: String,
}
