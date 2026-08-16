//! Append-only JSONL agent log for session persistence and replay.
//!
//! Each agent session gets its own log file that records every significant event:
//! user messages, assistant responses, tool calls/results, compaction summaries,
//! errors, and cancellations. This enables:
//! - Session replay (reconstruct the full conversation history)
//! - Crash recovery (resume from last known state)
//! - Debugging (trace exact tool call sequence)

use std::path::Path;

use crate::event_bus::JsonlAppender;

/// A single log entry in the agent's JSONL log.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    /// Monotonically increasing sequence number
    pub seq: u64,
    /// Unix timestamp (seconds)
    pub timestamp: i64,
    /// Agent identifier
    pub agent_id: String,
    /// The entry payload
    #[serde(flatten)]
    pub entry_type: LogEntryType,
}

/// Types of log entries recorded during an agent session.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum LogEntryType {
    /// User sent a message
    UserMessage {
        content: String,
    },
    /// Assistant produced a response (text or tool calls)
    AssistantMessage {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<ToolCallInfo>>,
    },
    /// A tool was invoked
    ToolCall {
        name: String,
        arguments: serde_json::Value,
    },
    /// A tool returned a result
    ToolResult {
        name: String,
        result: String,
        duration_ms: u64,
    },
    /// Context compaction occurred
    CompactionSummary {
        summary: String,
        tokens_before: usize,
        tokens_after: usize,
    },
    /// An error occurred
    Error {
        message: String,
    },
    /// The agent was cancelled by the user
    Cancelled,
    /// The agent completed successfully
    Completed {
        final_text: String,
    },
}

/// Compact representation of a tool call for logging.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Append-only JSONL agent log.
///
/// Each line in the file is a single `LogEntry` JSON object.
/// Writes go through the shared `JsonlAppender` (EventBus core) and are
/// flushed to disk after each entry.
pub struct AgentLog {
    appender: JsonlAppender,
    next_seq: u64,
    agent_id: String,
}

impl AgentLog {
    /// Create or open an agent log file.
    ///
    /// If the file already exists, entries are appended. The sequence counter
    /// is initialized based on existing entries.
    pub async fn new(session_dir: &Path, session_id: &str, agent_id: &str) -> Result<Self, String> {
        let log_dir = session_dir.join("agent_logs");
        let file_name = format!("{}.jsonl", session_id);

        // Count existing entries to initialize next_seq
        let file_path = log_dir.join(&file_name);
        let next_seq = if file_path.exists() {
            let content = std::fs::read_to_string(&file_path).unwrap_or_default();
            content.lines().filter(|l| !l.is_empty()).count() as u64
        } else {
            0
        };

        Ok(Self {
            appender: JsonlAppender::open(&log_dir, &file_name),
            next_seq,
            agent_id: agent_id.to_string(),
        })
    }

    /// Append a log entry and flush to disk.
    pub async fn append(&mut self, entry_type: LogEntryType) -> Result<(), String> {
        let entry = LogEntry {
            seq: self.next_seq,
            timestamp: chrono::Utc::now().timestamp(),
            agent_id: self.agent_id.clone(),
            entry_type,
        };
        self.next_seq += 1;

        self.appender.append(&entry)
    }

    /// Get the file path of this log.
    pub fn path(&self) -> &Path {
        self.appender.path()
    }

