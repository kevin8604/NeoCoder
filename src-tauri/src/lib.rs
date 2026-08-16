pub mod a2a;
pub mod agent;
pub mod chat;
pub mod commands;
pub mod completion;
pub mod config;
pub mod event_bus;
pub mod fs_service;
pub mod fs_watcher;
pub mod llm;
pub mod logging;
pub mod lsp;
pub mod mcp;
pub mod memory;
pub mod rag;
pub mod sandbox;
pub mod skill;
pub mod telemetry;
pub mod terminal;

// Pure-logic tests executed under Miri (UB sanitizer) — compiled only when
// running with a nightly toolchain and `cargo miri`.
#[cfg(miri)]
mod miri_safe_tests;

use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;
use tokio::sync::RwLock;

use agent::ConfirmAwaiters;
use agent::PlanApprovalAwaiters;
use agent::QuestionAwaiters;
use agent::ToolRegistry;
use agent::definition::AgentRegistry;
use commands::pty::PtyState;
use fs_watcher::FileWatcher;
use lsp::LspManager;
use mcp::client::McpRegistry;
use rag::CodeIndexer;
use std::collections::HashMap;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 初始化日志系统（替换 env_logger）
    let app_data_dir = directories::ProjectDirs::from("com", "neocoder", "NeoCoder")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| {
            // Fallback: 使用当前目录下的 .neocoder
            std::env::current_dir()
                .unwrap_or_default()
                .join(".neocoder")
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
                memory: Arc::new(RwLock::new(chat::ConversationMemory::with_storage(
                    sessions_dir,
                ))),
                plan_mode_sessions: Arc::new(std::sync::Mutex::new(
                    std::collections::HashSet::new(),
                )),
            });

            // Initialize completion cache (LRU, max 200 entries)
            app.manage::<Arc<std::sync::Mutex<commands::completion::CompletionCache>>>(Arc::new(
                std::sync::Mutex::new(commands::completion::CompletionCache::new(200)),
            ));

            // Initialize completion cancel map
            app.manage::<commands::completion::CancelMap>(Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            )));

            // Initialize completion candidates store
            app.manage::<commands::completion::CompletionCandidates>(Arc::new(
                std::sync::Mutex::new(HashMap::new()),
            ));

            // Initialize edit-intent tracker (recently edited files → completion signal)
            app.manage::<Arc<completion::edit_intent::EditIntentTracker>>(Arc::new(
                completion::edit_intent::EditIntentTracker::new(),
            ));

            // Initialize Agent cancel map
            app.manage::<commands::chat::AgentCancelMap>(Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            )));

            // Initialize Agent pause control (session-scoped flag + notify)
            app.manage::<agent::PauseControl>(std::sync::Mutex::new(HashMap::new()));

            // Initialize checkpoint store (keyed by session_id)
            app.manage::<agent::checkpoint::CheckpointStore>(agent::checkpoint::new_store());

            // Initialize FileSnapshotStore for file undo mechanism
            app.manage::<commands::chat::FileSnapshotStore>(Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            )));

            // Initialize EditDiff snapshots
            app.manage::<commands::project::FileSnapshots>(Arc::new(std::sync::Mutex::new(
                HashMap::new(),
            )));

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

                // Load persisted index from the active workspace's per-workspace DB
                // (falls back to the legacy global code_index.db when no workspace is active)
                let db_path = workspace_index_db_path(&settings, &config_path);
                let load_result = tauri::async_runtime::block_on(indexer.load_from_db(&db_path));
                match load_result {
                    Ok(n) if n > 0 => log::info!("Loaded {} chunks from index DB: {}", n, db_path),
                    Ok(_) => log::info!("Index DB empty or not found: {}", db_path),
                    Err(e) => log::warn!("Failed to load index DB: {}", e),
                }

                app.manage::<Arc<CodeIndexer>>(indexer);
            }

            // Initialize QuestionAwaiters for AskUserQuestion
            app.manage::<QuestionAwaiters>(Arc::new(std::sync::Mutex::new(HashMap::new())));

            // Initialize ConfirmAwaiters for dangerous operation confirmation
            app.manage::<ConfirmAwaiters>(Arc::new(std::sync::Mutex::new(HashMap::new())));

            // Initialize PlanApprovalAwaiters for Plan Mode approval flow
            app.manage::<PlanApprovalAwaiters>(Arc::new(std::sync::Mutex::new(HashMap::new())));

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
            let mcp_tools: Arc<std::sync::Mutex<Vec<agent::ToolDefinition>>> =
                Arc::new(std::sync::Mutex::new(Vec::new()));
            app.manage::<Arc<McpRegistry>>(mcp_registry.clone());
            app.manage::<Arc<std::sync::Mutex<Vec<agent::ToolDefinition>>>>(mcp_tools.clone());

            // Initialize Cloud Agent task manager (persisted to disk so tasks
            // survive restarts; interrupted tasks can be resumed)
            let cloud_task_storage = config_path.join("cloud_tasks.json");
            let cloud_task_manager = Arc::new(agent::cloud::CloudTaskManager::with_storage(
                cloud_task_storage,
            ));
            app.manage::<commands::cloud::CloudTaskState>(cloud_task_manager);

            // Initialize PTY (terminal) state
            app.manage(PtyState::new());

            // Initialize A2A HTTP server (if enabled in settings)
            {
                let settings = app.state::<Arc<RwLock<config::AppSettings>>>();
                let a2a_enabled = settings.inner().blocking_read().a2a_server_enabled;
                let a2a_port = settings.inner().blocking_read().a2a_server_port;
                let a2a_token = settings.inner().blocking_read().a2a_server_token.clone();

                let runtime_state = commands::a2a::A2aRuntimeState::default();
                if a2a_enabled {
                    let token = if a2a_token.is_empty() {
                        None
                    } else {
                        Some(a2a_token)
                    };
                    match a2a::server::start_server(app.handle().clone(), a2a_port, token) {
                        Ok(()) => {
                            runtime_state
                                .running
                                .store(true, std::sync::atomic::Ordering::SeqCst);
                            runtime_state
                                .port
                                .store(a2a_port, std::sync::atomic::Ordering::SeqCst);
                            log::info!("[A2A] Server enabled on port {}", a2a_port);
                        }
                        Err(e) => log::warn!("[A2A] Failed to start server: {}", e),
                    }
                }
                app.manage::<commands::a2a::A2aRuntimeState>(runtime_state);
            }

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
                        Ok(count) => {
                            log::info!("[MCP] Connected to '{}', {} tools discovered", name, count)
                        }
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
            app.manage::<Arc<std::sync::Mutex<FileWatcher>>>(Arc::new(std::sync::Mutex::new(
                FileWatcher::new(),
            )));

            // Auto-start watching the active workspace (or most recent project)
            {
                let watch_target = {
                    let settings = app
                        .state::<Arc<RwLock<config::AppSettings>>>()
                        .inner()
                        .blocking_read();
                    settings
                        .active_workspace_id
                        .as_ref()
                        .and_then(|id| settings.workspaces.iter().find(|w| &w.id == id))
                        .map(|w| w.path.clone())
                        .or_else(|| settings.project_paths.first().cloned())
                };
                if let Some(ref path) = watch_target {
                    let watcher_state = app.state::<Arc<std::sync::Mutex<FileWatcher>>>();
                    let mut w = watcher_state
                        .inner()
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if let Err(e) = w.start_watch(std::path::Path::new(path), true) {
                        log::warn!("[FsWatcher] Failed to auto-watch '{}': {}", path, e);
                    } else {
                        log::info!("[FsWatcher] Auto-watching project: {}", path);
                    }
                }
            }

            // Initialize Skill Manager (global + active-workspace project level)
            {
                let global_skills_dir = config_path.join("skills");
                let project_skills_dir = {
                    let settings = app
                        .state::<Arc<RwLock<config::AppSettings>>>()
                        .inner()
                        .blocking_read();
                    settings
                        .active_workspace_id
                        .as_ref()
                        .and_then(|id| settings.workspaces.iter().find(|w| &w.id == id))
                        .map(|w| {
                            std::path::Path::new(&w.path)
                                .join(".neocoder")
                                .join("skills")
                        })
                        .or_else(|| {
                            settings
                                .project_paths
                                .first()
                                .map(|p| std::path::Path::new(p).join(".neocoder").join("skills"))
                        })
                };

                let skill_manager = Arc::new(skill::SkillManager::new(
                    global_skills_dir,
                    project_skills_dir,
                ));
                skill_manager.ensure_default_files();
                app.manage::<commands::skill::SkillState>(skill_manager);
            }

            // Spawn background auto-reindex loop
            let watcher_for_reindex = app
                .state::<Arc<std::sync::Mutex<FileWatcher>>>()
                .inner()
                .clone();
            let indexer_for_reindex = app.state::<Arc<CodeIndexer>>().inner().clone();
            let edit_intent_bg = app
                .state::<Arc<completion::edit_intent::EditIntentTracker>>()
                .inner()
                .clone();
            let settings_for_reindex = app
                .state::<Arc<RwLock<config::AppSettings>>>()
                .inner()
                .clone();
            let config_path_clone = config_path.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(10)).await;

                    let events = {
                        let watcher = watcher_for_reindex.lock().unwrap();
                        watcher.poll_events(500)
                    };

                    let mut index_changed = false;
                    for event in events {
                        // Record edit-intent for modified files (completion signal)
                        if matches!(
                            event.kind,
                            fs_watcher::FileChangeKind::Modified
                                | fs_watcher::FileChangeKind::Created
                        ) {
                            edit_intent_bg.record_edit(&event.path.to_string_lossy());
                        }
                        if indexer_for_reindex
                            .handle_file_change(&event.path, event.kind)
                            .await
                        {
                            index_changed = true;
                        }
                    }
                    // Persist index changes to the active workspace's DB (once per batch)
                    if index_changed {
                        let db_path = {
                            let settings = settings_for_reindex.read().await;
                            workspace_index_db_path(&settings, &config_path_clone)
                        };
                        if let Err(e) = indexer_for_reindex.save_to_db(&db_path).await {
                            log::warn!("Failed to save index to DB: {}", e);
                        }
                    }
                }
            });

            log::info!("NeoCoder initialized successfully");

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::config::get_settings,
            commands::config::update_settings,
            commands::config::get_app_logs,
            commands::config::get_log_path,
            commands::completion::request_completion,
            commands::completion::cancel_completion,
            commands::completion::cycle_completion,
            commands::edit_inline::edit_inline,
            commands::chat::send_message,
            commands::chat::new_session,
            commands::chat::clear_session,
            commands::chat::list_sessions,
            commands::chat::delete_session,
            commands::chat::get_session_messages,
            commands::project::open_project,
            commands::project::get_file_tree,
            commands::workspace::list_workspaces,
            commands::workspace::activate_workspace,
            commands::workspace::remove_workspace,
            commands::workspace::rename_workspace,
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
            commands::lsp::rename_symbol,
            commands::lsp::get_code_actions,
            commands::lsp::format_document,
            commands::search::search_codebase,
            commands::search::reindex_project,
            commands::search::index_file,
            commands::search::remove_from_index,
            commands::search::get_index_stats,
            commands::chat::answer_agent_question,
            commands::chat::answer_confirm,
            commands::chat::cancel_agent,
            commands::chat::pause_agent,
            commands::chat::resume_agent,
            commands::chat::resume_session,
            commands::chat::list_resumable_sessions,
            commands::chat::get_agents,
            commands::chat::approve_plan,
            commands::chat::reject_plan,
            commands::chat::skip_plan,
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
            commands::chat::checkpoint_diff,
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
            commands::cloud::resume_cloud_task,
            commands::pty::start_terminal,
            commands::pty::write_stdin,
            commands::pty::stop_terminal,
            commands::pty::resize_terminal,
            commands::dependency_graph::get_dependency_graph,
            commands::review::trigger_auto_review,
            commands::review::get_auto_review_settings,
            commands::memory::check_local_model,
            commands::memory::search_memory,
            commands::memory::preview_memory,
            commands::memory::list_notes,
            commands::memory::read_note,
            commands::memory::get_memory_stats,
            commands::memory::get_memory_entries,
            commands::memory::cleanup_memory,
            commands::memory::run_deep_dreaming,
            commands::memory::export_training_data,
            commands::a2a::get_a2a_status,
            commands::a2a::set_a2a_config,
            commands::a2a::list_remote_agents,
            commands::a2a::discover_remote_agent,
            commands::a2a::invoke_remote_agent,
            telemetry::get_telemetry_summary,
            telemetry::get_telemetry_events,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Resolve the code index DB path for the currently active workspace,
/// falling back to the legacy global `code_index.db` when no workspace
/// is active or the entry is missing.
fn workspace_index_db_path(settings: &config::AppSettings, config_dir: &std::path::Path) -> String {
    match settings
        .active_workspace_id
        .as_ref()
        .and_then(|id| settings.workspaces.iter().find(|w| &w.id == id))
    {
        Some(ws) if !ws.index_db_path.is_empty() => ws.index_db_path.clone(),
        Some(ws) => ws.index_db_path_for(config_dir),
        None => config_dir
            .join("code_index.db")
            .to_string_lossy()
            .to_string(),
    }
}
