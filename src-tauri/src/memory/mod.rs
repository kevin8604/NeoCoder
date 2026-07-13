pub mod session_store;
pub mod long_term;
pub mod notes;
pub mod search;
pub mod tools;
pub mod agent_log;
pub mod ebbinghaus;
pub mod preferences;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Mutex;
use crate::chat::{ChatMessage, ChatSession};

/// MemoryManager: unified interface for all memory operations.
/// Replaces the old ConversationMemory with a file-based Markdown system.
pub struct MemoryManager {
    pub(crate) _base_dir: PathBuf,
    sessions: Mutex<session_store::SessionStorage>,
    pub(crate) long_term: long_term::LongTermMemory,
    notes: notes::DailyNotes,
    memory_search: search::MemorySearch,
    /// User editing preferences tracker
    pub preferences: Mutex<preferences::UserPreferences>,
}

impl MemoryManager {
    pub fn new(base_dir: PathBuf) -> Self {
        // Ensure base directory exists
        let _ = std::fs::create_dir_all(&base_dir);
        let _ = std::fs::create_dir_all(base_dir.join("notes"));
        let _ = std::fs::create_dir_all(base_dir.join("sessions"));

        Self {
            sessions: Mutex::new(session_store::SessionStorage::new(&base_dir)),
            long_term: long_term::LongTermMemory::new(&base_dir),
            notes: notes::DailyNotes::new(&base_dir),
            memory_search: search::MemorySearch::new(&base_dir),
            preferences: Mutex::new(preferences::UserPreferences::load(&base_dir)),
            _base_dir: base_dir,
        }
    }

    // ── Session API (replaces ConversationMemory) ──

