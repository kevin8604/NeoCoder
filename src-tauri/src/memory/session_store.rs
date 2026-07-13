use std::path::{Path, PathBuf};
use std::fs;
use crate::chat::{ChatMessage, Role};

// ── SessionStore trait ──────────────────────────────────────────────────────

/// Abstract session storage trait for pluggable backends.
/// Currently implemented by file-system Markdown storage;
/// future implementations could target SQLite, Redis, etc.
pub trait SessionStore: Send + Sync {
    /// Create a new session with the given id and title.
    fn create_session(&self, id: &str, title: &str) -> Result<(), String>;
    /// Save a message to the session.
    fn save_message(&self, session_id: &str, msg: &ChatMessage, seq: u32) -> Result<(), String>;
    /// Load all messages for a session (ordered by sequence).
    fn load_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>, String>;
    /// Delete a session and all its data.
    fn delete_session(&self, session_id: &str) -> Result<(), String>;
    /// Remove expired sessions older than `max_age_days`. Returns count of deleted sessions.
    fn cleanup_expired(&self, max_age_days: u32) -> Result<usize, String>;
}

// ── Session meta ────────────────────────────────────────────────────────────
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SessionMeta {
    id: String,
    title: String,
    created_at: String,
}

/// Branch information returned by list_branches.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BranchInfo {
    pub id: String,
    pub name: String,
    pub message_count: u32,
    pub is_active: bool,
}

/// Branch map type: maps branch_id to list of message seq numbers
type BranchMap = std::collections::HashMap<String, Vec<u32>>;

/// Markdown-based session message storage.
/// Each session = a directory with session.md + messages/*.md files with YAML frontmatter.
pub struct SessionStorage {
    sessions_dir: PathBuf,
}

impl SessionStorage {
    pub fn new(base_dir: &Path) -> Self {
        Self {
            sessions_dir: base_dir.join("sessions"),
        }
    }

    /// Create a session directory with session metadata file.
    pub fn create_session(&self, id: &str, title: &str) -> Result<(), String> {
        let dir = self.session_path(id);
        fs::create_dir_all(&dir).map_err(|e| format!("Failed to create session dir: {}", e))?;
        // Create messages sub-directory
        let msg_dir = dir.join("messages");
        fs::create_dir_all(&msg_dir).map_err(|e| format!("Failed to create messages dir: {}", e))?;

        let meta = SessionMeta {
            id: id.to_string(),
            title: title.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        let yaml = serde_yaml::to_string(&meta)
            .map_err(|e| format!("Failed to serialize session meta: {}", e))?;
        let content = format!("---\n{}---\n", yaml);
        fs::write(&dir.join("session.md"), &content)
            .map_err(|e| format!("Failed to write session.md: {}", e))?;
        Ok(())
    }

    /// Save a message as a numbered markdown file with YAML frontmatter.
    pub fn save_message(&self, session_id: &str, msg: &ChatMessage, seq: u32) -> Result<(), String> {
        let msg_dir = self.message_dir(session_id);
        if !msg_dir.exists() {
            fs::create_dir_all(&msg_dir)
                .map_err(|e| format!("Failed to create message dir: {}", e))?;
        }

        let filename = format!("{:08}.md", seq);

        // Build frontmatter
        let role_str = match msg.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::Tool => "tool",
        };

        let mut frontmatter = format!("role: {}\n", role_str);
        if let Some(ref calls) = msg.tool_calls {
            let json = serde_json::to_string(calls)
                .unwrap_or_else(|_| "[]".to_string());
            frontmatter.push_str(&format!("tool_calls: {}\n", json));
        }

        // For tool messages, include tool_call_id (serialized inside content)
        let content = format!("---\n{}---\n\n{}", frontmatter, msg.content);
        let path = msg_dir.join(&filename);
        fs::write(&path, &content)
            .map_err(|e| format!("Failed to write message file {}: {}", filename, e))
    }

