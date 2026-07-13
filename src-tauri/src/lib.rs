pub mod agent;
pub mod config;
pub mod completion;
pub mod chat;
pub mod rag;
pub mod lsp;
pub mod fs_watcher;
pub mod llm;
pub mod memory;
pub mod commands;
pub mod logging;
pub mod skill;
pub mod sandbox;
pub mod mcp;
pub mod telemetry;

use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;
use tokio::sync::RwLock;

use commands::pty::PtyState;
use lsp::LspManager;
use rag::CodeIndexer;
use fs_watcher::FileWatcher;
use fs_watcher::FileChangeKind;
use agent::QuestionAwaiters;
use agent::ConfirmAwaiters;
use agent::ToolRegistry;
use agent::definition::AgentRegistry;
use mcp::client::McpRegistry;
use std::collections::HashMap;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 初始化日志系统（替换 env_logger）
    let app_data_dir = directories::ProjectDirs::from("com", "neecoder", "NeeCoder")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| {
            // Fallback: 使用当前目录下的 .neecoder
            std::env::current_dir().unwrap_or_default().join(".neecoder")
        });
    logging::init(&app_data_dir);

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            // Initialize config
            let config_path = app.path().app_config_dir().unwrap_or_default();
            let sessions_dir = config_path.join("sessions");
            let config_manager = config::ConfigManager::new(config_path.clone());
            let settings_handle = config_manager.settings_handle();

            // Manage state
            app.manage(commands::config::ConfigState {
                manager: Arc::new(RwLock::new(config_manager)),
            });
            app.manage::<Arc<RwLock<config::AppSettings>>>(settings_handle);

            app.manage(commands::chat::ChatState {
                memory: Arc::new(RwLock::new(chat::ConversationMemory::with_storage(sessions_dir))),
            });

            // Initialize completion cache (LRU, max 200 entries)
            app.manage::<Arc<std::sync::Mutex<commands::completion::CompletionCache>>>(Arc::new(
                std::sync::Mutex::new(commands::completion::CompletionCache::new(200)),
            ));

            // Initialize completion cancel map
            app.manage::<commands::completion::CancelMap>(Arc::new(std::sync::Mutex::new(HashMap::new())));

            // Initialize Agent cancel map
            app.manage::<commands::chat::AgentCancelMap>(Arc::new(std::sync::Mutex::new(HashMap::new())));

            // Initialize checkpoint store (keyed by session_id)
            app.manage::<agent::checkpoint::CheckpointStore>(agent::checkpoint::new_store());

            // Initialize FileSnapshotStore for file undo mechanism
            app.manage::<commands::chat::FileSnapshotStore>(Arc::new(std::sync::Mutex::new(HashMap::new())));

            // Initialize EditDiff snapshots
            app.manage::<commands::project::FileSnapshots>(Arc::new(std::sync::Mutex::new(HashMap::new())));

            // Initialize LSP manager
            app.manage::<Arc<LspManager>>(Arc::new(LspManager::new()));

            // Initialize Code Indexer (with default settings; actual model/key configured on reindex)
            {
                let default_settings = app.state::<Arc<RwLock<config::AppSettings>>>();
                let settings = default_settings.inner().blocking_read();
                let indexer = Arc::new(CodeIndexer::new(
                    settings.llm_provider.clone(),
                    settings.api_key.clone(),
                    None,
                    settings.embedding_model.clone(),
                ));

                // Load persisted index from SQLite DB (if exists)
                let db_path = config_path.join("code_index.db");
                let db_path_str = db_path.to_string_lossy().to_string();
                let load_result = tauri::async_runtime::block_on(indexer.load_from_db(&db_path_str));
                match load_result {
                    Ok(n) if n > 0 => log::info!("Loaded {} chunks from index DB: {}", n, db_path_str),
                    Ok(_) => log::info!("Index DB empty or not found: {}", db_path_str),
                    Err(e) => log::warn!("Failed to load index DB: {}", e),
                }

                app.manage::<Arc<CodeIndexer>>(indexer);
            }

            // Initialize QuestionAwaiters for AskUserQuestion
            app.manage::<QuestionAwaiters>(Arc::new(std::sync::Mutex::new(HashMap::new())));

            // Initialize ConfirmAwaiters for dangerous operation confirmation
            app.manage::<ConfirmAwaiters>(Arc::new(std::sync::Mutex::new(HashMap::new())));

            // Initialize ToolRegistry from tools.json (runtime file, fallback to embedded)
            let tools = agent::load_tools_from_disk();
            log::info!("Loaded {} tools from config", tools.len());
            app.manage::<ToolRegistry>(Arc::new(tools));

            // Initialize AgentRegistry from agents.json (runtime file, fallback to embedded)
            let agents = agent::definition::load_agents_from_disk();
            log::info!("Loaded {} agent definitions", agents.len());
            app.manage::<AgentRegistry>(Arc::new(agents));

            // Initialize MCP Registry (initially empty, populated by background task)
            let mcp_registry = Arc::new(McpRegistry::new());
            let mcp_tools: Arc<std::sync::Mutex<Vec<agent::ToolDefinition>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
            app.manage::<Arc<McpRegistry>>(mcp_registry.clone());
            app.manage::<Arc<std::sync::Mutex<Vec<agent::ToolDefinition>>>>(mcp_tools.clone());

            // Initialize Cloud Agent task manager
            let cloud_task_manager = Arc::new(agent::cloud::CloudTaskManager::new());
            app.manage::<commands::cloud::CloudTaskState>(cloud_task_manager);

            // Initialize PTY (terminal) state
            app.manage(PtyState::new());

            // Initialize Telemetry collector (separate from logging)
            let telemetry_dir = app.path().app_data_dir().unwrap_or_default();
            app.manage(telemetry::TelemetryCollector::new(&telemetry_dir));

            // Spawn background task to connect to configured MCP servers
            let mcp_registry_bg = mcp_registry.clone();
            let mcp_tools_bg = mcp_tools.clone();
            let mcp_servers_path = config_path.join("mcp_servers.json");
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(1)).await;

                let servers: Vec<mcp::McpServerConfig> = if mcp_servers_path.exists() {
                    match std::fs::read_to_string(&mcp_servers_path) {
                        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
                        Err(_) => vec![],
                    }
                } else {
                    let _ = std::fs::write(&mcp_servers_path, "[]");
                    vec![]
                };

                for server_config in servers {
                    let name = server_config.name.clone();
                    match mcp_registry_bg.connect(server_config).await {
                        Ok(count) => log::info!("[MCP] Connected to '{}', {} tools discovered", name, count),
                        Err(e) => log::warn!("[MCP] Failed to connect to '{}': {}", name, e),
                    }
                }

                // Populate MCP tool definitions for the agent to merge
                let mcp_schemas = mcp_registry_bg.get_tool_schemas().await;
                let tools: Vec<agent::ToolDefinition> = mcp_schemas
                    .iter()
                    .filter_map(|schema| {
                        let func = schema.get("function")?;
                        Some(agent::ToolDefinition {
                            name: func.get("name")?.as_str()?.to_string(),
                            description: func.get("description")?.as_str()?.to_string(),
                            parameters: func.get("parameters")?.clone(),
                        })
                    })
                    .collect();

                if !tools.is_empty() {
                    log::info!("[MCP] {} MCP tools available for agents", tools.len());
                    if let Ok(mut guard) = mcp_tools_bg.lock() {
                        *guard = tools;
                    }
                }
            });

            // Initialize file watcher
            app.manage::<Arc<std::sync::Mutex<FileWatcher>>>(Arc::new(std::sync::Mutex::new(FileWatcher::new())));

            // Initialize Skill Manager (global + project-level)
            {
                let global_skills_dir = config_path.join("skills");
                let project_skills_dir = app.state::<Arc<RwLock<config::AppSettings>>>()
                    .inner()
                    .blocking_read()
                    .project_paths
                    .first()
                    .map(|p| std::path::Path::new(p).join(".neecoder").join("skills"));

                let skill_manager = Arc::new(skill::SkillManager::new(
                    global_skills_dir,
                    project_skills_dir,
                ));
                skill_manager.ensure_default_files();
                app.manage::<commands::skill::SkillState>(skill_manager);
            }

            // Spawn background auto-reindex loop
            let watcher_for_reindex = app.state::<Arc<std::sync::Mutex<FileWatcher>>>().inner().clone();
            let indexer_for_reindex = app.state::<Arc<CodeIndexer>>().inner().clone();
            let config_path_clone = config_path.clone();
            tauri::async_runtime::spawn(async move {
                let db_path = config_path_clone.join("code_index.db");
                let db_path_str = db_path.to_string_lossy().to_string();
                loop {
                    tokio::time::sleep(Duration::from_secs(10)).await;

                    let events = {
                        let watcher = watcher_for_reindex.lock().unwrap();
                        watcher.poll_events(500)
                    };

                    for event in events {
                        let path = event.path;
                        let kind = event.kind;
                        let indexer = &indexer_for_reindex;
                        let should_save = {
                            match kind {
                                FileChangeKind::Created | FileChangeKind::Modified => {
                                    match tokio::fs::read_to_string(&path).await {
                                        Ok(content) => {
                                            let path_str = path.to_string_lossy().to_string();
                                            match indexer.index_file(&path_str, &content).await {
                                                Ok(n) => log::info!("Auto-reindexed {} ({} chunks)", path_str, n),
                                                Err(e) => log::warn!("Failed to reindex {}: {}", path_str, e),
                                            }
                                            true
                                        }
                                        Err(_) => false, // Binary file or non-UTF-8, skip
                                    }
                                }
                                FileChangeKind::Deleted => {
                                    let path_str = path.to_string_lossy().to_string();
                                    indexer.remove_file(&path_str).await;
                                    log::info!("Removed from index: {}", path_str);
                                    true
                                }
                            }
                        };
                        // Persist index changes to DB
                        if should_save {
                            if let Err(e) = indexer.save_to_db(&db_path_str).await {
                                log::warn!("Failed to save index to DB: {}", e);
                            }
                        }
                    }
                }
            });

            log::info!("NeeCoder initialized successfully");

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::config::get_settings,
            commands::config::update_settings,
            commands::config::get_app_logs,
            commands::config::get_log_path,
            commands::completion::request_completion,
            commands::completion::cancel_completion,
            commands::edit_inline::edit_inline,
            commands::chat::send_message,
            commands::chat::new_session,
            commands::chat::clear_session,
            commands::chat::list_sessions,
            commands::chat::delete_session,
            commands::chat::get_session_messages,
            commands::project::open_project,
            commands::project::get_file_tree,
            commands::project::read_file,
            commands::project::write_file,
            commands::project::create_file,
            commands::project::create_directory,
            commands::project::delete_file,
            commands::project::rename_file,
            commands::project::accept_change,
            commands::project::reject_change,
            commands::project::accept_all_changes,
            commands::project::reject_all_changes,
            commands::lsp::get_symbols,
            commands::lsp::get_hover_info,
            commands::lsp::start_lsp,
            commands::lsp::lsp_did_open,
            commands::lsp::lsp_did_change,
            commands::lsp::lsp_did_close,
            commands::lsp::shutdown_lsp,
            commands::search::search_codebase,
            commands::search::reindex_project,
            commands::search::index_file,
            commands::search::remove_from_index,
            commands::search::get_index_stats,
            commands::chat::answer_agent_question,
            commands::chat::answer_confirm,
            commands::chat::cancel_agent,
            commands::chat::get_agents,
            commands::agent::get_all_agents,
            commands::agent::save_agent,
            commands::agent::delete_agent,
            commands::agent::list_available_tools,
            commands::chat::restore_file,
            commands::chat::replay_session,
            commands::chat::fork_session,
            commands::chat::set_plan_mode,
            commands::chat::list_checkpoints,
            commands::chat::restore_checkpoint,
            commands::chat::create_branch,
            commands::chat::list_branches,
            commands::chat::delete_branch,
            commands::skill::list_skills,
            commands::skill::execute_skill,
            commands::skill::reload_skills,
            commands::skill::save_skill,
            commands::skill::delete_skill,
            commands::mcp::list_mcp_servers,
            commands::mcp::connect_mcp_server,
            commands::mcp::disconnect_mcp_server,
            commands::cloud::start_cloud_agent,
            commands::cloud::get_cloud_task,
            commands::cloud::list_cloud_tasks,
            commands::cloud::cancel_cloud_task,
            commands::pty::start_terminal,
            commands::pty::write_stdin,
            commands::pty::stop_terminal,
            commands::pty::resize_terminal,
            telemetry::get_telemetry_summary,
            telemetry::get_telemetry_events,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
