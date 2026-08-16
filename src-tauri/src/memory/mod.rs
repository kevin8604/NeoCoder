pub mod agent_log;
pub mod ebbinghaus;
pub mod finetune;
pub mod long_term;
pub mod notes;
pub mod preferences;
pub mod search;
pub mod session_store;
pub mod tools;

#[cfg(test)]
mod tests;

use crate::chat::{ChatMessage, ChatSession};
use std::path::PathBuf;
use std::sync::Mutex;

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
        let title = "Session".to_string(); // Will be renamed later
        let sessions = self
            .sessions
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        sessions.create_session(&id, &title)?;
        Ok(id)
    }

    pub fn add_message(&self, session_id: &str, msg: ChatMessage) -> Result<(), String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        let seq = sessions.next_sequence(session_id)?;
        sessions.save_message(session_id, &msg, seq)
    }

    pub fn get_context_window(
        &self,
        session_id: &str,
        max_tokens: usize,
    ) -> Result<Vec<ChatMessage>, String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        sessions.load_context_window(session_id, max_tokens)
    }

    pub fn clear_session(&self, session_id: &str) -> Result<(), String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        sessions.clear_messages(session_id)
    }

    pub fn delete_session(&self, session_id: &str) -> Result<(), String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        sessions.delete_session(session_id)
    }

    /// Clean up sessions older than `max_age_days`. Returns count of deleted.
    pub fn cleanup_expired_sessions(&self, max_age_days: u32) -> Result<usize, String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;
        sessions.cleanup_expired_sessions(max_age_days)
    }

    pub fn get_all_sessions(&self) -> Result<Vec<ChatSession>, String> {
        // Get session list and message dir paths under lock, then release
        let list: Vec<(String, String, String, std::path::PathBuf)> = {
            let sessions = self
                .sessions
                .lock()
                .map_err(|e| format!("Lock error: {}", e))?;
            let raw_list = sessions.list_sessions()?;
            raw_list
                .into_iter()
                .map(|(id, title, created_at)| {
                    let msg_dir = sessions.message_dir_path(&id);
                    (id, title, created_at, msg_dir)
                })
                .collect()
        }; // Lock released here

        // Count messages in parallel using std::thread::scope
        let results: Vec<(String, String, String, usize)> = std::thread::scope(|s| {
            let handles: Vec<_> = list
                .into_iter()
                .map(|(id, title, created_at, msg_dir)| {
                    s.spawn(move || {
                        let count = if msg_dir.exists() {
                            std::fs::read_dir(&msg_dir)
                                .map(|entries| {
                                    entries
                                        .filter_map(|e| e.ok())
                                        .filter(|e| {
                                            e.path().extension().and_then(|s| s.to_str())
                                                == Some("md")
                                        })
                                        .count()
                                })
                                .unwrap_or(0)
                        } else {
                            0
                        };
                        (id, title, created_at, count)
                    })
                })
                .collect();
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

    /// List daily note dates (newest first): (date, char_len, first_line).
    pub fn list_notes(&self) -> Result<Vec<(String, usize, String)>, String> {
        self.notes.list_notes()
    }

    /// Read a daily note for a specific date (YYYY-MM-DD).
    pub fn read_note(&self, date: &str) -> Result<String, String> {
        self.notes.read_note(date)
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
                let mut scored: Vec<(f64, usize)> = entries
                    .iter()
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
                let top: Vec<&ebbinghaus::MemoryEntry> = scored
                    .iter()
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
                    let recalled_indices: Vec<usize> = scored
                        .iter()
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

    pub fn search_memory(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<search::MemSearchResult>, String> {
        let results = self.memory_search.search(query, max_results)?;

        // Trigger Ebbinghaus recall for long-term memory hits
        if !results.is_empty()
            && let Ok(entries) = self.long_term.read_entries()
        {
            let mut recall_indices = Vec::new();
            for result in &results {
                if result.file_path == "MEMORY.md" {
                    // Find matching entry index
                    for (i, entry) in entries.iter().enumerate() {
                        if entry.text.contains(result.line_content.trim()) {
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

        Ok(results)
    }

    // ── Semantic search (embedding-based) ──

    /// Cosine similarity between two embedding vectors.
    fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        let mut dot = 0.0f32;
        let mut na = 0.0f32;
        let mut nb = 0.0f32;
        for (x, y) in a.iter().zip(b.iter()) {
            dot += x * y;
            na += x * x;
            nb += y * y;
        }
        if na == 0.0 || nb == 0.0 {
            0.0
        } else {
            dot / (na.sqrt() * nb.sqrt())
        }
    }

    /// Split text into overlapping-free character-sized chunks (line-aware).
    fn chunk_text(text: &str, max_chunk_len: usize) -> Vec<(usize, String)> {
        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut start_line = 1usize;
        for (i, line) in text.lines().enumerate() {
            if current.len() + line.len() + 1 > max_chunk_len && !current.is_empty() {
                chunks.push((start_line, std::mem::take(&mut current)));
                start_line = i + 1;
            }
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
        if !current.is_empty() {
            chunks.push((start_line, current));
        }
        chunks
    }

    /// Semantic search: embed query + memory chunks, rank by cosine similarity.
    ///
    /// Uses the LLM Router (local Ollama embedding model first, remote fallback).
    /// This solves the "search '性能优化' can't find 'performance'" problem that
    /// pure BM25 keyword matching suffers from. Falls back to BM25 on any error.
    pub async fn semantic_search_memory(
        &self,
        query: &str,
        settings: &crate::config::AppSettings,
        max_results: usize,
    ) -> Result<Vec<search::MemSearchResult>, String> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        // Collect corpus (reuse BM25 collection, skipping messages/)
        let mut docs: Vec<(String, String)> = Vec::new();
        self.memory_search
            .collect_docs(&self._base_dir, &mut docs)?;
        if docs.is_empty() {
            return Ok(Vec::new());
        }

        // Chunk each document (limit total chunks to bound embedding cost)
        const MAX_CHUNK_LEN: usize = 700;
        const MAX_TOTAL_CHUNKS: usize = 200;
        let mut chunks: Vec<(String, usize, String)> = Vec::new(); // (file, start_line, text)
        for (file, content) in docs.iter().take(80) {
            let body = if let Some(end) = content.find("\n---") {
                content[end + 4..].to_string()
            } else {
                content.clone()
            };
            for (start_line, chunk) in Self::chunk_text(&body, MAX_CHUNK_LEN) {
                chunks.push((file.clone(), start_line, chunk));
                if chunks.len() >= MAX_TOTAL_CHUNKS {
                    break;
                }
            }
            if chunks.len() >= MAX_TOTAL_CHUNKS {
                break;
            }
        }
        if chunks.is_empty() {
            return Ok(Vec::new());
        }

        // Route the embedding task (local Ollama first, remote fallback)
        let local_available =
            crate::llm::health::is_ollama_running(&settings.local_model.base_url).await;
        let route = crate::llm::LlmRouter::route(
            crate::llm::TaskType::Embedding,
            &settings.local_model,
            local_available,
            &settings.llm_provider,
            &settings.api_key,
            &settings.embedding_model,
        );

        // Embed query + all chunks in one request
        let mut texts: Vec<String> = Vec::with_capacity(chunks.len() + 1);
        texts.push(trimmed.to_string());
        texts.extend(chunks.iter().map(|(_, _, t)| t.clone()));

        let embeddings = crate::llm::embed_texts(
            &route.provider,
            &route.api_key,
            route.base_url.as_deref(),
            &route.model,
            &texts,
        )
        .await?;

        if embeddings.len() != texts.len() {
            return Err(format!(
                "Embedding count mismatch: got {}, expected {}",
                embeddings.len(),
                texts.len()
            ));
        }

        // Rank chunks by cosine similarity to the query
        let query_emb = &embeddings[0];
        let mut scored: Vec<(f32, usize)> = chunks
            .iter()
            .enumerate()
            .map(|(i, _)| (Self::cosine_sim(query_emb, &embeddings[i + 1]), i))
            .filter(|(s, _)| s.is_finite() && *s > 0.05)
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // Extract the best matching line from each top chunk
        let mut results = Vec::new();
        for (score, idx) in scored.iter().take(max_results) {
            let (file, start_line, chunk_text) = &chunks[*idx];
            // Find the line with the highest keyword overlap inside the chunk
            let query_tokens: Vec<String> = trimmed
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|s| s.len() >= 2)
                .map(|s| s.to_string())
                .collect();
            let mut best_line = chunk_text.lines().next().unwrap_or(chunk_text).to_string();
            let mut best_hits = 0usize;
            for (offset, line) in chunk_text.lines().enumerate() {
                let lower = line.to_lowercase();
                let hits = query_tokens
                    .iter()
                    .filter(|t| lower.contains(t.as_str()))
                    .count();
                if hits > best_hits {
                    best_hits = hits;
                    best_line = line.to_string();
                }
                if best_hits == query_tokens.len() {
                    break;
                }
                let _ = offset;
            }
            results.push(search::MemSearchResult {
                file_path: file.clone(),
                line_number: start_line
                    + chunk_text.lines().position(|l| l == best_line).unwrap_or(0),
                line_content: best_line,
                relevance: *score,
            });
        }

        Ok(results)
    }

    /// Hybrid search: merge BM25 + semantic results (deduplicated, re-ranked).
    pub async fn hybrid_search_memory(
        &self,
        query: &str,
        settings: &crate::config::AppSettings,
        max_results: usize,
    ) -> Result<Vec<search::MemSearchResult>, String> {
        let mut bm25 = self.search_memory(query, max_results)?;
        let semantic = match self
            .semantic_search_memory(query, settings, max_results)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                log::warn!("[MemorySearch] Semantic search failed, BM25 only: {}", e);
                vec![]
            }
        };

        // Merge: semantic results take precedence, then BM25; dedup by (file, line)
        let mut seen = std::collections::HashSet::new();
        let mut merged: Vec<search::MemSearchResult> = Vec::new();
        for r in semantic.into_iter().chain(bm25.drain(..)) {
            let key = format!("{}:{}", r.file_path, r.line_content.trim());
            if seen.insert(key) {
                merged.push(r);
            }
        }
        merged.truncate(max_results);
        Ok(merged)
    }

    // ── Dreaming: session-end memory consolidation ──

    /// P0: Quick pre-check — is this session about coding?
    /// Uses keyword-based detection (no extra LLM call).
    fn is_coding_session(session_messages: &[crate::chat::ChatMessage]) -> bool {
        let user_text: String = session_messages
            .iter()
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
    ///
    /// Uses the LLM Router: local Ollama model preferred (privacy + cost),
    /// automatic fallback to the remote provider when unavailable.
    pub async fn dreaming(
        &self,
        session_messages: &[crate::chat::ChatMessage],
        settings: &crate::config::AppSettings,
    ) {
        use std::sync::{Arc, Mutex};

        // Only dream if there are meaningful messages (at least 1 user + 1 assistant)
        let user_count = session_messages
            .iter()
            .filter(|m| matches!(m.role, crate::chat::Role::User))
            .count();
        let assistant_count = session_messages
            .iter()
            .filter(|m| matches!(m.role, crate::chat::Role::Assistant))
            .count();
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
                format!(
                    "{}...",
                    crate::agent::utils::safe_truncate(&msg.content, 500)
                )
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

        // ── LLM Router: local model preferred, remote fallback ──
        let local_available =
            crate::llm::health::is_ollama_running(&settings.local_model.base_url).await;
        let route = crate::llm::LlmRouter::route(
            crate::llm::TaskType::Dreaming,
            &settings.local_model,
            local_available,
            &settings.llm_provider,
            &settings.api_key,
            &settings.chat_model,
        );
        let route_used = format!("{:?}/{}", route.provider, route.model);

        let request = crate::llm::ChatRequestParams {
            model: route.model.clone(),
            messages: vec![crate::llm::ChatMessage {
                role: "user".into(),
                content: prompt,
                images: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            system: "You are a memory consolidation assistant for a coding agent.\n                Summarize CODING sessions only. IGNORE non-coding topics.\n                Output only the bullet points, no preamble.".into(),
            max_tokens: route.max_tokens,
            temperature: route.temperature,
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

        match crate::llm::stream_chat(
            &route.provider,
            &route.api_key,
            route.base_url.as_deref(),
            request,
            on_token,
            None,
        )
        .await
        {
            Ok(_) => {
                let summary = collected
                    .lock()
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                if !summary.is_empty() {
                    log::info!("[Dreaming] Summary generated via {}", route_used);
                    // Append to daily notes
                    let _ = self.append_note(&summary);

                    // Check if it contains persistent knowledge worth storing long-term
                    let has_persistent =
                        summary.contains("[Lesson]") || summary.contains("[Decision]");
                    if has_persistent {
                        let lessons: Vec<&str> = summary
                            .lines()
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
                log::info!(
                    "[Dreaming] Evicted {} entries to enforce capacity limit",
                    count
                );
            }
            _ => {}
        }
    }

    // ── Deep Dreaming: periodic global memory consolidation ──

    /// Deep Dreaming: read all daily notes + MEMORY.md, ask the LLM to merge
    /// duplicates, drop stale entries, and rewrite MEMORY.md in a compact form.
    /// Returns a human-readable report of what changed.
    pub async fn deep_dreaming(
        &self,
        settings: &crate::config::AppSettings,
    ) -> Result<String, String> {
        use std::sync::{Arc, Mutex};
        let entries = self.long_term.read_entries()?;
        if entries.is_empty() {
            return Ok("No long-term memory entries to consolidate.".to_string());
        }

        // Build compact entry list for the LLM
        let mut entry_text = String::new();
        for (i, e) in entries.iter().enumerate() {
            let text = crate::agent::utils::safe_truncate(&e.text, 300);
            entry_text.push_str(&format!("[{}] (category: {:?}) {}\n", i, e.category, text));
        }

        let prompt = format!(
            "You are consolidating a developer's long-term memory file.\n\n\
             Below are the current memory entries (index: text).\n\n\
             {}\n\n\
             Tasks:\n\
             1. Identify DUPLICATE or NEAR-DUPLICATE entries and merge them (report as 'merged: X -> Y').\n\
             2. Identify STALE/OUTDATED entries and mark them for removal (report as 'removed: X').\n\
             3. Keep everything else unchanged.\n\n\
             Output ONLY the deduplicated entry list, one entry per line, prefixed by the original index:\n\
             - 'KEEP <index>: <original text>' for entries to keep\n\
             - 'MERGE <index1>+<index2>: <merged text>' for merged entries\n\
             - 'DROP <index>' for entries to drop",
            entry_text
        );

        let local_available =
            crate::llm::health::is_ollama_running(&settings.local_model.base_url).await;
        let route = crate::llm::LlmRouter::route(
            crate::llm::TaskType::Dreaming,
            &settings.local_model,
            local_available,
            &settings.llm_provider,
            &settings.api_key,
            &settings.chat_model,
        );

        let request = crate::llm::ChatRequestParams {
            model: route.model.clone(),
            messages: vec![crate::llm::ChatMessage {
                role: "user".into(),
                content: prompt,
                images: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            system: "You are a memory consolidation assistant. Output only the structured lines, no preamble.".into(),
            max_tokens: 1024,
            temperature: 0.2,
            thinking_enabled: false,
            thinking_budget: 0,
        };

        let collected = Arc::new(Mutex::new(String::new()));
        let collected_clone = collected.clone();
        let on_token = move |token: String| {
            if let Ok(mut s) = collected_clone.lock() {
                s.push_str(&token);
            }
            Ok(())
        };

        crate::llm::stream_chat(
            &route.provider,
            &route.api_key,
            route.base_url.as_deref(),
            request,
            on_token,
            None,
        )
        .await
        .map_err(|e| format!("Deep dreaming LLM call failed: {}", e))?;

        let output = collected
            .lock()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if output.is_empty() {
            return Err("Deep dreaming returned empty output".to_string());
        }

        // Parse the LLM's decisions and apply them
        let mut kept: Vec<ebbinghaus::MemoryEntry> = Vec::new();
        let mut report = String::new();
        let mut removed_count = 0usize;
        let mut merged_count = 0usize;

        for line in output.lines() {
            let line = line.trim();
            if line.starts_with("KEEP ") {
                if let Some(rest) = line.strip_prefix("KEEP ")
                    && let Some((idx_str, _)) = rest.split_once(':')
                    && let Ok(idx) = idx_str.trim().parse::<usize>()
                    && idx < entries.len()
                    && !kept.iter().any(|e| e.text == entries[idx].text)
                {
                    kept.push(entries[idx].clone());
                }
            } else if line.starts_with("DROP") {
                removed_count += 1;
                report.push_str(&format!("{}\n", line));
            } else if line.starts_with("MERGE ") {
                merged_count += 1;
                if let Some(rest) = line.strip_prefix("MERGE ")
                    && let Some((idxs, merged_text)) = rest.split_once(':')
                {
                    // merged_text is the replacement — create a fresh entry with merged content
                    let text = merged_text.trim().to_string();
                    if !text.is_empty() {
                        let category = ebbinghaus::MemoryCategory::detect_from_text(&text);
                        kept.push(ebbinghaus::MemoryEntry::with_category(
                            text,
                            "Learned Patterns".to_string(),
                            category,
                        ));
                    }
                    report.push_str(&format!(
                        "MERGE (indices {}): {}\n",
                        idxs.trim(),
                        merged_text.trim()
                    ));
                }
            }
        }

        // Fallback: if LLM produced no KEEP lines, don't destroy existing memory
        if kept.is_empty() {
            log::warn!("[DeepDreaming] LLM output unparsable, keeping all entries");
            return Ok("Deep dreaming produced unparsable output; no changes made.".to_string());
        }

        self.long_term.write_entries(&kept)?;
        let report_out = format!(
            "Deep Dreaming complete: {} kept, {} merged, {} dropped.\n{}",
            kept.len(),
            merged_count,
            removed_count,
            report
        );
        log::info!("[DeepDreaming] {}", report_out);
        Ok(report_out)
    }

    // ── Memory GC: capacity control + expiration cleanup ──

    /// Run all garbage collection passes driven by `MemoryGCConfig`:
    /// 1. Long-term memory capacity enforcement (token budget → entry limit)
    /// 2. Expired long-term entries (Ebbinghaus decay)
    /// 3. Daily note files older than the retention window
    /// 4. Sessions older than the retention window
    ///
    /// Returns a JSON report of what was cleaned.
    pub fn run_gc(
        &self,
        settings: &crate::config::AppSettings,
    ) -> Result<serde_json::Value, String> {
        let cfg = &settings.memory_gc;

        // Token budget → entry limit (~40 tokens per entry on average)
        let max_entries = (cfg.max_memory_tokens / 40).max(20);
        let evicted = self.long_term.enforce_capacity(max_entries)?;

        let expired = self.long_term.cleanup_expired()?;
        let notes_deleted = self.notes.cleanup_expired(cfg.notes_retention_days)?;
        let sessions_deleted = self.cleanup_expired_sessions(cfg.session_retention_days)?;

        let report = serde_json::json!({
            "evicted_entries": evicted,
            "expired_entries": expired,
            "notes_deleted": notes_deleted,
            "sessions_deleted": sessions_deleted,
            "max_memory_tokens": cfg.max_memory_tokens,
            "notes_retention_days": cfg.notes_retention_days,
            "session_retention_days": cfg.session_retention_days,
        });
        log::info!("[MemoryGC] {:?}", report);
        Ok(report)
    }

    // ── Fine-tune data pipeline ──

    /// Export long-term memory entries as a JSONL training dataset
    /// (MEMORY.md → JSONL, ready for LoRA fine-tuning).
    ///
    /// Honors `FineTuneConfig.enabled`; manual triggers are still allowed via
    /// the dedicated command (the threshold is informational for auto-trigger).
    pub fn export_training_data(
        &self,
        settings: &crate::config::AppSettings,
    ) -> Result<String, String> {
        if !settings.fine_tune.enabled {
            return Err("Fine-tuning is disabled in Settings.".to_string());
        }
        let entries = self.long_term.read_entries()?;
        finetune::export_training_data(&self._base_dir, &entries, None)
    }

    // ── Memory stats ──

    /// Collect aggregate memory statistics for the frontend panel.
    pub fn get_memory_stats(
        &self,
        settings: &crate::config::AppSettings,
    ) -> Result<serde_json::Value, String> {
        let entries = self.long_term.read_entries()?;

        let mut category_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut total_chars = 0usize;
        let mut avg_stability = 0.0f64;
        for e in &entries {
            let cat = format!("{:?}", e.category);
            *category_counts.entry(cat).or_insert(0) += 1;
            total_chars += e.text.len();
            avg_stability += e.stability;
        }
        if !entries.is_empty() {
            avg_stability /= entries.len() as f64;
        }

        // Count daily notes files
        let notes_dir = self._base_dir.join("notes");
        let note_count = std::fs::read_dir(&notes_dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
                    .count()
            })
            .unwrap_or(0);

        Ok(serde_json::json!({
            "long_term_entries": entries.len(),
            "category_counts": category_counts,
            "total_chars": total_chars,
            "avg_stability": avg_stability.round() as u32,
            "notes_count": note_count,
            "max_memory_tokens": settings.memory_gc.max_memory_tokens,
            "notes_retention_days": settings.memory_gc.notes_retention_days,
            "semantic_search": settings.memory_gc.semantic_search,
        }))
    }
}
