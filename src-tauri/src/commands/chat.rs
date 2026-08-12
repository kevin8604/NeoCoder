use tauri::{Emitter, State, Manager};
use crate::agent;
use crate::agent::definition::AgentRegistry;
use crate::agent::QuestionAwaiters;
use crate::agent::ConfirmAwaiters;
use crate::agent::PlanAction;
use crate::agent::PlanApprovalAwaiters;
use crate::agent::PauseControl;
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
    /// 会话级 Plan 模式开关：`set_plan_mode` 写入，`send_message` 未显式传
    /// plan_mode 参数时回退查询（显式参数优先）。
    pub plan_mode_sessions: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
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

/// Parse a context file reference. Supports the `path:line` form (e.g.
/// `src/main.rs:42`) used by `@file:line` mentions. Windows drive-letter
/// colons are handled safely because the suffix after the last colon must
/// be a positive integer. Returns (path, optional 1-based line number).
fn parse_context_file_ref(raw: &str) -> (&str, Option<usize>) {
    if let Some(idx) = raw.rfind(':') {
        let line_part = &raw[idx + 1..];
        if !line_part.is_empty() && line_part.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(line) = line_part.parse::<usize>() {
                // Numeric suffix — treat as a line reference (0 → whole file)
                return if line >= 1 {
                    (&raw[..idx], Some(line))
                } else {
                    (&raw[..idx], None)
                };
            }
        }
    }
    (raw, None)
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

    // Plan 模式：显式传参优先，否则回退到会话级开关（set_plan_mode 写入）
    let plan_mode = plan_mode.or_else(|| {
        chat_state
            .plan_mode_sessions
            .lock()
            .ok()
            .map(|s| s.contains(&session_id))
            .filter(|&enabled| enabled)
    });

    // Build messages array for LLM API
    let mut messages: Vec<LlmMessage> = vec![];

    // Inject context files as system messages (from @ file mentions) — parallel read
    // `@file:line` references (path:line) inject only the referenced line and its
    // surroundings instead of the whole file
    if let Some(ref files) = context_files {
        if !files.is_empty() {
            const LINE_REFERENCE_RADIUS: usize = 20;
            let read_futures = files.iter().map(|fp| {
                let (path, line) = parse_context_file_ref(fp);
                let path = path.to_string();
                async move {
                    match tokio::fs::read_to_string(&path).await {
                        Ok(content) => Some((fp.clone(), content, line)),
                        Err(e) => {
                            log::warn!("[Chat] Failed to read context file {}: {}", fp, e);
                            None
                        }
                    }
                }
            });
            let results = futures_util::future::join_all(read_futures).await;
            for result in results.into_iter().flatten() {
                let (file_ref, content, line) = result;
                let truncated = if content.len() > 50_000 {
                    // Use char-aware truncation to avoid UTF-8 boundary panics
                    let truncated: String = content.chars().take(50_000).collect();
                    format!("{}... [truncated at 50KB]", truncated)
                } else {
                    content
                };
                let injected = if let Some(line) = line {
                    // Extract only the referenced line and its surroundings
                    let lines: Vec<&str> = truncated.lines().collect();
                    if lines.is_empty() || line > lines.len() {
                        // Line out of range — fall back to the whole (truncated) file
                        truncated
                    } else {
                        let start = line.saturating_sub(LINE_REFERENCE_RADIUS).max(1);
                        let end = (line + LINE_REFERENCE_RADIUS).min(lines.len());
                        format!(
                            "(line-reference excerpt: lines {}-{})\n{}",
                            start,
                            end,
                            lines[start - 1..end].join("\n")
                        )
                    }
                } else {
                    truncated
                };
                messages.push(LlmMessage {
                    role: "system".into(),
                    content: format!("File: {}\n```\n{}\n```", file_ref, injected),
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

    // ── @codebase / #codebase RAG 注入（Ask / Edit / Agent 全模式）──
    // 前端 @codebase 提及触发：检索整个代码库并注入最相关片段。
    // 放在 is_agent 块之前，使 Ask/Edit 模式也获得同样的 RAG 上下文。
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

    // ── Extract memory context & project instructions (shared by all modes) ──
    let memory_context = memory.memory_manager().inject_memory_context();

    // ── Ask 模式记忆 RAG：语义检索与当前问题最相关的记忆条目 ──
    // 与整体 MEMORY.md 注入互补：MEMORY.md 提供全局上下文，这里提供
    // 与问题高度相关的命中片段（BM25 或混合检索，自动降级）。
    let ask_memory_rag: Option<String> = if matches!(mode, ChatMode::Ask) {
        let mgr = memory.memory_manager();
        let want_semantic = settings.memory_gc.semantic_search;
        let results = if want_semantic {
            mgr.hybrid_search_memory(&message, &settings, 5).await
                .or_else(|_| mgr.search_memory(&message, 5))
                .ok()
        } else {
            mgr.search_memory(&message, 5).ok()
        };
        results.map(|rs| {
            rs.iter()
                .map(|r| format!("- {}:{} — {}", r.file_path, r.line_number, r.line_content.trim()))
                .collect::<Vec<_>>()
                .join("\n")
        }).filter(|s| !s.is_empty())
    } else {
        None
    };
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
        let agent_memory_context = memory.memory_manager().inject_memory_context();
        drop(memory);

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

        // ── Shared agent spawn pipeline (cancel/pause registration, execution,
        // result persistence, dreaming) — also used by resume_agent ──
        let settings_snapshot = settings.clone();
        return spawn_agent_pipeline(
            app,
            &chat_state,
            session_id,
            messages,
            agent_id,
            effective_project_path,
            plan_mode,
            settings_snapshot,
            agent_memory_context,
        )
        .await;
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
    if let Some(ref rag) = ask_memory_rag {
        system_prompt.push_str("\n\n## Relevant Memory (retrieved for this question)\n\n");
        system_prompt.push_str(rag);
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

/// Shared agent spawn pipeline used by `send_message` (new tasks) and
/// `resume_agent` (recovered tasks). Registers cancel/pause controls, runs
/// the agent in a panic-safe nested task, persists the final result, flushes
/// a session note and fires dreaming.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn spawn_agent_pipeline(
    app: tauri::AppHandle,
    chat_state: &ChatState,
    session_id: String,
    messages: Vec<LlmMessage>,
    agent_id: Option<String>,
    project_path: Option<String>,
    plan_mode: Option<bool>,
    settings: AppSettings,
    agent_memory_context: String,
) -> Result<String, String> {
    let provider = settings.llm_provider.clone();
    let api_key = settings.api_key.clone();
    let chat_model = settings.chat_model.clone();

    // Determine which agent to use
    let effective_agent_id = agent_id.clone().unwrap_or_else(|| "orchestrator".to_string());

    // Look up agent definition
    let agent_def: Option<crate::agent::definition::AgentDefinition> = app.try_state::<AgentRegistry>()
        .and_then(|registry| crate::agent::definition::find_agent(registry.inner(), &effective_agent_id));

    // Load custom instructions from settings + .neecoder/instructions.md
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

    // Register Agent cancellation flag
    let cancel_flag = Arc::new(AtomicBool::new(false));
    if let Some(cancel_map) = app.try_state::<AgentCancelMap>() {
        if let Ok(mut map) = cancel_map.lock() {
            map.insert(session_id.clone(), cancel_flag.clone());
        }
    }

    // Register Agent pause control (flag + notify) for this session
    let pause_flag = Arc::new(AtomicBool::new(false));
    let pause_notify = Arc::new(tokio::sync::Notify::new());
    if let Some(pc) = app.try_state::<PauseControl>() {
        if let Ok(mut map) = pc.lock() {
            map.insert(session_id.clone(), (pause_flag.clone(), pause_notify.clone()));
        }
    }

    let agent_def_cloned = agent_def.clone();
    let session_id_for_cleanup = session_id.clone();
    let memory_arc = chat_state.memory.clone();
    let memory_manager_for_flush = chat_state.memory.read().await.memory_manager();
    // Snapshot full settings for the post-run dreaming task (fire-and-forget)
    let settings_for_dreaming = settings.clone();

    let is_plan_mode = plan_mode.unwrap_or(false);

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

                // Cleanup pause control
                if let Some(pc) = app.try_state::<PauseControl>() {
                    if let Ok(mut map) = pc.lock() {
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

        // Cleanup pause control
        if let Some(pc) = app.try_state::<PauseControl>() {
            if let Ok(mut map) = pc.lock() {
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
        // Routes through the LLM Router: local Ollama first (privacy + cost), remote fallback.
        let memory_mgr = memory_manager_for_flush.clone();
        tokio::spawn(async move {
            memory_mgr.dreaming(&session_msgs, &settings_for_dreaming).await;
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

    Ok("Agent started".to_string())
}

/// Resume an interrupted agent task from its persisted JSONL log.
///
/// Rebuilds the LLM message history (user/assistant/tool pairs), injects a
/// "[SESSION_RESUMED]" instruction, and re-runs the agent with the same
/// session id so cancel/pause/events keep working. Returns an error when the
/// session has no agent log or the task already completed.
#[tauri::command]
pub async fn resume_session(
    app: tauri::AppHandle,
    session_id: String,
    project_path: Option<String>,
    followup: Option<String>,
    chat_state: State<'_, ChatState>,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
) -> Result<String, String> {
    // Locate the agent log
    let config_dir = app.path().app_config_dir().map_err(|e| format!("App config dir unavailable: {}", e))?;
    let log_path = config_dir.join("sessions").join("agent_logs").join(format!("{}.jsonl", session_id));
    if !log_path.exists() {
        return Err(format!("No agent task found for session '{}'", session_id));
    }

    // Replay the log and verify the task did not already complete
    let entries = crate::memory::agent_log::AgentLog::replay(&log_path).await?;
    if entries.is_empty() {
        return Err(format!("Agent log for session '{}' is empty", session_id));
    }
    if let Some(last) = entries.last() {
        if matches!(last.entry_type, crate::memory::agent_log::LogEntryType::Completed { .. }) {
            return Err("The agent task for this session already completed".to_string());
        }
    }

    // Rebuild LLM messages from the log (tool results paired to their calls)
    let mut messages = crate::memory::agent_log::AgentLog::to_messages(&entries);

    // Find the agent that ran this task (from the log), default to orchestrator
    let log_agent_id = entries.iter().find_map(|e| {
        if e.agent_id.is_empty() || e.agent_id == "agent" {
            None
        } else {
            Some(e.agent_id.clone())
        }
    }).unwrap_or_else(|| "orchestrator".to_string());

    // Inject the resume instruction so the model continues, not restarts
    let followup_hint = match followup {
        Some(f) if !f.trim().is_empty() => format!("\nUser additionally asks: {}", f.trim()),
        _ => String::new(),
    };
    messages.push(LlmMessage {
        role: "system".into(),
        content: format!(
            "[SESSION_RESUMED] The application restarted while this task was in progress. \
             The complete task history is above. Review the current state of the workspace \
             (tool results show what was done; files may have changed since) and CONTINUE the \
             task to completion. Do not repeat completed steps; finish what remains.{}",
            followup_hint
        ),
        images: None,
        tool_calls: None,
        tool_call_id: None,
    });

    // Snapshot settings for the pipeline
    let settings = settings.read().await;
    let settings_snapshot = settings.clone();
    drop(settings);

    let memory = chat_state.memory.read().await;
    let agent_memory_context = memory.memory_manager().inject_memory_context();
    drop(memory);

    spawn_agent_pipeline(
        app,
        &chat_state,
        session_id,
        messages,
        Some(log_agent_id),
        project_path,
        None,
        settings_snapshot,
        agent_memory_context,
    )
    .await
}

/// List agent sessions whose tasks were interrupted (no Completed entry) and
/// can be resumed after a restart. Returns session id, agent id and the last
/// activity timestamp of each resumable task.
#[tauri::command]
pub async fn list_resumable_sessions(
    app: tauri::AppHandle,
) -> Result<Vec<serde_json::Value>, String> {
    let config_dir = app.path().app_config_dir().map_err(|e| format!("App config dir unavailable: {}", e))?;
    let logs_dir = config_dir.join("sessions").join("agent_logs");
    let mut resumable = Vec::new();

    if !logs_dir.is_dir() {
        return Ok(resumable);
    }

    let mut files: Vec<_> = std::fs::read_dir(&logs_dir)
        .map_err(|e| format!("Failed to read agent logs: {}", e))?
        .flatten()
        .collect();
    files.sort_by_key(|e| e.file_name());

    for entry in files {
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };

        let mut last_entry: Option<crate::memory::agent_log::LogEntry> = None;
        let mut agent_id = String::new();
        for line in content.lines().filter(|l| !l.trim().is_empty()) {
            if let Ok(entry) = serde_json::from_str::<crate::memory::agent_log::LogEntry>(line) {
                agent_id = entry.agent_id.clone();
                last_entry = Some(entry);
            }
        }

        let Some(last) = last_entry else { continue };
        if matches!(last.entry_type, crate::memory::agent_log::LogEntryType::Completed { .. }) {
            continue;
        }

        let session_id = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if session_id.is_empty() {
            continue;
        }

        resumable.push(serde_json::json!({
            "session_id": session_id,
            "agent_id": agent_id,
            "last_timestamp": last.timestamp,
        }));
    }

    Ok(resumable)
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

/// Pause a running agent. Sets the session-scoped pause flag; the agent main
/// loop parks until `resume_agent` fires the notify. Idempotent.
#[tauri::command]
pub async fn pause_agent(
    app: tauri::AppHandle,
    session_id: String,
    pause_control: State<'_, PauseControl>,
) -> Result<(), String> {
    if let Ok(map) = pause_control.lock() {
        if let Some((flag, _notify)) = map.get(&session_id) {
            flag.store(true, Ordering::SeqCst);
            let _ = app.emit(
                "chat-event",
                ChatEvent::AgentStatus {
                    session_id: session_id.clone(),
                    agent_id: None,
                    status: "pause_requested".into(),
                    iteration: 0,
                    total_iterations: 0,
                    estimated_tokens: 0,
                    elapsed_ms: 0,
                },
            );
        }
    }
    Ok(())
}

/// Resume a paused agent: clears the flag and fires the notify so the main
/// loop can continue. Idempotent.
#[tauri::command]
pub async fn resume_agent(
    app: tauri::AppHandle,
    session_id: String,
    pause_control: State<'_, PauseControl>,
) -> Result<(), String> {
    if let Ok(map) = pause_control.lock() {
        if let Some((flag, notify)) = map.get(&session_id) {
            flag.store(false, Ordering::SeqCst);
            notify.notify_one();
            let _ = app.emit(
                "chat-event",
                ChatEvent::AgentStatus {
                    session_id: session_id.clone(),
                    agent_id: None,
                    status: "resumed".into(),
                    iteration: 0,
                    total_iterations: 0,
                    estimated_tokens: 0,
                    elapsed_ms: 0,
                },
            );
        }
    }
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

/// ── Plan Mode Approval Commands ──

/// Send approval (session_id from PlanCreated event) to the waiting Agent.
#[tauri::command]
pub async fn approve_plan(
    session_id: String,
    awaiters: State<'_, PlanApprovalAwaiters>,
) -> Result<(), String> {
    let mut map = awaiters.lock().map_err(|e| format!("Lock error: {}", e))?;
    if let Some(sender) = map.remove(&session_id) {
        let _ = sender.send(PlanAction::Approve);
    }
    Ok(())
}

/// Reject a plan with an optional reason.
#[tauri::command]
pub async fn reject_plan(
    session_id: String,
    reason: Option<String>,
    awaiters: State<'_, PlanApprovalAwaiters>,
) -> Result<(), String> {
    let mut map = awaiters.lock().map_err(|e| format!("Lock error: {}", e))?;
    if let Some(sender) = map.remove(&session_id) {
        let _ = sender.send(PlanAction::Reject(reason.unwrap_or_default()));
    }
    Ok(())
}

/// Skip plan — go straight to execution phase.
#[tauri::command]
pub async fn skip_plan(
    session_id: String,
    awaiters: State<'_, PlanApprovalAwaiters>,
) -> Result<(), String> {
    let mut map = awaiters.lock().map_err(|e| format!("Lock error: {}", e))?;
    if let Some(sender) = map.remove(&session_id) {
        let _ = sender.send(PlanAction::Skip);
    }
    Ok(())
}

#[tauri::command]
pub fn get_terminal_history(count: Option<usize>) -> Vec<String> {
    let n = count.unwrap_or(3);
    let entries = crate::terminal::get_recent_terminal(n);
    entries.into_iter().map(|(cmd, output, exit)| {
        format!("$ {}\n{}\nExit: {}", cmd, output, exit)
    }).collect()
}

#[tauri::command]
pub fn get_error_summary() -> String {
    crate::terminal::get_error_summary()
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
/// Persists per-session; `send_message` falls back to this when it is not
/// called with an explicit `plan_mode` argument.
#[tauri::command]
pub async fn set_plan_mode(
    session_id: String,
    enabled: bool,
    chat_state: State<'_, ChatState>,
) -> Result<bool, String> {
    let mut sessions = chat_state
        .plan_mode_sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if enabled {
        sessions.insert(session_id.clone());
    } else {
        sessions.remove(&session_id);
    }
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

/// Get the structured diff introduced by a checkpoint's git commit.
///
/// Uses `git diff <hash>^ <hash> -- <files>`; falls back to `git show <hash>`
/// when the checkpoint commit has no parent (e.g. it is the repository's first
/// commit). Returns an empty list when the checkpoint produced no changes.
#[tauri::command]
pub async fn checkpoint_diff(
    session_id: String,
    iteration: u32,
    project_path: Option<String>,
    store: State<'_, crate::agent::checkpoint::CheckpointStore>,
) -> Result<Vec<crate::chat::FileChange>, String> {
    let work_dir = project_path
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| "No project path provided".to_string())?;
    let checkpoint = {
        let store = store.lock().unwrap_or_else(|e| e.into_inner());
        store.get(&session_id)
            .and_then(|cps| cps.iter().find(|cp| cp.iteration == iteration))
            .cloned()
            .ok_or_else(|| format!("No checkpoint found for iteration {}", iteration))?
    };
    let hash = checkpoint.commit_hash.as_ref()
        .ok_or_else(|| "Checkpoint has no commit hash (project is not a git repository)".to_string())?;

    let files: Vec<&str> = checkpoint.files.iter().map(|s| s.as_str()).collect();
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("diff")
        .arg(format!("{}^", hash))
        .arg(hash)
        .current_dir(&work_dir);
    if !files.is_empty() {
        cmd.arg("--").args(&files);
    }
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        cmd.output(),
    ).await;

    let raw = match output {
        Ok(Ok(out)) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        _ => {
            // Fallback: initial commit (no parent) — diff against the empty tree
            let output = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                tokio::process::Command::new("git")
                    .arg("show")
                    .arg(hash)
                    .arg("--format=")
                    .current_dir(&work_dir)
                    .output(),
            ).await;
            match output {
                Ok(Ok(out)) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
                Ok(Ok(out)) => {
                    return Err(format!("git diff failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
                }
                Ok(Err(e)) => return Err(format!("Failed to execute git: {}", e)),
                Err(_) => return Err("git diff timed out".to_string()),
            }
        }
    };

    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(parse_unified_diff(&raw))
}

/// Parse git unified diff text into structured per-file changes.
///
/// Recognises `diff --git` file headers, `@@` hunk headers (kept as context
/// rows for line-number display), and ` ` (context) / `+` (add) / `-` (remove)
/// content lines. Binary/header noise (index, ---, +++, new/deleted file
/// markers, `\ No newline`) is skipped.
fn parse_unified_diff(raw: &str) -> Vec<crate::chat::FileChange> {
    let mut changes: Vec<crate::chat::FileChange> = Vec::new();
    let mut current: Option<(String, Vec<crate::chat::DiffHunk>)> = None;
    let mut old_line = 0u32;
    let mut new_line = 0u32;

    for line in raw.lines() {
        if let Some(header) = line.strip_prefix("diff --git ") {
            // New file section: "a/path b/path" — keep the b/ (new) path
            let path = header.rsplit(" b/").next().unwrap_or(header).trim();
            if let Some((path, hunks)) = current.take() {
                changes.push(crate::chat::FileChange { file_path: path, hunks });
            }
            current = Some((path.to_string(), Vec::new()));
            old_line = 0;
            new_line = 0;
        } else if line.starts_with("@@") {
            // Hunk header: @@ -old_start[,count] +new_start[,count] @@
            if let Some((_, hunks)) = &mut current {
                hunks.push(crate::chat::DiffHunk {
                    hunk_type: "hunk".into(),
                    content: line.to_string(),
                    old_start: old_line,
                    new_start: new_line,
                });
            }
            let mut it = line.split_whitespace();
            it.next(); // @@
            let old_part = it.next().unwrap_or("-0");
            let new_part = it.next().unwrap_or("+0");
            old_line = old_part.trim_start_matches('-').split(',').next()
                .and_then(|s| s.parse().ok()).unwrap_or(0);
            new_line = new_part.trim_start_matches('+').split(',').next()
                .and_then(|s| s.parse().ok()).unwrap_or(0);
        } else if line.starts_with("index ")
            || line.starts_with("---")
            || line.starts_with("+++")
            || line.starts_with("new file mode")
            || line.starts_with("deleted file mode")
            || line == r"\ No newline at end of file"
        {
            continue;
        } else if let Some((_, hunks)) = &mut current {
            let hunk_type = if line.starts_with('+') {
                "add"
            } else if line.starts_with('-') {
                "remove"
            } else {
                "context"
            };
            let content = line.trim_start_matches(['+', '-', ' ']).to_string();
            hunks.push(crate::chat::DiffHunk {
                hunk_type: hunk_type.into(),
                content,
                old_start: old_line,
                new_start: new_line,
            });
            match hunk_type {
                "add" => new_line += 1,
                "remove" => old_line += 1,
                _ => {
                    old_line += 1;
                    new_line += 1;
                }
            }
        }
    }
    if let Some((path, hunks)) = current.take() {
        changes.push(crate::chat::FileChange { file_path: path, hunks });
    }
    changes
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

#[cfg(test)]
mod tests {
    use super::parse_context_file_ref;

    #[test]
    fn test_parse_unified_diff_basic() {
        let raw = r#"diff --git a/src/main.rs b/src/main.rs
index 123..456 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!("old");
+    println!("new");
     println!("both");
 }
diff --git a/README.md b/README.md
new file mode 100644
--- /dev/null
+++ b/README.md
@@ -0,0 +1,2 @@
+# Title
+Body
"#;
        let changes = super::parse_unified_diff(raw);
        assert_eq!(changes.len(), 2);

        // 第一个文件：路径取自 b/ 侧，含 add/remove/context 行
        let main = &changes[0];
        assert_eq!(main.file_path, "src/main.rs");
        let types: Vec<&str> = main.hunks.iter().map(|h| h.hunk_type.as_str()).collect();
        assert_eq!(types, vec!["hunk", "context", "remove", "add", "context", "context"]);
        assert_eq!(main.hunks[2].content, "println!(\"old\");");
        assert_eq!(main.hunks[3].content, "println!(\"new\");");
        // 行号跟踪：context 后 old/new 同步前进，remove 后 new_start 不前进
        assert_eq!(main.hunks[2].old_start, 2);
        assert_eq!(main.hunks[3].new_start, 2);
        assert_eq!(main.hunks[3].old_start, 3); // remove 已消耗旧文件行 2

        // 第二个文件：新增文件
        let readme = &changes[1];
        assert_eq!(readme.file_path, "README.md");
        assert!(readme.hunks.iter().all(|h| h.hunk_type == "add" || h.hunk_type == "hunk"));
    }

    #[test]
    fn test_parse_unified_diff_empty() {
        assert!(super::parse_unified_diff("").is_empty());
        // 无 diff --git 头的垃圾文本 → 无文件
        assert!(super::parse_unified_diff("hello world\n").is_empty());
    }

    #[test]
    fn test_parse_context_file_ref_plain_path() {
        // No colon / no numeric suffix → no line reference
        assert_eq!(parse_context_file_ref("src/main.rs"), ("src/main.rs", None));
    }

    #[test]
    fn test_parse_context_file_ref_with_line() {
        // `path:line` form → path + line
        assert_eq!(parse_context_file_ref("src/main.rs:42"), ("src/main.rs", Some(42)));
        assert_eq!(parse_context_file_ref("main.rs:1"), ("main.rs", Some(1)));
        assert_eq!(parse_context_file_ref("main.rs:0"), ("main.rs", None));
    }

    #[test]
    fn test_parse_context_file_ref_windows_drive() {
        // Windows drive-letter colons must not be mistaken for line refs
        assert_eq!(
            parse_context_file_ref("C:\\workspace\\file.rs"),
            ("C:\\workspace\\file.rs", None)
        );
        // Windows absolute path + line ref still works (last colon wins)
        assert_eq!(
            parse_context_file_ref("C:\\workspace\\file.rs:42"),
            ("C:\\workspace\\file.rs", Some(42))
        );
    }

    #[test]
    fn test_parse_context_file_ref_non_numeric_suffix() {
        // A colon not followed by digits is not a line ref (e.g. URLs, labels)
        assert_eq!(parse_context_file_ref("docs/README.md:section"), ("docs/README.md:section", None));
        assert_eq!(parse_context_file_ref("file.rs:"), ("file.rs:", None));
    }
}