    /// Load all messages for a session, ordered by sequence number.
    pub fn load_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>, String> {
        let msg_dir = self.message_dir(session_id);
        if !msg_dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries: Vec<_> = fs::read_dir(&msg_dir)
            .map_err(|e| format!("Failed to read messages dir: {}", e))?
            .filter_map(|entry| entry.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
            .collect();

        entries.sort_by_key(|e| e.file_name());

        let mut messages = Vec::new();
        for entry in entries {
            let content = fs::read_to_string(entry.path())
                .map_err(|e| format!("Failed to read {}: {}", entry.path().display(), e))?;
            if let Some(msg) = Self::parse_message_file(&content) {
                messages.push(msg);
            }
        }
        Ok(messages)
    }

    /// Load messages with token budget (oldest-first, accumulated from most recent)
    pub fn load_context_window(&self, session_id: &str, max_tokens: usize) -> Result<Vec<ChatMessage>, String> {
        let all = self.load_messages(session_id)?;
        let mut token_count: usize = 0usize;
        let mut result = Vec::new();

        for msg in all.iter().rev() {
            let tokens = Self::estimate_tokens(msg);
            if token_count + tokens > max_tokens && !result.is_empty() {
                break;
            }
            token_count += tokens;
            result.push(msg.clone());
        }

        result.reverse();
        Ok(result)
    }

    /// Delete a session directory entirely.
    pub fn delete_session(&self, session_id: &str) -> Result<(), String> {
        let dir = self.session_path(session_id);
        if dir.exists() {
            fs::remove_dir_all(&dir)
                .map_err(|e| format!("Failed to delete session dir: {}", e))?;
        }
        Ok(())
    }

    /// Clear all messages in a session (keep session metadata).
    pub fn clear_messages(&self, session_id: &str) -> Result<(), String> {
        let msg_dir = self.message_dir(session_id);
        if msg_dir.exists() {
            fs::remove_dir_all(&msg_dir)
                .map_err(|e| format!("Failed to clear messages dir: {}", e))?;
            fs::create_dir_all(&msg_dir)
                .map_err(|e| format!("Failed to recreate messages dir: {}", e))?;
        }
        Ok(())
    }

    // ── Conversation Branching ──────────────────────────────────────

    fn branches_path(&self, session_id: &str) -> PathBuf {
        self.session_path(session_id).join("branches.json")
    }

    /// Load branch map for a session. Returns empty map if no branches file exists.
    fn load_branches(&self, session_id: &str) -> BranchMap {
        let path = self.branches_path(session_id);
        if !path.exists() {
            return std::collections::HashMap::new();
        }
        let content = fs::read_to_string(&path).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    }

    /// Save branch map to disk.
    fn save_branches(&self, session_id: &str, branches: &BranchMap) -> Result<(), String> {
        let path = self.branches_path(session_id);
        let json = serde_json::to_string_pretty(branches)
            .map_err(|e| format!("Failed to serialize branches: {}", e))?;
        fs::write(&path, json).map_err(|e| format!("Failed to write branches.json: {}", e))
    }

    /// Create a new branch from a given message sequence number.
    /// The new branch includes all messages up to (and including) `from_seq`.
    pub fn create_branch(&self, session_id: &str, from_seq: u32, branch_name: &str) -> Result<String, String> {
        let mut branches = self.load_branches(session_id);

        // Find the parent branch that contains from_seq
        let parent_seqs = branches.get("main")
            .cloned()
            .unwrap_or_else(|| {
                // If no branches file exists, all messages are in "main"
                let msg_dir = self.message_dir(session_id);
                if !msg_dir.exists() { return Vec::new(); }
                let mut seqs: Vec<u32> = fs::read_dir(&msg_dir)
                    .ok()
                    .map(|entries| {
                        entries.filter_map(|e| e.ok())
                            .filter_map(|e| {
                                e.file_name().to_str()
                                    .and_then(|s| s.strip_suffix(".md"))
                                    .and_then(|s| s.parse::<u32>().ok())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                seqs.sort();
                seqs
            });

        // New branch includes messages up to from_seq
        let new_seqs: Vec<u32> = parent_seqs.into_iter()
            .filter(|&s| s <= from_seq)
            .collect();

        let branch_id = if branch_name.is_empty() {
            format!("branch_{}", branches.len() + 1)
        } else {
            branch_name.to_string()
        };

        branches.insert(branch_id.clone(), new_seqs);
        self.save_branches(session_id, &branches)?;

        Ok(branch_id)
    }

    /// List all branches for a session with their message counts.
    pub fn list_branches(&self, session_id: &str) -> Result<Vec<BranchInfo>, String> {
        let branches = self.load_branches(session_id);
        if branches.is_empty() {
            // Return default "main" branch
            let msg_count = self.load_messages(session_id)?.len() as u32;
            return Ok(vec![BranchInfo {
                id: "main".to_string(),
                name: "Main".to_string(),
                message_count: msg_count,
                is_active: true,
            }]);
        }

        let mut result: Vec<BranchInfo> = branches.iter().map(|(id, seqs)| {
            BranchInfo {
                id: id.clone(),
                name: id.clone(),
                message_count: seqs.len() as u32,
                is_active: id == "main",
            }
        }).collect();

        result.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(result)
    }

    /// Load messages for a specific branch.
    pub fn load_branch_messages(&self, session_id: &str, branch_id: &str) -> Result<Vec<ChatMessage>, String> {
        let branches = self.load_branches(session_id);
        let seqs = branches.get(branch_id)
            .ok_or_else(|| format!("Branch '{}' not found", branch_id))?;

        let all_messages = self.load_messages(session_id)?;
        let seq_set: std::collections::HashSet<u32> = seqs.iter().copied().collect();

        let filtered: Vec<ChatMessage> = all_messages.into_iter()
            .enumerate()
            .filter(|(idx, _)| seq_set.contains(&(*idx as u32)))
            .map(|(_, msg)| msg)
            .collect();

        Ok(filtered)
    }

    /// Delete a branch (cannot delete "main").
    pub fn delete_branch(&self, session_id: &str, branch_id: &str) -> Result<(), String> {
        if branch_id == "main" {
            return Err("Cannot delete the main branch".to_string());
        }
        let mut branches = self.load_branches(session_id);
        branches.remove(branch_id);
        self.save_branches(session_id, &branches)
    }

    /// List all sessions (returns (id, title) pairs).
    pub fn list_sessions(&self) -> Result<Vec<(String, String, String)>, String> {
        let dir = &self.sessions_dir;
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();
        let entries = fs::read_dir(dir)
            .map_err(|e| format!("Failed to read sessions dir: {}", e))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() { continue; }
            let session_id = path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            let meta_path = path.join("session.md");
            if meta_path.exists() {
                if let Ok(content) = fs::read_to_string(&meta_path) {
                    if let Some(smeta) = Self::parse_session_meta(&content) {
                        sessions.push((session_id, smeta.title, smeta.created_at));
                        continue;
                    }
                }
            }
            // Fallback if no session.md
            sessions.push((session_id.clone(), session_id, String::new()));
        }

        sessions.sort_by(|a, b| b.2.cmp(&a.2)); // newest first
        Ok(sessions)
    }

    /// Count the number of messages in a session (without loading them).
    pub fn count_messages(&self, session_id: &str) -> Result<usize, String> {
        let msg_dir = self.message_dir(session_id);
        if !msg_dir.exists() {
            return Ok(0);
        }
        let count = fs::read_dir(&msg_dir)
            .map_err(|e| format!("Failed to read messages dir: {}", e))?
            .filter_map(|entry| entry.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
            .count();
        Ok(count)
    }

    /// Get the next sequence number for a session.
    pub fn next_sequence(&self, session_id: &str) -> Result<u32, String> {
        let msg_dir = self.message_dir(session_id);
        if !msg_dir.exists() {
            return Ok(1);
        }

        let max_seq: u32 = fs::read_dir(msg_dir)
            .map_err(|e| format!("Failed to read messages dir: {}", e))?
            .filter_map(|entry| entry.ok())
            .filter_map(|e| {
                let name = e.file_name();
                let s = name.to_str()?;
                s.strip_suffix(".md")?.parse::<u32>().ok()
            })
            .max()
            .unwrap_or(0);

        Ok(max_seq + 1)
    }

    // ── helpers ──

    /// Public accessor for a session's message directory path
    pub fn message_dir_path(&self, id: &str) -> PathBuf {
        self.message_dir(id)
    }

    /// Clean up expired sessions older than `max_age_days`. Returns count of deleted.
    pub fn cleanup_expired_sessions(&self, max_age_days: u32) -> Result<usize, String> {
        if max_age_days == 0 {
            return Ok(0);
        }

        let dir = &self.sessions_dir;
        if !dir.exists() {
            return Ok(0);
        }

        let now = chrono::Utc::now();
        let cutoff = now - chrono::Duration::days(max_age_days as i64);
        let mut deleted = 0usize;

        let entries = fs::read_dir(dir)
            .map_err(|e| format!("Failed to read sessions dir: {}", e))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() { continue; }

            let meta_path = path.join("session.md");
            if meta_path.exists() {
                if let Ok(content) = fs::read_to_string(&meta_path) {
                    if let Some(meta) = Self::parse_session_meta(&content) {
                        if let Ok(created_at) = chrono::DateTime::parse_from_rfc3339(&meta.created_at) {
                            if created_at < cutoff {
                                if fs::remove_dir_all(&path).is_ok() {
                                    deleted += 1;
                                    log::info!("[SessionStore] Expired session '{}' (created {})", meta.id, meta.created_at);
                                }
                            }
                        }
                    }
                }
            }
        }

        if deleted > 0 {
            log::info!("[SessionStore] Cleaned up {} expired sessions (older than {} days)", deleted, max_age_days);
        }
        Ok(deleted)
    }

    fn session_path(&self, id: &str) -> PathBuf {
        self.sessions_dir.join(id)
    }

    fn message_dir(&self, id: &str) -> PathBuf {
        self.session_path(id).join("messages")
    }

    fn parse_message_file(content: &str) -> Option<ChatMessage> {
        // Format:
        // ---
        // role: <value>
        // tool_calls: <json_array_or_null>
        // ---
        // <content>
        let rest = content.strip_prefix("---\n")?;
        let end = rest.find("\n---")?;
        let frontmatter = &rest[..end];
        let body_start = end + 4;
        let body = content[body_start..].trim().to_string();

        let mut role_str = "";
        let mut tool_calls = None;

        for line in frontmatter.lines() {
            if let Some(val) = line.strip_prefix("role: ") {
                role_str = val.trim();
            } else if let Some(val) = line.strip_prefix("tool_calls: ") {
                let trimmed = val.trim();
                if trimmed != "null" && trimmed != "~" {
                    if let Ok(calls) = serde_json::from_str::<Vec<crate::chat::ToolCall>>(trimmed) {
                        tool_calls = Some(calls);
                    }
                }
            }
        }

        let role = match role_str {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "system" => Role::System,
            "tool" => Role::Tool,
            _ => return None,
        };

        Some(ChatMessage {
            role,
            content: body,
            images: None,
            tool_calls,
        })
    }

    fn parse_session_meta(content: &str) -> Option<SessionMeta> {
        // Format: ---\n<yaml>\n---
        let rest = content.strip_prefix("---\n")?;
        // Handle trailing ---\n
        let yaml_str = if let Some(end) = rest.rfind("\n---") {
            &rest[..end]
        } else {
            rest.trim_end_matches("---\n").trim_end_matches("---")
        };

        serde_yaml::from_str::<SessionMeta>(yaml_str).ok()
    }

    fn estimate_tokens(msg: &ChatMessage) -> usize {
        let content_tokens = msg.content.chars().count() / 3;
        let tool_call_tokens = msg.tool_calls.as_ref()
            .map(|tcs| serde_json::to_string(tcs).unwrap_or_default().len() / 3)
            .unwrap_or(0);
        content_tokens + tool_call_tokens
    }
}

// ── SessionStore impl for SessionStorage ────────────────────────────────────

impl SessionStore for SessionStorage {
    fn create_session(&self, id: &str, title: &str) -> Result<(), String> {
        SessionStorage::create_session(self, id, title)
    }

    fn save_message(&self, session_id: &str, msg: &ChatMessage, seq: u32) -> Result<(), String> {
        SessionStorage::save_message(self, session_id, msg, seq)
    }

    fn load_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>, String> {
        SessionStorage::load_messages(self, session_id)
    }

    fn delete_session(&self, session_id: &str) -> Result<(), String> {
        SessionStorage::delete_session(self, session_id)
    }

    fn cleanup_expired(&self, max_age_days: u32) -> Result<usize, String> {
        self.cleanup_expired_sessions(max_age_days)
    }
}