    pub fn create_session(&self) -> Result<String, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let title = format!("Session"); // Will be renamed later
        let sessions = self.sessions.lock().map_err(|e| format!("Lock error: {}", e))?;
        sessions.create_session(&id, &title)?;
        Ok(id)
    }

    pub fn add_message(&self, session_id: &str, msg: ChatMessage) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|e| format!("Lock error: {}", e))?;
        let seq = sessions.next_sequence(session_id)?;
        sessions.save_message(session_id, &msg, seq)
    }

    pub fn get_context_window(&self, session_id: &str, max_tokens: usize) -> Result<Vec<ChatMessage>, String> {
        let sessions = self.sessions.lock().map_err(|e| format!("Lock error: {}", e))?;
        sessions.load_context_window(session_id, max_tokens)
    }

    pub fn clear_session(&self, session_id: &str) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|e| format!("Lock error: {}", e))?;
        sessions.clear_messages(session_id)
    }

    pub fn delete_session(&self, session_id: &str) -> Result<(), String> {
        let sessions = self.sessions.lock().map_err(|e| format!("Lock error: {}", e))?;
        sessions.delete_session(session_id)
    }

    /// Clean up sessions older than `max_age_days`. Returns count of deleted.
    pub fn cleanup_expired_sessions(&self, max_age_days: u32) -> Result<usize, String> {
        let sessions = self.sessions.lock().map_err(|e| format!("Lock error: {}", e))?;
        sessions.cleanup_expired_sessions(max_age_days)
    }

    pub fn get_all_sessions(&self) -> Result<Vec<ChatSession>, String> {
        // Get session list and message dir paths under lock, then release
        let list: Vec<(String, String, String, std::path::PathBuf)> = {
            let sessions = self.sessions.lock().map_err(|e| format!("Lock error: {}", e))?;
            let raw_list = sessions.list_sessions()?;
            raw_list.into_iter().map(|(id, title, created_at)| {
                let msg_dir = sessions.message_dir_path(&id);
                (id, title, created_at, msg_dir)
            }).collect()
        }; // Lock released here

        // Count messages in parallel using std::thread::scope
        let results: Vec<(String, String, String, usize)> = std::thread::scope(|s| {
            let handles: Vec<_> = list.into_iter().map(|(id, title, created_at, msg_dir)| {
                s.spawn(move || {
                    let count = if msg_dir.exists() {
                        std::fs::read_dir(&msg_dir)
                            .map(|entries| entries
                                .filter_map(|e| e.ok())
                                .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
                                .count())
                            .unwrap_or(0)
                    } else {
                        0
                    };
                    (id, title, created_at, count)
                })
            }).collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });

        let mut result = Vec::new();
        for (id, title, created_at, msg_count) in results {
            result.push(ChatSession {
                id,
                title,
                messages: std::collections::VecDeque::new(),
                message_count: msg_count,
                created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
            });
        }
        Ok(result)
    }

    // ── Long-term memory ──

    pub fn read_long_term(&self) -> Result<String, String> {
        self.long_term.read()
    }

    pub fn write_long_term(&self, content: &str) -> Result<(), String> {
        self.long_term.write(content)
    }

    pub fn append_long_term(&self, section: &str, entry: &str) -> Result<(), String> {
        self.long_term.append(section, entry)
    }

    // ── Daily notes ──

    pub fn read_today_note(&self) -> Result<String, String> {
        self.notes.read_today()
    }

    pub fn append_note(&self, entry: &str) -> Result<(), String> {
        self.notes.append(entry)
    }

    // ── Memory context injection ──

    /// Returns formatted memory context string for injection into Agent system prompt.
    /// Uses Ebbinghaus retention scoring + category filtering + keyword relevance.
    pub fn inject_memory_context(&self) -> String {
        let mut ctx = String::new();
        let now = chrono::Utc::now().date_naive();

        // Long-term memory: R-value sorted, coding-only, relevance-filtered, top-N
        match self.long_term.read_entries() {
            Ok(entries) if !entries.is_empty() => {
                // Compute composite score: retention * (1.0 if coding else 0.0) + relevance bias
                let mut scored: Vec<(f64, usize)> = entries.iter()
                    .enumerate()
                    .map(|(i, e)| {
                        let retention = ebbinghaus::compute_retention(e, now);
                        // P2: Filter by category — only coding entries get full weight
                        let category_bonus = if e.category.is_coding() { 1.0 } else { 0.0 };
                        // P3: Keyword relevance bonus
                        let relevance = ebbinghaus::compute_coding_relevance(&e.text);
                        let score = retention * category_bonus + relevance * 0.3;
                        (score, i)
                    })
                    .collect();
                scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

                // Take top entries with score > 0.01 (max 20)
                const MAX_LT_ENTRIES: usize = 20;
                let top: Vec<&ebbinghaus::MemoryEntry> = scored.iter()
                    .filter(|(s, _)| *s > 0.01)
                    .take(MAX_LT_ENTRIES)
                    .map(|(_, i)| &entries[*i])
                    .collect();

                if !top.is_empty() {
                    ctx.push_str("## Long-term Memory (MEMORY.md)\n\n");
                    let mut current_section = String::new();
                    for entry in &top {
                        if entry.section != current_section {
                            ctx.push_str(&format!("### {}\n", entry.section));
                            current_section = entry.section.clone();
                        }
                        let r = ebbinghaus::compute_retention(entry, now);
                        let rel = ebbinghaus::compute_coding_relevance(&entry.text);
                        ctx.push_str(&format!("{} (R={:.2} rel={:.1})\n", entry.text, r, rel));
                    }
                    ctx.push('\n');

                    // Record recall for injected entries
                    let recalled_indices: Vec<usize> = scored.iter()
                        .filter(|(s, _)| *s > 0.01)
                        .take(MAX_LT_ENTRIES)
                        .map(|(_, i)| *i)
                        .collect();
                    if !recalled_indices.is_empty() {
                        let _ = self.long_term.recall_entries(&recalled_indices);
                    }
                }
            }
            _ => {}
        }

        // Today's notes
        match self.notes.read_today() {
            Ok(content) if !content.is_empty() => {
                ctx.push_str("\n## Today's Notes\n\n");
                ctx.push_str(&content);
                ctx.push('\n');
            }
            _ => {}
        }

        // Yesterday's notes
        match self.notes.read_yesterday() {
            Ok(content) if !content.is_empty() => {
                ctx.push_str("\n## Yesterday's Notes\n\n");
                ctx.push_str(&content);
                ctx.push('\n');
            }
            _ => {}
        }

        ctx
    }

    // ── Search ──

    pub fn search_memory(&self, query: &str, max_results: usize) -> Result<Vec<search::MemSearchResult>, String> {
        let results = self.memory_search.search(query, max_results)?;

        // Trigger Ebbinghaus recall for long-term memory hits
        if !results.is_empty() {
            if let Ok(entries) = self.long_term.read_entries() {
                let mut recall_indices = Vec::new();
                for result in &results {
                    if result.file_path == "MEMORY.md" {
                        // Find matching entry index
                        for (i, entry) in entries.iter().enumerate() {
                            if entry.text.contains(&result.line_content.trim()) {
                                if !recall_indices.contains(&i) {
                                    recall_indices.push(i);
                                }
                                break;
                            }
                        }
                    }
                }
                if !recall_indices.is_empty() {
                    let _ = self.long_term.recall_entries(&recall_indices);
                }
            }
        }

        Ok(results)
    }

    // ── Dreaming: session-end memory consolidation ──

    /// P0: Quick pre-check — is this session about coding?
    /// Uses keyword-based detection (no extra LLM call).
    fn is_coding_session(session_messages: &[crate::chat::ChatMessage]) -> bool {
        let user_text: String = session_messages.iter()
            .filter(|m| matches!(m.role, crate::chat::Role::User))
            .map(|m| m.content.as_str())
            .take(10) // Only first 10 user messages
            .collect::<Vec<_>>()
            .join(" ");

        if user_text.trim().is_empty() {
            return true; // Empty → assume coding (don't block by default)
        }

        let relevance = ebbinghaus::compute_coding_relevance(&user_text);
        relevance > 0.3 // Threshold: at least some coding keywords present
    }

    /// Dreaming: after a session ends, use LLM to summarize key learnings
    /// and write them to daily notes + long-term memory (MEMORY.md).
    /// This is fire-and-forget — errors are silently ignored.
    pub async fn dreaming(
        &self,
        session_messages: &[crate::chat::ChatMessage],
        provider: &crate::config::LlmProvider,
        api_key: &str,
        base_url: Option<&str>,
        chat_model: &str,
    ) {
        use std::sync::{Arc, Mutex};

        // Only dream if there are meaningful messages (at least 1 user + 1 assistant)
        let user_count = session_messages.iter().filter(|m| matches!(m.role, crate::chat::Role::User)).count();
        let assistant_count = session_messages.iter().filter(|m| matches!(m.role, crate::chat::Role::Assistant)).count();
        if user_count == 0 || assistant_count == 0 {
            return;
        }

        // P0: Pre-check — skip dreaming for non-coding sessions (keyword-based, no extra LLM call)
        if !Self::is_coding_session(session_messages) {
            log::debug!("[Dreaming] Skipping — non-coding session detected");
            return;
        }

        // Build summary of the session (truncate to avoid huge prompts)
        let mut conversation = String::new();
        for msg in session_messages.iter().take(20) {
            let role = match msg.role {
                crate::chat::Role::User => "User",
                crate::chat::Role::Assistant => "Assistant",
                _ => continue,
            };
            let content = if msg.content.len() > 500 {
                format!("{}...", crate::agent::utils::safe_truncate(&msg.content, 500))
            } else {
                msg.content.clone()
            };
            conversation.push_str(&format!("{}: {}\n\n", role, content));
        }

        // P1: Domain-constrained dreaming prompt — only coding topics
        let prompt = format!(
            "Summarize this CODING session in 3-5 concise bullet points.\n\
             ONLY include topics related to: programming languages, frameworks, tools,\n\
             debugging techniques, architecture patterns, engineering practices.\n\
             IGNORE any non-coding topics (writing essays, general conversation, etc.).\n\n\
             Focus on:\n\
             1. What was the user's coding goal?\n\
             2. What key technical decisions were made?\n\
             3. What errors were encountered and how were they resolved?\n\
             4. What reusable patterns or lessons learned should be remembered?\n\n\
             Output format (keep it short):\n\
             - [Goal]: <one line>\n\
             - [Decision]: <key technical choice>\n\
             - [Lesson]: <reusable pattern or pitfall to avoid>\n\n\
             Conversation:\n{}",
            conversation
        );

        let request = crate::llm::ChatRequestParams {
            model: chat_model.to_string(),
            messages: vec![crate::llm::ChatMessage {
                role: "user".into(),
                content: prompt,
                images: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            system: "You are a memory consolidation assistant for a coding agent.\n                Summarize CODING sessions only. IGNORE non-coding topics.\n                Output only the bullet points, no preamble.".into(),
            max_tokens: 300,
            temperature: 0.3,
            thinking_enabled: false,
            thinking_budget: 0,
        };

        // Use stream_chat and collect tokens
        let collected = Arc::new(Mutex::new(String::new()));
        let collected_clone = collected.clone();
        let on_token = move |token: String| {
            if let Ok(mut s) = collected_clone.lock() {
                s.push_str(&token);
            }
            Ok(())
        };

        match crate::llm::stream_chat(provider, api_key, base_url, request, on_token, None).await {
            Ok(_) => {
                let summary = collected.lock().map(|s| s.trim().to_string()).unwrap_or_default();
                if !summary.is_empty() {
                    // Append to daily notes
                    let _ = self.append_note(&summary);

                    // Check if it contains persistent knowledge worth storing long-term
                    let has_persistent = summary.contains("[Lesson]") || summary.contains("[Decision]");
                    if has_persistent {
                        let lessons: Vec<&str> = summary.lines()
                            .filter(|l| l.contains("[Lesson]") || l.contains("[Decision]"))
                            .collect();
                        if !lessons.is_empty() {
                            let entry = lessons.join("\n");
                            let _ = self.append_long_term("Learned Patterns", &entry);
                        }
                    }
                }
            }
            Err(e) => {
                log::debug!("[Dreaming] LLM call failed (non-critical): {}", e);
            }
        }

        // Ebbinghaus cleanup: archive expired long-term memory entries
        match self.long_term.cleanup_expired() {
            Ok(count) if count > 0 => {
                log::info!("[Dreaming] Cleaned up {} expired memory entries", count);
            }
            _ => {}
        }

        // Capacity enforcement: evict lowest-retention entries if over limit
        match self.long_term.enforce_capacity(50) {
            Ok(count) if count > 0 => {
                log::info!("[Dreaming] Evicted {} entries to enforce capacity limit", count);
            }
            _ => {}
        }
    }
}