    /// Read and replay all entries from a log file.
    pub async fn replay(log_path: &Path) -> Result<Vec<LogEntry>, String> {
        if !log_path.exists() {
            return Ok(Vec::new());
        }

        let content = tokio::fs::read_to_string(log_path)
            .await
            .map_err(|e| format!("Failed to read agent log: {}", e))?;

        let mut entries = Vec::new();
        for (line_num, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<LogEntry>(line) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    log::warn!(
                        "Skipping malformed log entry at line {}: {}",
                        line_num + 1,
                        e
                    );
                }
            }
        }

        Ok(entries)
    }

    /// Convert log entries back into LLM chat messages for replay.
    pub fn to_messages(entries: &[LogEntry]) -> Vec<crate::llm::ChatMessage> {
        let mut messages = Vec::new();
        // Tool results must reference the tool_call_id of the assistant's
        // preceding tool_calls, otherwise OpenAI-compatible APIs reject the
        // request (role "tool" must follow an assistant tool_call). Queue the
        // ids in order as assistant messages appear and consume them as tool
        // results arrive.
        let mut pending_tool_call_ids: std::collections::VecDeque<String> =
            std::collections::VecDeque::new();

        for entry in entries {
            match &entry.entry_type {
                LogEntryType::UserMessage { content } => {
                    messages.push(crate::llm::ChatMessage {
                        role: "user".into(),
                        content: content.clone(),
                        images: None,
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                LogEntryType::AssistantMessage {
                    content,
                    tool_calls,
                } => {
                    for tc in tool_calls.iter().flatten() {
                        pending_tool_call_ids.push_back(tc.id.clone());
                    }
                    let tc_json = tool_calls.as_ref().map(|calls| {
                        serde_json::Value::Array(
                            calls
                                .iter()
                                .map(|tc| {
                                    serde_json::json!({
                                        "id": tc.id,
                                        "type": "function",
                                        "function": {
                                            "name": tc.name,
                                            "arguments": serde_json::to_string(&tc.arguments)
                                                .unwrap_or_default(),
                                        }
                                    })
                                })
                                .collect(),
                        )
                    });
                    messages.push(crate::llm::ChatMessage {
                        role: "assistant".into(),
                        content: content.clone(),
                        images: None,
                        tool_calls: tc_json,
                        tool_call_id: None,
                    });
                }
                LogEntryType::ToolResult {
                    name: _,
                    result,
                    ..
                } => {
                    let call_id = pending_tool_call_ids
                        .pop_front()
                        .unwrap_or_else(|| format!("replay-{}", entry.seq));
                    messages.push(crate::llm::ChatMessage {
                        role: "tool".into(),
                        content: result.clone(),
                        images: None,
                        tool_calls: None,
                        tool_call_id: Some(call_id),
                    });
                }
                LogEntryType::CompactionSummary { summary, .. } => {
                    messages.push(crate::llm::ChatMessage {
                        role: "system".into(),
                        content: format!(
                            "[CONTEXT COMPACTED] Summary of previous conversation:\n\n{}",
                            summary
                        ),
                        images: None,
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                // ToolCall, Error, Cancelled, Completed don't produce LLM messages
                _ => {}
            }
        }

        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_append_and_replay() {
        let tmp_dir = std::env::temp_dir().join("neocoder_test_agent_log");
        let _ = tokio::fs::create_dir_all(&tmp_dir).await;

        let mut log = AgentLog::new(&tmp_dir, "test-session-1", "test-agent")
            .await
            .unwrap();

        log.append(LogEntryType::UserMessage {
            content: "Hello".into(),
        })
        .await
        .unwrap();

        log.append(LogEntryType::AssistantMessage {
            content: "Hi there!".into(),
            tool_calls: None,
        })
        .await
        .unwrap();

        log.append(LogEntryType::ToolCall {
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "main.rs"}),
        })
        .await
        .unwrap();

        log.append(LogEntryType::ToolResult {
            name: "read_file".into(),
            result: "fn main() {}".into(),
            duration_ms: 42,
        })
        .await
        .unwrap();

        // Replay
        let entries = AgentLog::replay(log.path()).await.unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].seq, 0);
        assert_eq!(entries[1].seq, 1);
        assert!(matches!(
            entries[0].entry_type,
            LogEntryType::UserMessage { .. }
        ));
        assert!(matches!(
            entries[1].entry_type,
            LogEntryType::AssistantMessage { .. }
        ));

        // Cleanup
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
    }

    #[test]
    fn test_to_messages() {
        let entries = vec![
            LogEntry {
                seq: 0,
                timestamp: 1000,
                agent_id: "test".into(),
                entry_type: LogEntryType::UserMessage {
                    content: "Build a feature".into(),
                },
            },
            LogEntry {
                seq: 1,
                timestamp: 1001,
                agent_id: "test".into(),
                entry_type: LogEntryType::AssistantMessage {
                    content: "I'll do that".into(),
                    tool_calls: None,
                },
            },
            LogEntry {
                seq: 2,
                timestamp: 1002,
                agent_id: "test".into(),
                entry_type: LogEntryType::ToolResult {
                    name: "read_file".into(),
                    result: "file contents".into(),
                    duration_ms: 10,
                },
            },
        ];

        let messages = AgentLog::to_messages(&entries);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "Build a feature");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[2].role, "tool");
    }
}
