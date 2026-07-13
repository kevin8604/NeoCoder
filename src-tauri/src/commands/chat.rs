use tauri::{Emitter, State, Manager};
use crate::agent;
use crate::agent::definition::AgentRegistry;
use crate::agent::QuestionAwaiters;
use crate::agent::ConfirmAwaiters;
use crate::chat::{ChatMode, ChatEvent, ConversationMemory, CHAT_SYSTEM_PROMPT};
use crate::config::AppSettings;
use crate::llm::{self, ChatMessage as LlmMessage, ChatRequestParams};
use crate::rag::CodeIndexer;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Store active Agent cancellation flags, keyed by session_id
pub type AgentCancelMap = Arc<std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>>;

/// Store file snapshots for undo mechanism, keyed by session_id -> (file_path -> original_content)
pub type FileSnapshotStore = Arc<std::sync::Mutex<HashMap<String, HashMap<String, String>>>>;

const EDIT_SYSTEM_PROMPT: &str = "You are an AI coding assistant in Edit mode. Your task is to help the user make precise code changes.

When suggesting file modifications, you MUST format each code change as follows:


```language:path/to/file.rs
// NEW content for the file
```

Rules:
1. Always include the file path after the language identifier, separated by a colon
2. If the file doesn't exist yet, include the full content to create it
3. For existing files, show only the CHANGED section with a few lines of surrounding context — do NOT reproduce the entire file unless the change genuinely affects most of it
4. Keep changes minimal and targeted; preserve existing indentation, style, and unrelated code
5. You can suggest changes to multiple files - each file gets its own code block
6. Explain the reasoning for each change briefly
7. Use this format: ```rust:src/main.rs
...
```";

pub struct ChatState {
    pub memory: Arc<RwLock<ConversationMemory>>,
}

/// Sanitize conversation messages for LLM API compatibility.
/// - Non-agent mode: removes all tool/tool_calls messages
/// - Agent mode: ensures tool messages are properly paired (removes orphans)
fn sanitize_messages(
    messages: &[crate::chat::ChatMessage],
    is_agent: bool,
) -> Vec<crate::chat::ChatMessage> {
    if !is_agent {
        // In Ask/Edit mode, strip all tool-related messages entirely
        return messages.iter().filter(|m| {
            !matches!(m.role, crate::chat::Role::Tool) && m.tool_calls.is_none()
        }).cloned().collect();
    }

    // In Agent mode: ensure proper tool_calls → tool pairing
    let mut result = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let msg = &messages[i];
        if matches!(msg.role, crate::chat::Role::Assistant) && msg.tool_calls.is_some() {
            // Check if next message(s) are tool responses
            let mut has_tool_response = false;
            let mut j = i + 1;
            while j < messages.len() && matches!(messages[j].role, crate::chat::Role::Tool) {
                has_tool_response = true;
                j += 1;
            }
            if has_tool_response {
                // Keep assistant + tool messages
                result.push(msg.clone());
                for k in (i + 1)..j {
                    result.push(messages[k].clone());
                }
                i = j;
            } else {
                // Orphaned assistant with tool_calls — convert to plain assistant
                result.push(crate::chat::ChatMessage {
                    role: crate::chat::Role::Assistant,
                    content: if msg.content.is_empty() {
                        "[Tool call was made but no response received]".to_string()
                    } else {
                        msg.content.clone()
                    },
                    images: None,
                    tool_calls: None,
                });
                i += 1;
            }
        } else if matches!(msg.role, crate::chat::Role::Tool) {
            // Orphaned tool message (no preceding assistant with tool_calls) — skip
            i += 1;
        } else {
            result.push(msg.clone());
            i += 1;
        }
    }
    result
}

