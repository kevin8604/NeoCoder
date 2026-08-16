//! Telemetry / Usage Analytics System
//!
//! Tracks tool usage frequency, agent success rates, average iterations,
//! token consumption, and other metrics for data-driven improvement.
//!
//! ## Architecture
//! - **In-memory counters**: Thread-safe atomics for real-time stats
//! - **JSONL file**: Persistent append-only log at `{app_data}/telemetry/telemetry.jsonl`
//!   (separate from `logs/neocoder.log` to avoid mixing)
//! - **Query API**: `get_summary()` returns aggregated snapshot
//!
//! ## File Format
//! Each line is a JSON object with `event`, `timestamp`, and event-specific fields.
//! Example:
//! ```json
//! {"event":"session_start","ts":1720000000,"session_id":"abc","model":"gpt-4o"}
//! {"event":"tool_call","ts":1720000001,"session_id":"abc","tool":"read_file","success":true,"duration_ms":42}
//! {"event":"session_end","ts":1720000060,"session_id":"abc","outcome":"success","iterations":5,"tokens":1234}
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::event_bus::{EventBus, JsonlAppender};
use serde::{Deserialize, Serialize};

/// Telemetry event types written to the JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum TelemetryEvent {
    /// Agent session started
    #[serde(rename = "session_start")]
    SessionStart {
        session_id: String,
        model: String,
        provider: String,
        plan_mode: bool,
    },
    /// Agent session finished
    #[serde(rename = "session_end")]
    SessionEnd {
        session_id: String,
        outcome: String, // "success", "error", "cancelled"
        iterations: usize,
        total_prompt_tokens: usize,
        total_completion_tokens: usize,
        duration_ms: u64,
        error_message: Option<String>,
    },
    /// A tool was invoked
    #[serde(rename = "tool_call")]
    ToolCall {
        session_id: String,
        tool: String,
        success: bool,
        duration_ms: u64,
        /// Whether this was a repeated failure (loop detection)
        is_loop: bool,
    },
    /// Model routing decision
    #[serde(rename = "model_routing")]
    ModelRouting {
        session_id: String,
        selected_model: String,
        reason: String, // "fast", "chat", "thinking", "manual"
    },
    /// Code completion request
    #[serde(rename = "completion")]
    Completion {
        model: String,
        trigger: String, // "manual", "auto"
        latency_ms: u64,
        success: bool,
    },
    /// Inline edit request
    #[serde(rename = "inline_edit")]
    InlineEdit {
        model: String,
        latency_ms: u64,
        success: bool,
    },
}

/// In-memory aggregated counters for fast queries.
pub struct TelemetryCounters {
    pub total_sessions: AtomicU64,
    pub successful_sessions: AtomicU64,
    pub failed_sessions: AtomicU64,
    pub cancelled_sessions: AtomicU64,
    pub total_iterations: AtomicU64,
    pub total_tool_calls: AtomicU64,
    pub total_tool_failures: AtomicU64,
    pub total_prompt_tokens: AtomicU64,
    pub total_completion_tokens: AtomicU64,
    pub total_completions: AtomicU64,
    pub total_inline_edits: AtomicU64,
    /// Per-tool usage counts: tool_name → count
    pub tool_usage: Mutex<HashMap<String, u64>>,
    /// Per-model usage counts: model_name → session count
    pub model_usage: Mutex<HashMap<String, u64>>,
}

impl TelemetryCounters {
    fn new() -> Self {
        Self {
            total_sessions: AtomicU64::new(0),
            successful_sessions: AtomicU64::new(0),
            failed_sessions: AtomicU64::new(0),
            cancelled_sessions: AtomicU64::new(0),
            total_iterations: AtomicU64::new(0),
            total_tool_calls: AtomicU64::new(0),
            total_tool_failures: AtomicU64::new(0),
            total_prompt_tokens: AtomicU64::new(0),
            total_completion_tokens: AtomicU64::new(0),
            total_completions: AtomicU64::new(0),
            total_inline_edits: AtomicU64::new(0),
            tool_usage: Mutex::new(HashMap::new()),
            model_usage: Mutex::new(HashMap::new()),
        }
    }
}

/// Telemetry collector: thread-safe, manages both in-memory counters and JSONL file.
/// The JSONL file is written through the shared `EventBus` appender core.
pub struct TelemetryCollector {
    counters: TelemetryCounters,
    appender: Arc<JsonlAppender>,
}