#[tauri::command]
pub async fn send_message(
    app: tauri::AppHandle,
    session_id: String,
    message: String,
    mode: ChatMode,
    agent_id: Option<String>,
    project_path: Option<String>,
    context_files: Option<Vec<String>>,
    plan_mode: Option<bool>,
    // Optional base64-encoded images (data:image/...;base64,...)
    images: Option<Vec<String>>,
    // Directory paths to expand as context (all source files inside)
    context_folders: Option<Vec<String>>,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
    chat_state: State<'_, ChatState>,
) -> Result<String, String> {
    let memory = chat_state.memory.read().await;
    let settings = settings.read().await;

    // Build messages array for LLM API
    let mut messages: Vec<LlmMessage> = vec![];

    // Inject context files as system messages (from @ file mentions) — parallel read
    if let Some(ref files) = context_files {
        if !files.is_empty() {
            let read_futures = files.iter().map(|fp| {
                let fp = fp.clone();
                async move {
                    match tokio::fs::read_to_string(&fp).await {
                        Ok(content) => Some((fp, content)),
                        Err(e) => {
                            log::warn!("[Chat] Failed to read context file {}: {}", fp, e);
                            None
                        }
                    }
                }
            });
            let results = futures_util::future::join_all(read_futures).await;
            for result in results.into_iter().flatten() {
                let (file_path, content) = result;
                let truncated = if content.len() > 50_000 {
                    // Use char-aware truncation to avoid UTF-8 boundary panics
                    let truncated: String = content.chars().take(50_000).collect();
                    format!("{}... [truncated at 50KB]", truncated)
                } else {
                    content
                };
                messages.push(LlmMessage {
                    role: "system".into(),
                    content: format!("File: {}\n```\n{}\n```", file_path, truncated),
                    images: None,
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
        }
    }

    // ── Expand context folders: recursively read source files ──
    if let Some(ref folders) = context_folders {
        if !folders.is_empty() {
            // Collect all file paths from all folders first (bounded by depth and count)
            const MAX_FILES_PER_FOLDER: usize = 50;
            const MAX_FILE_SIZE: usize = 50_000;
            const MAX_DEPTH: usize = 3;

            // Common source file extensions to include (skip binaries, images, etc.)
            let is_source_file = |path: &std::path::Path| -> bool {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    matches!(ext,
                        "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "java" | "kt" |
                        "c" | "cpp" | "h" | "hpp" | "cs" | "rb" | "php" | "swift" | "dart" |
                        "vue" | "svelte" | "html" | "css" | "scss" | "less" | "json" | "yaml" |
                        "yml" | "toml" | "xml" | "md" | "txt" | "sql" | "sh" | "bash" | "zsh" |
                        "ps1" | "bat" | "cmd" | "dockerfile" | "docker-compose" | "env" | "lock" |
                        "graphql" | "proto" | "lua" | "r" | "jl" | "ex" | "exs" | "hs" | "ml" |
                        "zig" | "nim" | "scala" | "clj" | "erl" | "elm" | "tf" | "hcl" | "ini" |
                        "cfg" | "conf" | "log" | "csv"
                    )
                } else {
                    // Files without extension (e.g., Makefile, Dockerfile, LICENSE)
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| matches!(n, "Makefile" | "Dockerfile" | "LICENSE" | "README" | "CHANGELOG" | ".gitignore" | ".env"))
                        .unwrap_or(false)
                }
            };

            // Skip common non-source directories
            let should_skip_dir = |name: &str| -> bool {
                matches!(name,
                    "node_modules" | ".git" | "target" | "dist" | "build" | "__pycache__" |
                    ".next" | ".nuxt" | ".cache" | "vendor" | ".idea" | ".vscode" | "bin" | "obj"
                )
            };

            for folder_path in folders {
                let folder = std::path::Path::new(folder_path);
                if !folder.is_dir() {
                    log::warn!("[Chat] Context folder is not a directory: {}", folder_path);
                    continue;
                }

                // Collect files using recursive walk with depth limit
                let mut files_to_read: Vec<String> = Vec::new();
                let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(folder.to_path_buf(), 0)];

                while let Some((dir, depth)) = stack.pop() {
                    if depth > MAX_DEPTH || files_to_read.len() >= MAX_FILES_PER_FOLDER {
                        break;
                    }
                    match tokio::fs::read_dir(&dir).await {
                        Ok(mut entries) => {
                            while let Ok(Some(entry)) = entries.next_entry().await {
                                let entry_path = entry.path();
                                let entry_name = entry.file_name().to_string_lossy().to_string();

                                if entry_path.is_dir() {
                                    if !should_skip_dir(&entry_name) {
                                        stack.push((entry_path, depth + 1));
                                    }
                                } else if is_source_file(&entry_path) {
                                    files_to_read.push(entry_path.to_string_lossy().to_string());
                                    if files_to_read.len() >= MAX_FILES_PER_FOLDER {
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("[Chat] Failed to read directory {}: {}", folder_path, e);
                        }
                    }
                }

                log::info!("[Chat] Expanding context folder '{}': {} files found", folder_path, files_to_read.len());

                // Read all files in parallel
                let read_futures = files_to_read.iter().map(|fp| {
                    let fp = fp.clone();
                    async move {
                        match tokio::fs::read_to_string(&fp).await {
                            Ok(content) => Some((fp, content)),
                            Err(e) => {
                                log::debug!("[Chat] Failed to read file in folder {}: {}", fp, e);
                                None
                            }
                        }
                    }
                });
                let results = futures_util::future::join_all(read_futures).await;
                let mut included_count = 0;
                for result in results.into_iter().flatten() {
                    let (file_path, content) = result;
                    if content.len() > MAX_FILE_SIZE {
                        continue; // Skip very large files in folder context
                    }
                    messages.push(LlmMessage {
                        role: "system".into(),
                        content: format!("File: {}\n```\n{}\n```", file_path, content),
                        images: None,
                        tool_calls: None,
                        tool_call_id: None,
                    });
                    included_count += 1;
                }

                // Add a summary message so LLM knows the folder structure
                messages.push(LlmMessage {
                    role: "system".into(),
                    content: format!(
                        "[FOLDER_CONTEXT] Directory '{}' included {} source files as context. \
                         Use the Agent's file reading tools for any files not shown above.",
                        folder_path, included_count
                    ),
                    images: None,
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
        }
    }

    // Add existing conversation history (sanitized for API compatibility)
    let context_window = crate::config::model_context_window(&settings.chat_model);
    let context_messages = memory.get_context_window(&session_id, context_window);
    let is_agent = matches!(mode, ChatMode::Agent);
    let sanitized = sanitize_messages(&context_messages, is_agent);
    for msg in &sanitized {
        let tool_calls_json = msg.tool_calls.as_ref().map(|tcs| {
            serde_json::Value::Array(tcs.iter().map(|tc| {
                serde_json::json!({
                    "id": tc.id,
                    "type": "function",
                    "function": {
                        "name": tc.tool_name,
                        "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                    }
                })
            }).collect())
        });
        let msg_images: Option<Vec<llm::ImageContent>> = msg.images.as_ref().map(|imgs| {
            imgs.iter().map(|url| llm::ImageContent {
                url: url.clone(),
                detail: Some("auto".into()),
            }).collect()
        });
        messages.push(LlmMessage {
            role: match msg.role {
                crate::chat::Role::User => "user",
                crate::chat::Role::Assistant => "assistant",
                crate::chat::Role::System => "system",
                crate::chat::Role::Tool => "tool",
            }.into(),
            content: msg.content.clone(),
            images: msg_images,
            tool_calls: tool_calls_json,
            tool_call_id: None,
        });
    }

    // Add user message
    let llm_images: Option<Vec<llm::ImageContent>> = images.as_ref().map(|imgs| {
        imgs.iter().map(|url| llm::ImageContent {
            url: url.clone(),
            detail: Some("auto".into()),
        }).collect()
    });
    messages.push(LlmMessage {
        role: "user".into(),
        content: message.clone(),
        images: llm_images,
        tool_calls: None,
        tool_call_id: None,
    });

    // ── Extract memory context & project instructions (shared by all modes) ──
    let memory_context = memory.memory_manager().inject_memory_context();
    let effective_project_path = project_path
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| settings.project_paths.first().cloned());
    let project_instructions = {
        if let Some(ref pp) = effective_project_path {
            let file_path = std::path::Path::new(pp).join(".neecoder").join("instructions.md");
            std::fs::read_to_string(&file_path).ok().filter(|c| !c.trim().is_empty())
        } else {
            None
        }
    };

    // ── Agent 模式 ──
    if is_agent {
        let memory_manager = memory.memory_manager();
        let agent_memory_context = memory_manager.inject_memory_context();
        drop(memory);
        let provider = settings.llm_provider.clone();
        let api_key = settings.api_key.clone();
        let chat_model = settings.chat_model.clone();

        // Persist user message immediately (survives refresh/restart)
        let user_msg_for_storage = message.clone();
        let memory_arc = chat_state.memory.clone();
        {
            let mem = memory_arc.write().await;
            mem.add_message(&session_id, crate::chat::ChatMessage {
                role: crate::chat::Role::User,
                content: user_msg_for_storage,
                images: images.clone(),
                tool_calls: None,
            });
        }

        // Determine which agent to use
        let effective_agent_id = agent_id.unwrap_or_else(|| "orchestrator".to_string());

        // Look up agent definition
        let agent_def: Option<crate::agent::definition::AgentDefinition> = app.try_state::<AgentRegistry>()
            .and_then(|registry| crate::agent::definition::find_agent(registry.inner(), &effective_agent_id));

        // Load custom instructions from settings + .neecoder/instructions.md
        let project_path = effective_project_path.clone();
        let mut custom_instructions = String::new();
        if !settings.custom_instructions.trim().is_empty() {
            custom_instructions.push_str(&settings.custom_instructions);
        }
        // Try loading project-level instructions
        if let Some(ref pp) = project_path {
            let file_path = std::path::Path::new(pp).join(".neecoder").join("instructions.md");
            if let Ok(content) = std::fs::read_to_string(&file_path) {
                if !content.trim().is_empty() {
                    if !custom_instructions.is_empty() {
                        custom_instructions.push_str("\n\n");
                    }
                    custom_instructions.push_str(&content);
                }
            }
        }
        // Inject memory context (MEMORY.md + recent notes)
        if !agent_memory_context.is_empty() {
            if !custom_instructions.is_empty() {
                custom_instructions.push_str("\n\n");
            }
            custom_instructions.push_str(&agent_memory_context);
        }
        let custom_instructions = if custom_instructions.is_empty() { None } else { Some(custom_instructions) };

        // ── #codebase / @codebase RAG 自动注入 ──
        let mut messages = messages;
        let has_codebase = message.contains("#codebase") || message.contains("@codebase");
        if has_codebase {
            if let Some(indexer) = app.try_state::<Arc<CodeIndexer>>() {
                let indexer = indexer.inner().clone();
                // Extract the actual query (remove markers)
                let query = message.replace("#codebase", "").replace("@codebase", "").trim().to_string();
                let search_query = if query.is_empty() { &message } else { &query };
                if let Ok(results) = indexer.hybrid_search(search_query, 5).await {
                    if !results.is_empty() {
                        let ctx = crate::rag::build_rag_context(
                            &results.iter().map(|r| r.chunk.clone()).collect::<Vec<_>>(),
                            5,
                        );
                        // Inject as system context at the beginning
                        messages.insert(0, LlmMessage {
                            role: "system".into(),
                            content: format!("The user has requested codebase context. \
                                Here are the relevant code chunks from the project:\n\n{}", ctx),
                            images: None,
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                }
            }
        }

        // ── 智能上下文注入：Agent 模式自动检索相关文件 ──
        if !has_codebase {
            if let Some(ref ctx_files) = context_files {
                if ctx_files.is_empty() {
                    // No explicit context files — auto-inject relevant files from index
                    if let Some(indexer) = app.try_state::<Arc<CodeIndexer>>() {
                        let indexer = indexer.inner().clone();
                        if let Ok(results) = indexer.hybrid_search(&message, 3).await {
                            if !results.is_empty() {
                                // Group by file path, take top 3 unique files
                                let mut seen_files = std::collections::HashSet::new();
                                let mut auto_context = Vec::new();
                                for r in &results {
                                    let file_path = r.chunk.file_path.clone();
                                    if seen_files.insert(file_path.clone()) && auto_context.len() < 3 {
                                        auto_context.push(file_path);
                                    }
                                }
                                if !auto_context.is_empty() {
                                    let read_futures = auto_context.iter().map(|fp| {
                                        let fp = fp.clone();
                                        async move {
                                            match tokio::fs::read_to_string(&fp).await {
                                                Ok(content) => {
                                                    let truncated = if content.len() > 20_000 {
                                                        // Use char-aware truncation to avoid UTF-8 boundary panics
                                                        let truncated: String = content.chars().take(20_000).collect();
                                                        format!("{}... [truncated at 20KB]", truncated)
                                                    } else {
                                                        content
                                                    };
                                                    Some((fp, truncated))
                                                }
                                                Err(_) => None,
                                            }
                                        }
                                    });
                                    let file_results = futures_util::future::join_all(read_futures).await;
                                    for file_result in file_results.into_iter().flatten() {
                                        let (file_path, content) = file_result;
                                        messages.insert(0, LlmMessage {
                                            role: "system".into(),
                                            content: format!("Auto-context file: {}\n```\n{}\n```", file_path, content),
                                            images: None,
                                            tool_calls: None,
                                            tool_call_id: None,
                                        });
                                    }
                                    log::info!("[AutoContext] Injected {} auto-context files", auto_context.len());
                                }
                            }
                        }
                    }
                }
            }
        }

        // Register Agent cancellation flag
        let cancel_flag = Arc::new(AtomicBool::new(false));
        if let Some(cancel_map) = app.try_state::<AgentCancelMap>() {
            if let Ok(mut map) = cancel_map.lock() {
                map.insert(session_id.clone(), cancel_flag.clone());
            }
        }

        let agent_def_cloned = agent_def.clone();
        let session_id_for_cleanup = session_id.clone();
        let memory_manager_for_flush = memory_manager.clone();

        let is_plan_mode = plan_mode.unwrap_or(false);

        // ── P0-2: Pre-flight validation ──
        if api_key.trim().is_empty() && !matches!(provider, crate::config::LlmProvider::Ollama) {
            return Err("No API key configured. Please set your API key in Settings.".to_string());
        }
        if chat_model.trim().is_empty() {
            return Err("No chat model configured. Please select a model in Settings.".to_string());
        }
        if !effective_agent_id.is_empty() && effective_agent_id != "orchestrator" && agent_def.is_none() {
            return Err(format!("Agent '{}' not found in registry. Available agents: orchestrator, explorer, reviewer, architect", effective_agent_id));
        }

        tokio::spawn(async move {
            log::info!("[Agent] Spawn started for session '{}', agent '{}' (plan_mode={})", session_id, effective_agent_id, is_plan_mode);

            // ── P0-1: Panic-safe agent execution ──
            // Run the agent in a nested spawn so any panic is caught as a JoinError.
            // Clone all moved values for use after the inner spawn completes.
            let app2 = app.clone();
            let session_id2 = session_id.clone();
            let provider2 = provider.clone();
            let api_key2 = api_key.clone();
            let chat_model2 = chat_model.clone();
            let project_path2 = project_path.clone();
            let custom_instructions2 = custom_instructions.clone();
            let cancel_flag2 = cancel_flag.clone();
            let agent_def_cloned2 = agent_def_cloned.clone();
            let agent_memory_context2 = agent_memory_context.clone();
            let agent_handle = tokio::spawn(async move {
                agent::run_agent(
                    &app2,
                    &session_id2,
                    &messages,
                    &provider2,
                    &api_key2,
                    None,
                    &chat_model2,
                    project_path2.as_deref(),
                    custom_instructions2,
                    cancel_flag2,
                    agent_def_cloned2.as_ref(),
                    is_plan_mode,
                    Some(agent_memory_context2.clone()),
                ).await
            });

            let result = match agent_handle.await {
                Ok(r) => r,
                Err(join_err) => {
                    // Extract panic message if available
                    let panic_msg = if join_err.is_panic() {
                        if let Ok(payload) = join_err.try_into_panic() {
                            if let Some(s) = payload.downcast_ref::<String>() {
                                s.clone()
                            } else if let Some(s) = payload.downcast_ref::<&str>() {
                                s.to_string()
                            } else {
                                "Unknown panic payload".to_string()
                            }
                        } else {
                            "Panic (payload unavailable)".to_string()
                        }
                    } else {
                        format!("Task cancelled: {}", join_err)
                    };

                    log::error!("[Agent] Task panicked for session '{}': {}", session_id_for_cleanup, panic_msg);

                    // Cleanup cancel flag
                    if let Some(cancel_map) = app.try_state::<AgentCancelMap>() {
                        if let Ok(mut map) = cancel_map.lock() {
                            map.remove(&session_id_for_cleanup);
                        }
                    }

                    // Emit error to frontend
                    let _ = app.emit("chat-event", ChatEvent::Error {
                        session_id: session_id.clone(),
                        agent_id: Some(effective_agent_id.clone()),
                        message: format!("Agent task crashed: {}", panic_msg),
                    });
                    return;
                }
            };

            // Cleanup cancel flag
            if let Some(cancel_map) = app.try_state::<AgentCancelMap>() {
                if let Ok(mut map) = cancel_map.lock() {
                    map.remove(&session_id_for_cleanup);
                }
            }

            // Persist final result to conversation memory
            let result_content = match &result {
                Ok(text) => text.clone(),
                Err(e) => {
                    log::error!("[Agent] Failed for session '{}': {}", session_id_for_cleanup, e);
                    format!("Error: {}", e)
                }
            };
            let mem = memory_arc.write().await;
            mem.add_message(&session_id_for_cleanup, crate::chat::ChatMessage {
                role: crate::chat::Role::Assistant,
                content: result_content.clone(),
                images: None,
                tool_calls: None,
            });

            // Memory flush: append session summary to today's notes
            let note = format!(
                "Agent '{}' completed: {}",
                agent_def_cloned.as_ref().map(|d| d.id.as_str()).unwrap_or("agent"),
                result_content.chars().take(150).collect::<String>()
            );
            let _ = memory_manager_for_flush.append_note(&note);

            // Collect session messages for dreaming before dropping lock
            let context_window = crate::config::model_context_window(&chat_model);
            let session_msgs: Vec<crate::chat::ChatMessage> = mem.get_context_window(&session_id_for_cleanup, context_window);
            drop(mem);

            // Dreaming: fire-and-forget LLM summarization of session → MEMORY.md
            let memory_mgr = memory_manager_for_flush.clone();
            let provider_clone = provider.clone();
            let api_key_clone = api_key.clone();
            let chat_model_clone = chat_model.clone();
            tokio::spawn(async move {
                memory_mgr.dreaming(
                    &session_msgs,
                    &provider_clone,
                    &api_key_clone,
                    None,
                    &chat_model_clone,
                ).await;
            });

            if let Err(e) = result {
                log::error!("[Agent] Emitting error event: {}", e);
                let _ = app.emit("chat-event", ChatEvent::Error {
                    session_id: session_id.clone(),
                    agent_id: None,
                    message: e,
                });
            }
        });

        return Ok("Agent started".to_string());
    }

    // ── Ask / Edit mode ──

    // Drop read locks before acquiring write lock
    drop(memory);
    // Persist user message to disk (survives refresh/restart)
    let memory_arc = chat_state.memory.clone();
    {
        let mem = memory_arc.write().await;
        mem.add_message(&session_id, crate::chat::ChatMessage {
            role: crate::chat::Role::User,
            content: message.clone(),
            images: images.clone(),
            tool_calls: None,
        });
    }

    // Emit started event
    log::info!("[Chat] Starting stream for session {}", session_id);
    let _ = app.emit("chat-event", ChatEvent::Started {
        session_id: session_id.clone(),
        agent_id: None,
    });

    let app_clone = app.clone();
    let session_id_clone = session_id.clone();
    let provider = settings.llm_provider.clone();
    let api_key = settings.api_key.clone();
    let chat_model = settings.chat_model.clone();

    let base_prompt = match mode {
        ChatMode::Edit => EDIT_SYSTEM_PROMPT,
        _ => CHAT_SYSTEM_PROMPT,
    };

    // Inject memory context + project instructions to fix "amnesia" in Ask/Edit mode
    let mut system_prompt = base_prompt.to_string();
    if !memory_context.is_empty() {
        system_prompt.push_str("\n\n## Cross-session Memory\n\n");
        system_prompt.push_str(&memory_context);
    }
    if let Some(ref instructions) = project_instructions {
        system_prompt.push_str("\n\n## Project Instructions\n\n");
        system_prompt.push_str(instructions);
    }

    let request = ChatRequestParams {
        model: chat_model,
        messages,
        system: system_prompt,
        max_tokens: 4096,
        temperature: 0.7,
        thinking_enabled: settings.thinking_enabled,
        thinking_budget: settings.thinking_budget,
    };

    // Shared accumulator for the full response text
    let full_text: Arc<std::sync::Mutex<String>> = Arc::new(std::sync::Mutex::new(String::new()));
    let memory_for_stream = memory_arc.clone();

    // Spawn background task to stream the response
    tokio::spawn(async move {
        let full_text_inner = full_text.clone();
        let result = llm::stream_chat(
            &provider,
            &api_key,
            None,
            request,
            |token| {
                // Accumulate tokens for later persistence
                if let Ok(mut s) = full_text_inner.lock() {
                    s.push_str(&token);
                }
                let _ = app_clone.emit("chat-event", ChatEvent::Delta {
                    session_id: session_id_clone.clone(),
                    agent_id: None,
                    token,
                });
                Ok(())
            },
            None,
        ).await;

        match result {
            Ok(()) => {
                log::info!("[Chat] Stream completed for session {}", session_id_clone);
                let accumulated = full_text.lock()
                    .map(|s| s.clone())
                    .unwrap_or_default();

                // Persist assistant message to disk
                {
                    let mem = memory_for_stream.write().await;
                    mem.add_message(&session_id_clone, crate::chat::ChatMessage {
                        role: crate::chat::Role::Assistant,
                        content: accumulated.clone(),
                        images: None,
                        tool_calls: None,
                    });
                }

                let _ = app_clone.emit("chat-event", ChatEvent::Finished {
                    session_id: session_id_clone.clone(),
                    agent_id: None,
                    full_text: accumulated,
                });
            }
            Err(e) => {
                log::error!("[Chat] Stream error for session {}: {}", session_id_clone, e);

                // Persist partial response if any tokens were received
                let accumulated = full_text.lock()
                    .map(|s| s.clone())
                    .unwrap_or_default();
                if !accumulated.is_empty() {
                    let mem = memory_for_stream.write().await;
                    mem.add_message(&session_id_clone, crate::chat::ChatMessage {
                        role: crate::chat::Role::Assistant,
                        content: accumulated,
                        images: None,
                        tool_calls: None,
                    });
                }

                let _ = app_clone.emit("chat-event", ChatEvent::Error {
                    session_id: session_id_clone.clone(),
                    agent_id: None,
                    message: e,
                });
            }
        }
    });

    Ok("Streaming started".to_string())
}

#[tauri::command]
pub async fn cancel_agent(
    app: tauri::AppHandle,
    session_id: String,
    cancel_map: State<'_, AgentCancelMap>,
) -> Result<(), String> {
    if let Ok(mut map) = cancel_map.lock() {
        if let Some(flag) = map.get(&session_id) {
            flag.store(true, Ordering::SeqCst);
            map.remove(&session_id);
        }
    }
    let _ = app.emit("chat-event", ChatEvent::Cancelled { session_id, agent_id: None });
    Ok(())
}

#[tauri::command]
pub async fn new_session(
    chat_state: State<'_, ChatState>,
) -> Result<String, String> {
    let memory = chat_state.memory.write().await;
    Ok(memory.create_session())
}

#[tauri::command]
pub async fn list_sessions(
    chat_state: State<'_, ChatState>,
) -> Result<Vec<SessionInfo>, String> {
    let memory = chat_state.memory.read().await;
    let sessions = memory.get_all_sessions();
    Ok(sessions.into_iter().map(|s| SessionInfo {
        id: s.id,
        title: s.title,
        message_count: s.message_count,
        created_at: s.created_at.to_rfc3339(),
    }).collect())
}

#[tauri::command]
pub async fn delete_session(
    chat_state: State<'_, ChatState>,
    session_id: String,
) -> Result<(), String> {
    let memory = chat_state.memory.write().await;
    memory.delete_session(&session_id);
    Ok(())
}

#[derive(serde::Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub message_count: usize,
    pub created_at: String,
}

#[tauri::command]
pub async fn clear_session(
    chat_state: State<'_, ChatState>,
    session_id: String,
) -> Result<(), String> {
    let memory = chat_state.memory.write().await;
    memory.clear_session(&session_id);
    Ok(())
}

/// Load session message history for frontend display.
/// Returns only user and assistant messages (tool messages filtered out).
#[derive(serde::Serialize)]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
}

#[tauri::command]
pub async fn get_session_messages(
    chat_state: State<'_, ChatState>,
    session_id: String,
) -> Result<Vec<SessionMessage>, String> {
    let memory = chat_state.memory.read().await;
    let messages = memory.get_context_window(&session_id, 48000);
    let result: Vec<SessionMessage> = messages
        .into_iter()
        .filter(|m| matches!(m.role, crate::chat::Role::User | crate::chat::Role::Assistant))
        .filter(|m| !m.content.is_empty())
        .map(|m| SessionMessage {
            role: match m.role {
                crate::chat::Role::User => "user".to_string(),
                crate::chat::Role::Assistant => "assistant".to_string(),
                _ => "system".to_string(),
            },
            content: m.content,
        })
        .collect();
    Ok(result)
}

#[tauri::command]
pub async fn get_agents(
    registry: State<'_, AgentRegistry>,
) -> Result<Vec<crate::agent::definition::AgentDefinition>, String> {
    Ok(registry.as_ref().clone())
}

#[tauri::command]
pub async fn answer_agent_question(
    awaiters: State<'_, QuestionAwaiters>,
    question_id: String,
    answers: Vec<String>,
) -> Result<(), String> {
    let mut map = awaiters.lock().map_err(|e| format!("Lock error: {}", e))?;
    if let Some(sender) = map.remove(&question_id) {
        let _ = sender.send(answers.join("\n"));
    }
    Ok(())
}

#[tauri::command]
pub async fn answer_confirm(
    confirm_awaiters: State<'_, ConfirmAwaiters>,
    confirm_id: String,
    allowed: bool,
) -> Result<(), String> {
    let mut map = confirm_awaiters.lock().map_err(|e| format!("Lock error: {}", e))?;
    if let Some(sender) = map.remove(&confirm_id) {
        let _ = sender.send(allowed);
    }
    Ok(())
}

#[tauri::command]
pub fn get_terminal_history(count: Option<usize>) -> Vec<String> {
    let n = count.unwrap_or(3);
    let entries = crate::agent::tools::run_terminal_command::get_recent_terminal(n);
    entries.into_iter().map(|(cmd, output, exit)| {
        format!("$ {}\n{}\nExit: {}", cmd, output, exit)
    }).collect()
}

#[tauri::command]
pub fn get_error_summary() -> String {
    crate::agent::tools::run_terminal_command::get_error_summary()
}

#[tauri::command]
pub async fn restore_file(
    app: tauri::AppHandle,
    file_snapshots: State<'_, FileSnapshotStore>,
    session_id: String,
    file_path: String,
) -> Result<String, String> {
    let mut snapshots = file_snapshots.lock().map_err(|e| format!("Lock error: {}", e))?;

    let session_snapshots = snapshots
        .get_mut(&session_id)
        .ok_or_else(|| format!("No snapshots found for session: {}", session_id))?;

    let original_content = session_snapshots
        .get(&file_path)
        .ok_or_else(|| format!("No snapshot found for file: {}", file_path))?
        .clone();

    // Write the original content back to the file
    std::fs::write(&file_path, &original_content)
        .map_err(|e| format!("Failed to restore file: {}", e))?;

    // Emit file restored event
    let _ = app.emit(
        "chat-event",
        ChatEvent::FileRestored {
            session_id: session_id.clone(),
            agent_id: None,
            file_path: file_path.clone(),
            content: original_content,
        },
    );

    log::info!("File restored via command: {}", file_path);
    Ok(format!("File '{}' restored successfully", file_path))
}

/// Replay an agent session's log entries for frontend display/debugging.
/// Returns the raw JSONL log entries in order.
#[tauri::command]
pub async fn replay_session(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<Vec<crate::memory::agent_log::LogEntry>, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|_| "Failed to get config dir".to_string())?;

    let log_path = config_dir
        .join("sessions")
        .join("agent_logs")
        .join(format!("{}.jsonl", session_id));

    if !log_path.exists() {
        return Ok(Vec::new());
    }

    crate::memory::agent_log::AgentLog::replay(&log_path).await
}

/// Set plan mode for a session.
/// When enabled, the agent starts in Planning phase (read-only analysis).
/// The actual plan_mode is applied when `send_message` is called with `plan_mode: Some(true)`.
#[tauri::command]
pub async fn set_plan_mode(
    session_id: String,
    enabled: bool,
) -> Result<bool, String> {
    log::info!(
        "[Chat] Plan mode set to {} for session '{}'",
        enabled,
        session_id
    );
    Ok(enabled)
}

/// List all checkpoints for a session.
#[tauri::command]
pub async fn list_checkpoints(
    session_id: String,
    store: State<'_, crate::agent::checkpoint::CheckpointStore>,
) -> Result<Vec<crate::agent::checkpoint::Checkpoint>, String> {
    let store = store.lock().unwrap_or_else(|e| e.into_inner());
    Ok(store.get(&session_id).cloned().unwrap_or_default())
}

/// Fork a session to create a new branch from an existing session.
/// Copies all messages up to fork_point (if specified) to a new session.
#[tauri::command]
pub async fn fork_session(
    chat_state: State<'_, ChatState>,
    source_session_id: String,
    fork_point: Option<usize>,
) -> Result<String, String> {
    let memory = chat_state.memory.read().await;

    // Load all messages from source session
    let source_messages = memory.get_context_window(&source_session_id, 48000);

    // Create new session
    let new_session_id = memory.create_session();

    // Copy messages up to fork_point (if specified), otherwise all
    let copy_count = fork_point.unwrap_or(source_messages.len()).min(source_messages.len());
    let messages_to_copy = &source_messages[..copy_count];

    // Write copied messages to new session
    let mem_mgr = memory.memory_manager();
    for msg in messages_to_copy {
        mem_mgr.add_message(&new_session_id, msg.clone())?;
    }

    log::info!(
        "Forked session '{}' -> '{}' with {} messages",
        source_session_id, new_session_id, copy_count
    );

    Ok(new_session_id)
}

/// Restore a checkpoint by iteration number.
#[tauri::command]
pub async fn restore_checkpoint(
    session_id: String,
    iteration: u32,
    project_path: Option<String>,
    store: State<'_, crate::agent::checkpoint::CheckpointStore>,
) -> Result<(), String> {
    let checkpoint = {
        let store = store.lock().unwrap_or_else(|e| e.into_inner());
        store.get(&session_id)
            .and_then(|cps| cps.iter().find(|cp| cp.iteration == iteration))
            .cloned()
            .ok_or_else(|| format!("No checkpoint found for iteration {}", iteration))?
    };

    let manager = crate::agent::checkpoint::CheckpointManager::new(project_path);
    manager.restore(&checkpoint).await
}

// ── Conversation Branching Commands ──────────────────────────────────────

fn get_session_storage() -> Result<crate::memory::session_store::SessionStorage, String> {
    let base_dir = directories::ProjectDirs::from("com", "neecoder", "NeeCoder")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .ok_or_else(|| "Failed to get project data dir".to_string())?;
    Ok(crate::memory::session_store::SessionStorage::new(&base_dir))
}

#[tauri::command]
pub async fn create_branch(
    session_id: String,
    from_seq: u32,
    branch_name: String,
) -> Result<String, String> {
    let storage = get_session_storage()?;
    storage.create_branch(&session_id, from_seq, &branch_name)
}

#[tauri::command]
pub async fn list_branches(
    session_id: String,
) -> Result<Vec<crate::memory::session_store::BranchInfo>, String> {
    let storage = get_session_storage()?;
    storage.list_branches(&session_id)
}

#[tauri::command]
pub async fn delete_branch(
    session_id: String,
    branch_id: String,
) -> Result<(), String> {
    let storage = get_session_storage()?;
    storage.delete_branch(&session_id, &branch_id)
}