impl TelemetryCollector {
    /// Create a new collector. Registers the telemetry file on the global event bus
    /// and initializes the telemetry directory.
    pub fn new(app_data_dir: &Path) -> Self {
        let telemetry_dir = app_data_dir.join("telemetry");

        // Register on the global event bus (shared JSONL append core)
        let appender = EventBus::global().register("telemetry", &telemetry_dir, "telemetry.jsonl");

        // Write a session marker
        let marker = serde_json::json!({
            "event": "telemetry_init",
            "ts": chrono::Utc::now().timestamp(),
            "version": env!("CARGO_PKG_VERSION"),
        });
        let _ = appender.append(&marker);

        Self {
            counters: TelemetryCounters::new(),
            appender,
        }
    }

    /// Record a telemetry event: updates counters and appends to JSONL file.
    pub fn record(&self, event: &TelemetryEvent) {
        // Update in-memory counters
        self.update_counters(event);

        // Append to JSONL file via the shared appender
        if let Err(e) = self.appender.append(event) {
            log::debug!("[Telemetry] Failed to record event: {}", e);
        }
    }

    fn update_counters(&self, event: &TelemetryEvent) {
        match event {
            TelemetryEvent::SessionStart { model, .. } => {
                self.counters.total_sessions.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut usage) = self.counters.model_usage.lock() {
                    *usage.entry(model.clone()).or_insert(0) += 1;
                }
            }
            TelemetryEvent::SessionEnd {
                outcome,
                iterations,
                total_prompt_tokens,
                total_completion_tokens,
                ..
            } => {
                match outcome.as_str() {
                    "success" => self.counters.successful_sessions.fetch_add(1, Ordering::Relaxed),
                    "error" => self.counters.failed_sessions.fetch_add(1, Ordering::Relaxed),
                    "cancelled" => self.counters.cancelled_sessions.fetch_add(1, Ordering::Relaxed),
                    _ => 0,
                };
                self.counters.total_iterations.fetch_add(*iterations as u64, Ordering::Relaxed);
                self.counters.total_prompt_tokens.fetch_add(*total_prompt_tokens as u64, Ordering::Relaxed);
                self.counters.total_completion_tokens.fetch_add(*total_completion_tokens as u64, Ordering::Relaxed);
            }
            TelemetryEvent::ToolCall { tool, success, is_loop, .. } => {
                self.counters.total_tool_calls.fetch_add(1, Ordering::Relaxed);
                if !success {
                    self.counters.total_tool_failures.fetch_add(1, Ordering::Relaxed);
                }
                if let Ok(mut usage) = self.counters.tool_usage.lock() {
                    *usage.entry(tool.clone()).or_insert(0) += 1;
                }
                let _ = is_loop; // tracked in JSONL but not in counters
            }
            TelemetryEvent::Completion { .. } => {
                self.counters.total_completions.fetch_add(1, Ordering::Relaxed);
            }
            TelemetryEvent::InlineEdit { .. } => {
                self.counters.total_inline_edits.fetch_add(1, Ordering::Relaxed);
            }
            TelemetryEvent::ModelRouting { .. } => {
                // No counter update, just logged to file
            }
        }
    }

    /// Get a summary snapshot of current telemetry data.
    pub fn get_summary(&self) -> TelemetrySummary {
        let total_sessions = self.counters.total_sessions.load(Ordering::Relaxed);
        let successful = self.counters.successful_sessions.load(Ordering::Relaxed);
        let failed = self.counters.failed_sessions.load(Ordering::Relaxed);
        let cancelled = self.counters.cancelled_sessions.load(Ordering::Relaxed);
        let total_iterations = self.counters.total_iterations.load(Ordering::Relaxed);
        let total_tool_calls = self.counters.total_tool_calls.load(Ordering::Relaxed);
        let total_tool_failures = self.counters.total_tool_failures.load(Ordering::Relaxed);

        let avg_iterations = if total_sessions > 0 {
            total_iterations as f64 / total_sessions as f64
        } else {
            0.0
        };

        let success_rate = if total_sessions > 0 {
            successful as f64 / total_sessions as f64 * 100.0
        } else {
            0.0
        };

        let tool_failure_rate = if total_tool_calls > 0 {
            total_tool_failures as f64 / total_tool_calls as f64 * 100.0
        } else {
            0.0
        };

        let tool_usage = self.counters.tool_usage.lock()
            .map(|g| g.clone())
            .unwrap_or_default();

        let model_usage = self.counters.model_usage.lock()
            .map(|g| g.clone())
            .unwrap_or_default();

        // Sort tool usage by count (descending)
        let mut tool_usage_sorted: Vec<(String, u64)> = tool_usage.into_iter().collect();
        tool_usage_sorted.sort_by(|a, b| b.1.cmp(&a.1));

        TelemetrySummary {
            total_sessions,
            successful_sessions: successful,
            failed_sessions: failed,
            cancelled_sessions: cancelled,
            success_rate,
            avg_iterations,
            total_iterations,
            total_tool_calls,
            total_tool_failures,
            tool_failure_rate,
            total_prompt_tokens: self.counters.total_prompt_tokens.load(Ordering::Relaxed),
            total_completion_tokens: self.counters.total_completion_tokens.load(Ordering::Relaxed),
            total_completions: self.counters.total_completions.load(Ordering::Relaxed),
            total_inline_edits: self.counters.total_inline_edits.load(Ordering::Relaxed),
            top_tools: tool_usage_sorted.into_iter().take(10).collect(),
            model_usage,
        }
    }

    /// Get the telemetry file path.
    pub fn file_path(&self) -> PathBuf {
        self.appender.path().to_path_buf()
    }

    /// Read the last N events from the telemetry file.
    pub fn read_recent_events(&self, count: usize) -> Vec<TelemetryEvent> {
        let path = self.file_path();
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(count);

        lines[start..]
            .iter()
            .filter_map(|line| serde_json::from_str::<TelemetryEvent>(line).ok())
            .collect()
    }
}

/// Aggregated telemetry summary for frontend display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySummary {
    pub total_sessions: u64,
    pub successful_sessions: u64,
    pub failed_sessions: u64,
    pub cancelled_sessions: u64,
    pub success_rate: f64,
    pub avg_iterations: f64,
    pub total_iterations: u64,
    pub total_tool_calls: u64,
    pub total_tool_failures: u64,
    pub tool_failure_rate: f64,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_completions: u64,
    pub total_inline_edits: u64,
    /// Top 10 most-used tools: [(tool_name, count)]
    pub top_tools: Vec<(String, u64)>,
    /// Per-model session counts
    pub model_usage: HashMap<String, u64>,
}

// ── Tauri Commands ──────────────────────────────────────────────────────────

/// Get telemetry summary for frontend dashboard.
#[tauri::command]
pub fn get_telemetry_summary(
    state: tauri::State<'_, TelemetryCollector>,
) -> TelemetrySummary {
    state.get_summary()
}

/// Get recent telemetry events.
#[tauri::command]
pub fn get_telemetry_events(
    state: tauri::State<'_, TelemetryCollector>,
    count: Option<usize>,
) -> Vec<TelemetryEvent> {
    state.read_recent_events(count.unwrap_or(100))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telemetry_collector_basic() {
        let tmp = std::env::temp_dir().join("neocoder_telemetry_test");
        let _ = fs::create_dir_all(&tmp);

        let collector = TelemetryCollector::new(&tmp);

        // Record some events
        collector.record(&TelemetryEvent::SessionStart {
            session_id: "test-1".into(),
            model: "gpt-4o".into(),
            provider: "openai".into(),
            plan_mode: false,
        });

        collector.record(&TelemetryEvent::ToolCall {
            session_id: "test-1".into(),
            tool: "read_file".into(),
            success: true,
            duration_ms: 42,
            is_loop: false,
        });

        collector.record(&TelemetryEvent::ToolCall {
            session_id: "test-1".into(),
            tool: "edit".into(),
            success: false,
            duration_ms: 100,
            is_loop: false,
        });

        collector.record(&TelemetryEvent::SessionEnd {
            session_id: "test-1".into(),
            outcome: "success".into(),
            iterations: 5,
            total_prompt_tokens: 1000,
            total_completion_tokens: 500,
            duration_ms: 30000,
            error_message: None,
        });

        // Check summary
        let summary = collector.get_summary();
        assert_eq!(summary.total_sessions, 1);
        assert_eq!(summary.successful_sessions, 1);
        assert_eq!(summary.total_tool_calls, 2);
        assert_eq!(summary.total_tool_failures, 1);
        assert_eq!(summary.total_iterations, 5);
        assert!((summary.success_rate - 100.0).abs() < 0.01);

        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_telemetry_file_persistence() {
        let tmp = std::env::temp_dir().join("neocoder_telemetry_persist_test");
        let _ = fs::create_dir_all(&tmp);

        {
            let collector = TelemetryCollector::new(&tmp);
            collector.record(&TelemetryEvent::SessionStart {
                session_id: "persist-test".into(),
                model: "claude-3.5-sonnet".into(),
                provider: "anthropic".into(),
                plan_mode: true,
            });
        } // collector dropped, file flushed

        // Read back
        let path = tmp.join("telemetry").join("telemetry.jsonl");
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("persist-test"));
        assert!(content.contains("claude-3.5-sonnet"));

        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
    }
}
