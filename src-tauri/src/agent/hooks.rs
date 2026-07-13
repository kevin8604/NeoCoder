//! Lifecycle Hooks framework for the Agent harness.
//!
//! Provides pre-tool / post-tool / post-tool-batch hook mechanisms,
//! replacing hardcoded logic (snapshot, confirm, auto-diagnose) with
//! a pluggable, ordered hook chain.

use std::collections::{HashMap, VecDeque};
use std::io::Write as _;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{Emitter, Manager};
use crate::llm;

// ── Hook result types ──

/// Result of a pre-tool hook. Determines whether the tool should execute.
pub enum HookResult {
    /// Continue with tool execution (no modification).
    Continue,
    /// Deny tool execution with a reason message (injected as tool result).
    Deny(String),
    /// Modify tool arguments before execution.
    ModifyArgs(serde_json::Value),
}

/// Result of a post-tool hook. Can modify the result or inject additional messages.
pub struct PostHookResult {
    /// If Some, replaces the original tool result string.
    pub modified_result: Option<String>,
    /// Additional messages to inject into the LLM context after this tool.
    pub additional_messages: Vec<llm::ChatMessage>,
}

impl Default for PostHookResult {
    fn default() -> Self {
        Self {
            modified_result: None,
            additional_messages: Vec::new(),
        }
    }
}

/// Result of a post-tool-batch hook. Injects messages after all tools in a batch complete.
pub struct BatchHookResult {
    /// Additional messages to inject into the LLM context after the batch.
    pub additional_messages: Vec<llm::ChatMessage>,
}

impl Default for BatchHookResult {
    fn default() -> Self {
        Self { additional_messages: Vec::new() }
    }
}

// ── Hook context ──

/// Shared context passed to all hooks during tool execution.
pub struct HookContext {
    pub app: Option<tauri::AppHandle>,
    pub session_id: String,
    pub agent_id: String,
    pub project_path: Option<String>,
    pub cancelled: Arc<AtomicBool>,
    /// File snapshots for undo/rollback: file_path → original_content
    pub file_snapshots: Arc<std::sync::Mutex<HashMap<String, String>>>,
    /// Global snapshot store for cross-agent undo
    pub file_snapshot_store: Option<crate::commands::chat::FileSnapshotStore>,
}

// ── Lifecycle hook trait ──

#[async_trait::async_trait]
pub trait LifecycleHook: Send + Sync {
    /// Hook name for logging/debugging.
    fn name(&self) -> &str;

    /// Called before each tool execution. Can deny, modify args, or pass through.
    async fn pre_tool(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        _ctx: &HookContext,
    ) -> HookResult {
        let _ = (tool_name, args);
        HookResult::Continue
    }

    /// Called after each tool execution. Can modify result or inject messages.
    async fn post_tool(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        result: &str,
        _ctx: &HookContext,
    ) -> PostHookResult {
        let _ = (tool_name, args, result);
        PostHookResult::default()
    }

    /// Called after ALL tools in a batch complete. Can inject batch-level messages.
    async fn post_tool_batch(
        &self,
        tool_calls: &[crate::llm::ToolCallRequest],
        _ctx: &HookContext,
    ) -> BatchHookResult {
        let _ = tool_calls;
        BatchHookResult::default()
    }
}

// ── HookManager ──

/// Manages an ordered list of lifecycle hooks and executes them in sequence.
pub struct HookManager {
    hooks: Vec<Arc<dyn LifecycleHook>>,
}

impl HookManager {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Register a hook. Hooks execute in registration order.
    pub fn register(&mut self, hook: impl LifecycleHook + 'static) {
        self.hooks.push(Arc::new(hook));
    }

    /// Execute pre-tool hook chain. Returns the first Deny, or Continue.
    /// If a hook returns ModifyArgs, subsequent hooks see the modified args.
    pub async fn pre_tool_chain(
        &self,
        tool_name: &str,
        args: &mut serde_json::Value,
        ctx: &HookContext,
    ) -> HookResult {
        for hook in &self.hooks {
            match hook.pre_tool(tool_name, args, ctx).await {
                HookResult::Continue => {}
                HookResult::Deny(msg) => {
                    log::debug!("[Hooks] {} denied: {}", hook.name(), msg);
                    return HookResult::Deny(msg);
                }
                HookResult::ModifyArgs(new_args) => {
                    log::debug!("[Hooks] {} modified args for {}", hook.name(), tool_name);
                    *args = new_args;
                }
            }
        }
        HookResult::Continue
    }

    /// Execute post-tool hook chain. Collects additional messages from all hooks.
    pub async fn post_tool_chain(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        result: &str,
        ctx: &HookContext,
    ) -> PostHookResult {
        let mut final_result = result.to_string();
        let mut all_additional = Vec::new();

        for hook in &self.hooks {
            let hook_result = hook.post_tool(tool_name, args, &final_result, ctx).await;
            if let Some(modified) = hook_result.modified_result {
                final_result = modified;
            }
            all_additional.extend(hook_result.additional_messages);
        }

        PostHookResult {
            modified_result: if final_result != result { Some(final_result) } else { None },
            additional_messages: all_additional,
        }
    }

    /// Execute post-tool-batch hook chain. Collects batch-level additional messages.
    pub async fn post_tool_batch_chain(
        &self,
        tool_calls: &[crate::llm::ToolCallRequest],
        ctx: &HookContext,
    ) -> Vec<llm::ChatMessage> {
        let mut all_additional = Vec::new();
        for hook in &self.hooks {
            let result = hook.post_tool_batch(tool_calls, ctx).await;
            all_additional.extend(result.additional_messages);
        }
        all_additional
    }
}

// ════════════════════════════════════════════════════════════════════════
// Built-in Hooks
// ════════════════════════════════════════════════════════════════════════

// ── SnapshotHook ──

/// Saves file content before write/edit/append/delete operations for undo/rollback.
pub struct SnapshotHook;

#[async_trait::async_trait]
impl LifecycleHook for SnapshotHook {
    fn name(&self) -> &str { "SnapshotHook" }

    async fn pre_tool(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        ctx: &HookContext,
    ) -> HookResult {
        if !matches!(tool_name, "write_file" | "edit" | "append_file" | "delete_file") {
            return HookResult::Continue;
        }

        let raw_path = args.get("file_path").and_then(|v| v.as_str());
        let raw_path = match raw_path {
            Some(p) => p,
            None => return HookResult::Continue,
        };

        let resolved = crate::agent::utils::resolve_path(ctx.project_path.as_deref(), raw_path);
        let key = resolved.to_string_lossy().to_string();

        // Only snapshot on first write per file
        {
            let snapshots = ctx.file_snapshots.lock()
                .unwrap_or_else(|e| e.into_inner());
            if snapshots.contains_key(&key) {
                return HookResult::Continue;
            }
        }

        let original = std::fs::read_to_string(&resolved).unwrap_or_default();

        {
            let mut snapshots = ctx.file_snapshots.lock()
                .unwrap_or_else(|e| e.into_inner());
            snapshots.insert(key.clone(), original.clone());
        }

        // Also save to global FileSnapshotStore for undo mechanism
        if let Some(store) = &ctx.file_snapshot_store {
            if let Ok(mut snapshots) = store.lock() {
                let session_snapshots = snapshots
                    .entry(ctx.session_id.clone())
                    .or_default();
                session_snapshots.entry(key).or_insert(original);
            }
        }

        HookResult::Continue
    }
}

// ── ConfirmHook ──

/// Requests user confirmation before dangerous operations (delete, terminal commands).
pub struct ConfirmHook;

impl ConfirmHook {
    fn needs_confirmation(tool_name: &str) -> bool {
        matches!(tool_name, "delete_file" | "delete_directory" | "run_terminal_command")
    }

    fn build_description(tool_name: &str, args: &serde_json::Value) -> String {
        match tool_name {
            "delete_file" => {
                let path = args["path"].as_str().unwrap_or("(unknown)");
                format!("Delete file: {}", path)
            }
            "delete_directory" => {
                let path = args["path"].as_str().unwrap_or("(unknown)");
                format!("Delete directory (recursive): {}", path)
            }
            "run_terminal_command" => {
                let cmd = args["command"].as_str().unwrap_or("(unknown)");
                format!("Execute command: {}", cmd)
            }
            _ => format!("Execute: {}", tool_name),
        }
    }
}

#[async_trait::async_trait]
impl LifecycleHook for ConfirmHook {
    fn name(&self) -> &str { "ConfirmHook" }

    async fn pre_tool(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        ctx: &HookContext,
    ) -> HookResult {
        if !Self::needs_confirmation(tool_name) {
            return HookResult::Continue;
        }

        let description = Self::build_description(tool_name, args);

        // AppHandle required for confirm flow; if absent (e.g. in tests), allow
        let app = match ctx.app.as_ref() {
            Some(a) => a,
            None => {
                log::warn!("[ConfirmHook] No AppHandle available — allowing: {}", tool_name);
                return HookResult::Continue;
            }
        };

        // Try to get ConfirmAwaiters from Tauri state
        let awaiters = match app.try_state::<crate::agent::ConfirmAwaiters>() {
            Some(state) => state.inner().clone(),
            None => {
                // No confirm system available (dev fallback) — allow
                log::warn!("[ConfirmHook] ConfirmAwaiters not available — allowing: {}", tool_name);
                return HookResult::Continue;
            }
        };

        let confirm_id = uuid::Uuid::new_v4().to_string();

        // Emit confirm request event
        let _ = app.emit(
            "chat-event",
            crate::chat::ChatEvent::ConfirmRequest {
                session_id: ctx.session_id.clone(),
                agent_id: Some(ctx.agent_id.clone()),
                confirm_id: confirm_id.clone(),
                tool_name: tool_name.to_string(),
                description: description.clone(),
            },
        );

        let (sender, receiver) = tokio::sync::oneshot::channel();
        {
            let mut map = match awaiters.lock() {
                Ok(m) => m,
                Err(e) => {
                    log::warn!("[ConfirmHook] Failed to acquire confirm lock: {}", e);
                    return HookResult::Continue;
                }
            };
            map.insert(confirm_id, sender);
        }

        // Wait for user response, timeout 60s → auto-deny
        match tokio::time::timeout(std::time::Duration::from_secs(60), receiver).await {
            Ok(Ok(allowed)) => {
                if allowed {
                    HookResult::Continue
                } else {
                    let deny_msg = format!(
                        "[USER_DENIED] Operation '{}' was denied by the user. Please skip this action and continue with an alternative approach if possible.",
                        tool_name
                    );
                    HookResult::Deny(deny_msg)
                }
            }
            _ => {
                log::warn!("[ConfirmHook] Confirmation timed out — denying: {}", tool_name);
                let deny_msg = format!(
                    "[USER_DENIED] Operation '{}' was denied by the user (timeout). Please skip this action.",
                    tool_name
                );
                HookResult::Deny(deny_msg)
            }
        }
    }
}

// ── AutoDiagnoseHook ──

/// After file-modifying tools, auto-run compiler/linter diagnostics in parallel
/// and inject results as a tool message so the LLM can self-fix errors.
pub struct AutoDiagnoseHook;

#[async_trait::async_trait]
impl LifecycleHook for AutoDiagnoseHook {
    fn name(&self) -> &str { "AutoDiagnoseHook" }

    async fn post_tool_batch(
        &self,
        tool_calls: &[crate::llm::ToolCallRequest],
        ctx: &HookContext,
    ) -> BatchHookResult {
        // Collect modified file paths from this batch
        let mut modified_files: Vec<String> = Vec::new();
        for tc in tool_calls {
            if matches!(tc.name.as_str(), "write_file" | "edit" | "append_file") {
                if let Some(raw_path) = tc.arguments.get("file_path").and_then(|v| v.as_str()) {
                    let resolved = crate::agent::utils::resolve_path(ctx.project_path.as_deref(), raw_path);
                    modified_files.push(resolved.to_string_lossy().to_string());
                }
            }
        }

        if modified_files.is_empty() {
            return BatchHookResult::default();
        }

        let work_dir = ctx.project_path.as_deref().unwrap_or(".").to_string();

        // Build diagnostic futures — all run in parallel
        let diag_futures = modified_files.iter().map(|file_path| {
            let file_path = file_path.clone();
            let work_dir = work_dir.clone();
            async move {
                let language = crate::lsp::detect_language(&file_path);
                let lsp_lang = if language == "typescript" || language == "javascript" {
                    "typescript"
                } else {
                    &language
                };

                let (cmd, args): (&str, Vec<String>) = match lsp_lang {
                    "rust" => ("cargo", vec!["check".into(), "--message-format=short".into()]),
                    "typescript" | "javascript" => ("npx", vec!["tsc".into(), "--noEmit".into(), "--pretty".into(), "false".into()]),
                    "python" => ("python", vec!["-m".into(), "py_compile".into(), file_path.clone()]),
                    "go" => ("go", vec!["vet".into(), "./...".into()]),
                    "c" => ("gcc", vec!["-fsyntax-only".into(), "-Wall".into(), file_path.clone()]),
                    "cpp" => ("g++", vec!["-fsyntax-only".into(), "-Wall".into(), file_path.clone()]),
                    "java" => ("javac", vec!["-Xlint:all".into(), file_path.clone()]),
                    _ => return None,
                };

                let output = tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    tokio::process::Command::new(cmd)
                        .args(&args)
                        .current_dir(&work_dir)
                        .output(),
                ).await;

                if let Ok(Ok(out)) = output {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let combined = format!("{}{}", stdout, stderr);
                    let has_errors = out.status.code() != Some(0) && !combined.trim().is_empty();
                    if has_errors {
                        let file_name = std::path::Path::new(&file_path)
                            .file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                        let relevant: Vec<&str> = combined.lines()
                            .filter(|l| l.contains(&file_name) || l.contains("error") || l.contains("Error"))
                            .take(20)
                            .collect();
                        if !relevant.is_empty() {
                            return Some(format!("\n[{}] {}\n", file_path, relevant.join("\n")));
                        }
                    }
                }
                None
            }
        });

        // Run all diagnostics in parallel
        let results = futures_util::future::join_all(diag_futures).await;
        let mut all_diagnostics = String::new();
        for result in results.into_iter().flatten() {
            all_diagnostics.push_str(&result);
        }

        if all_diagnostics.is_empty() {
            return BatchHookResult::default();
        }

        // Inject as a system message so LLM sees the errors.
        // NOTE: Must use role "system" (not "tool") because this message is NOT a
        // response to any assistant tool_call — using role "tool" with a fabricated
        // tool_call_id (e.g. "auto-diag") causes DeepSeek/OpenAI API to reject the
        // request with 400 Bad Request ("Messages with role 'tool' must be a response
        // to a preceding message with 'tool_calls'").
        let mut additional_messages = Vec::new();
        additional_messages.push(llm::ChatMessage {
            role: "system".into(),
            content: format!(
                "[AUTO-DIAGNOSTICS] The following files were modified and have errors. Fix them now:\n{}",
                all_diagnostics
            ),
            images: None,
            tool_calls: None,
            tool_call_id: None,
        });

        BatchHookResult { additional_messages }
    }
}

// ── OutputTruncateHook ──

/// Intelligently truncates overly long tool results to save tokens.
/// Keeps the head (first 2/3) and tail (last 1/3) of the result.
pub struct OutputTruncateHook {
    max_chars: usize,
}

impl OutputTruncateHook {
    pub fn new(max_chars: usize) -> Self {
        Self { max_chars }
    }
}

impl Default for OutputTruncateHook {
    fn default() -> Self {
        Self { max_chars: 8000 }
    }
}

#[async_trait::async_trait]
impl LifecycleHook for OutputTruncateHook {
    fn name(&self) -> &str { "OutputTruncateHook" }

    async fn post_tool(
        &self,
        _tool_name: &str,
        _args: &serde_json::Value,
        result: &str,
        _ctx: &HookContext,
    ) -> PostHookResult {
        if result.len() <= self.max_chars {
            return PostHookResult::default();
        }

        let head_size = self.max_chars * 2 / 3;
        let tail_size = self.max_chars / 3;

        // Use char-aware slicing to avoid panicking on UTF-8 boundaries
        let head: String = result.chars().take(head_size).collect();
        let tail: String = result.chars().rev().take(tail_size).collect::<String>().chars().rev().collect();
        let omitted = result.chars().count() - head.chars().count() - tail.chars().count();

        let truncated = format!(
            "{}\n\n... [TRUNCATED: {} chars omitted, {} total] ...\n\n{}",
            head,
            omitted,
            result.chars().count(),
            tail
        );

        log::debug!(
            "[OutputTruncate] {} → {} chars (saved {} chars)",
            result.chars().count(), truncated.chars().count(), omitted
        );

        PostHookResult {
            modified_result: Some(truncated),
            ..Default::default()
        }
    }
}

// ── SensitiveDataFilterHook ──

/// Redacts sensitive data (API keys, secrets, tokens) from tool results
/// before they are injected into the LLM context.
pub struct SensitiveDataFilterHook;

static SENSITIVE_PATTERNS: std::sync::LazyLock<Vec<(regex::Regex, &'static str)>> = std::sync::LazyLock::new(|| {
    vec![
        // Generic secret assignments: api_key = "xxxx", PASSWORD=xxxx, secret: xxxx
        (regex::Regex::new(r#"(?i)(api[_-]?key|secret|password|token|authorization)\s*[=:]\s*['"]?([A-Za-z0-9_\-/+=]{16,})['"]?"#)
            .expect("regex compilation failed"), "$1 = [REDACTED]"),
        // OpenAI-style keys
        (regex::Regex::new(r"\bsk-[A-Za-z0-9]{20,}")
            .expect("regex compilation failed"), "[REDACTED:OpenAI-key]"),
        // AWS access key IDs
        (regex::Regex::new(r"\bAKIA[0-9A-Z]{16}\b")
            .expect("regex compilation failed"), "[REDACTED:AWS-key]"),
        // Private key blocks
        (regex::Regex::new(r#"(?s)(-----BEGIN\s+(RSA\s+)?PRIVATE\s+KEY-----).*?(-----END\s+(RSA\s+)?PRIVATE\s+KEY-----)"#)
            .expect("regex compilation failed"), "$1 [REDACTED] $3"),
    ]
});

#[async_trait::async_trait]
impl LifecycleHook for SensitiveDataFilterHook {
    fn name(&self) -> &str { "SensitiveDataFilterHook" }

    async fn post_tool(
        &self,
        _tool_name: &str,
        _args: &serde_json::Value,
        result: &str,
        _ctx: &HookContext,
    ) -> PostHookResult {
        let mut filtered = result.to_string();
        let mut redaction_count = 0;

        for (pattern, replacement) in SENSITIVE_PATTERNS.iter() {
            if pattern.is_match(&filtered) {
                filtered = pattern.replace_all(&filtered, *replacement).to_string();
                redaction_count += 1;
            }
        }

        if redaction_count > 0 {
            log::info!("[SensitiveFilter] Redacted {} pattern(s) from tool result", redaction_count);
        }

        PostHookResult {
            modified_result: if filtered != result { Some(filtered) } else { None },
            ..Default::default()
        }
    }
}

// ── ErrorPatternHook ──

/// Detects when the LLM repeatedly fails on the same file and injects a
/// hint message to encourage trying a different approach.
pub struct ErrorPatternHook {
    /// Tracks recent tool call outcomes: (file_path, is_error)
    recent: Arc<std::sync::Mutex<VecDeque<(String, bool)>>>,
}

impl ErrorPatternHook {
    pub fn new() -> Self {
        Self {
            recent: Arc::new(std::sync::Mutex::new(VecDeque::with_capacity(6))),
        }
    }

    fn is_error_result(result: &str) -> bool {
        let r = result.to_lowercase();
        r.contains("error:") || r.contains("failed:") || r.contains("not found:")
            || r.contains("no such file") || r.contains("permission denied")
            || r.contains("mismatched types") || r.contains("cannot find")
    }

    fn extract_file_path(args: &serde_json::Value) -> Option<String> {
        args.get("file_path").and_then(|v| v.as_str()).map(|s| s.to_string())
    }
}

impl Default for ErrorPatternHook {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl LifecycleHook for ErrorPatternHook {
    fn name(&self) -> &str { "ErrorPatternHook" }

    async fn post_tool(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        result: &str,
        _ctx: &HookContext,
    ) -> PostHookResult {
        let file_path = match Self::extract_file_path(args) {
            Some(p) => p,
            None => return PostHookResult::default(),
        };

        let is_error = Self::is_error_result(result);

        {
            let mut recent = self.recent.lock().unwrap_or_else(|e| e.into_inner());
            recent.push_back((file_path.clone(), is_error));
            if recent.len() > 5 {
                recent.pop_front();
            }

            // Check: last 3 entries are all errors on the same file?
            if recent.len() >= 3 {
                let last_three: Vec<&(String, bool)> = recent.iter().rev().take(3).collect();
                let all_errors = last_three.iter().all(|(_, err)| *err);
                let same_file = last_three.windows(2).all(|w| w[0].0 == w[1].0);

                if all_errors && same_file {
                    log::warn!("[ErrorPattern] 3 consecutive failures on '{}': {}", file_path, tool_name);
                    return PostHookResult {
                        additional_messages: vec![llm::ChatMessage {
                            role: "user".into(),
                            content: format!(
                                "[SYSTEM-HINT] You have failed to operate on '{}' 3 times consecutively. \
                                 STOP retrying the same approach. Re-read the file to understand its current \
                                 content first, then try a different strategy.",
                                file_path
                            ),
                            images: None,
                            tool_calls: None,
                            tool_call_id: None,
                        }],
                        ..Default::default()
                    };
                }
            }
        }

        PostHookResult::default()
    }
}

// ── AuditLogHook ──

/// Persists every tool invocation and result to a JSONL audit log file
/// for post-mortem analysis and replay capability.
pub struct AuditLogHook {
    log_dir: Option<std::path::PathBuf>,
}

impl AuditLogHook {
    pub fn new(log_dir: Option<std::path::PathBuf>) -> Self {
        Self { log_dir }
    }
}

#[async_trait::async_trait]
impl LifecycleHook for AuditLogHook {
    fn name(&self) -> &str { "AuditLogHook" }

    async fn post_tool(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        result: &str,
        ctx: &HookContext,
    ) -> PostHookResult {
        let log_dir = match &self.log_dir {
            Some(d) => d,
            None => return PostHookResult::default(),
        };

        let log_path = log_dir.join(format!("agent_audit_{}.jsonl", ctx.session_id));

        let entry = serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "session": ctx.session_id,
            "agent": ctx.agent_id,
            "tool": tool_name,
            "args": args,
            "result_len": result.len(),
            "result_preview": result.chars().take(500).collect::<String>(),
            "is_error": ErrorPatternHook::is_error_result(result),
        });

        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            Ok(mut file) => {
                if let Err(e) = writeln!(file, "{}", entry.to_string()) {
                    log::warn!("[AuditLog] Failed to write: {}", e);
                }
            }
            Err(e) => {
                log::warn!("[AuditLog] Failed to open {}: {}", log_path.display(), e);
            }
        }

        PostHookResult::default()
    }
}

// ── FileChangeTrackerHook ──

/// Emits file-change events to the frontend after file-modifying tools execute,
/// enabling the UI to display a real-time change summary.
pub struct FileChangeTrackerHook;

#[async_trait::async_trait]
impl LifecycleHook for FileChangeTrackerHook {
    fn name(&self) -> &str { "FileChangeTrackerHook" }

    async fn post_tool(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        result: &str,
        ctx: &HookContext,
    ) -> PostHookResult {
        if !matches!(tool_name, "write_file" | "edit" | "append_file" | "delete_file") {
            return PostHookResult::default();
        }

        let file_path = args.get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        let success = !ErrorPatternHook::is_error_result(result);

        if let Some(app) = &ctx.app {
            let _ = app.emit("chat-event", serde_json::json!({
                "type": "file-changed",
                "session_id": ctx.session_id,
                "agent_id": ctx.agent_id,
                "tool": tool_name,
                "path": file_path,
                "success": success,
            }));
        }

        PostHookResult::default()
    }
}

// ── Build default HookManager with built-in hooks ──

/// Create a HookManager pre-loaded with all built-in hooks in the correct order:
///
/// **Pre-tool hooks:**
/// 1. SnapshotHook (saves files before modification)
/// 2. ConfirmHook (requests user confirmation for dangerous ops)
///
/// **Post-tool hooks (in order):**
/// 3. SensitiveDataFilterHook (redacts secrets from results)
/// 4. ErrorPatternHook (detects retry loops)
/// 5. AuditLogHook (persists audit trail to JSONL)
/// 6. OutputTruncateHook (truncates oversized results)
/// 7. FileChangeTrackerHook (emits change events to frontend)
///
/// **Post-tool-batch hooks:**
/// 8. AutoDiagnoseHook (runs diagnostics after file modifications)
pub fn build_default_hooks(
    app: tauri::AppHandle,
    session_id: String,
    agent_id: String,
    project_path: Option<String>,
    cancelled: Arc<AtomicBool>,
    file_snapshots: Arc<std::sync::Mutex<HashMap<String, String>>>,
    file_snapshot_store: Option<crate::commands::chat::FileSnapshotStore>,
    config_dir: Option<std::path::PathBuf>,
) -> (HookManager, HookContext) {
    let mut manager = HookManager::new();

    // Pre-tool hooks
    manager.register(SnapshotHook);
    manager.register(ConfirmHook);

    // Post-tool hooks (order matters: filter → detect → audit → truncate → track)
    manager.register(SensitiveDataFilterHook);
    manager.register(ErrorPatternHook::new());
    manager.register(AuditLogHook::new(config_dir));
    manager.register(OutputTruncateHook::default());
    manager.register(FileChangeTrackerHook);

    // Post-tool-batch hooks
    manager.register(AutoDiagnoseHook);

    let ctx = HookContext {
        app: Some(app),
        session_id,
        agent_id,
        project_path,
        cancelled,
        file_snapshots,
        file_snapshot_store,
    };

    (manager, ctx)
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    /// A test hook that records all hook calls for verification.
    struct RecordingHook {
        name: &'static str,
        calls: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl LifecycleHook for RecordingHook {
        fn name(&self) -> &str { self.name }

        async fn pre_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
            _ctx: &HookContext,
        ) -> HookResult {
            let mut calls = self.calls.lock().unwrap();
            calls.push(format!("pre_tool:{}", tool_name));
            HookResult::Continue
        }

        async fn post_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
            _result: &str,
            _ctx: &HookContext,
        ) -> PostHookResult {
            let mut calls = self.calls.lock().unwrap();
            calls.push(format!("post_tool:{}", tool_name));
            PostHookResult::default()
        }
    }

    /// A test hook that denies a specific tool.
    struct DenyHook {
        deny_tool: &'static str,
    }

    #[async_trait::async_trait]
    impl LifecycleHook for DenyHook {
        fn name(&self) -> &str { "DenyHook" }

        async fn pre_tool(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
            _ctx: &HookContext,
        ) -> HookResult {
            if tool_name == self.deny_tool {
                HookResult::Deny(format!("Denied by test hook: {}", tool_name))
            } else {
                HookResult::Continue
            }
        }
    }

    fn make_test_ctx() -> HookContext {
        // App handle is None in tests — hooks that need it will fall back gracefullyly
        HookContext {
            app: None,
            session_id: "test-session".into(),
            agent_id: "test-agent".into(),
            project_path: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            file_snapshots: Arc::new(std::sync::Mutex::new(HashMap::new())),
            file_snapshot_store: None,
        }
    }

    #[tokio::test]
    async fn test_hook_manager_pre_tool_continue() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut manager = HookManager::new();
        manager.register(RecordingHook { name: "hook1", calls: calls.clone() });
        manager.register(RecordingHook { name: "hook2", calls: calls.clone() });

        let ctx = make_test_ctx();
        let mut args = serde_json::json!({});
        let result = manager.pre_tool_chain("read_file", &mut args, &ctx).await;

        assert!(matches!(result, HookResult::Continue));
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], "pre_tool:read_file");
        assert_eq!(calls[1], "pre_tool:read_file");
    }

    #[tokio::test]
    async fn test_hook_manager_pre_tool_deny() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut manager = HookManager::new();
        manager.register(RecordingHook { name: "hook1", calls: calls.clone() });
        manager.register(DenyHook { deny_tool: "delete_file" });
        // This hook should NOT be called because DenyHook stops the chain
        manager.register(RecordingHook { name: "hook3", calls: calls.clone() });

        let ctx = make_test_ctx();
        let mut args = serde_json::json!({});
        let result = manager.pre_tool_chain("delete_file", &mut args, &ctx).await;

        assert!(matches!(result, HookResult::Deny(_)));
        let calls = calls.lock().unwrap();
        // hook1 ran, DenyHook stopped the chain, hook3 never ran
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], "pre_tool:delete_file");
    }

    #[tokio::test]
    async fn test_hook_chain_order() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut manager = HookManager::new();
        manager.register(RecordingHook { name: "first", calls: calls.clone() });
        manager.register(RecordingHook { name: "second", calls: calls.clone() });
        manager.register(RecordingHook { name: "third", calls: calls.clone() });

        let ctx = make_test_ctx();
        let mut args = serde_json::json!({});
        manager.pre_tool_chain("read_file", &mut args, &ctx).await;
        manager.post_tool_chain("read_file", &args, "ok", &ctx).await;

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 6); // 3 pre + 3 post
        assert_eq!(calls[0], "pre_tool:read_file");
        assert_eq!(calls[1], "pre_tool:read_file");
        assert_eq!(calls[2], "pre_tool:read_file");
        assert_eq!(calls[3], "post_tool:read_file");
        assert_eq!(calls[4], "post_tool:read_file");
        assert_eq!(calls[5], "post_tool:read_file");
    }

    // ── OutputTruncateHook tests ──

    #[tokio::test]
    async fn test_output_truncate_no_truncation_for_short_result() {
        let hook = OutputTruncateHook::new(100);
        let ctx = make_test_ctx();
        let args = serde_json::json!({});
        let result = "short output";

        let post = hook.post_tool("read_file", &args, result, &ctx).await;
        assert!(post.modified_result.is_none(), "short result should not be truncated");
    }

    #[tokio::test]
    async fn test_output_truncate_long_result() {
        let hook = OutputTruncateHook::new(100);
        let ctx = make_test_ctx();
        let args = serde_json::json!({});
        let result: String = (0..200).map(|i| (b'a' + (i % 26) as u8) as char).collect();

        let post = hook.post_tool("read_file", &args, &result, &ctx).await;
        assert!(post.modified_result.is_some(), "long result should be truncated");
        let truncated = post.modified_result.unwrap();
        assert!(truncated.contains("TRUNCATED"), "truncated output should contain marker");
        assert!(truncated.len() < result.len(), "truncated output should be shorter");
    }

    // ── SensitiveDataFilterHook tests ──

    #[tokio::test]
    async fn test_sensitive_filter_redacts_openai_key() {
        let hook = SensitiveDataFilterHook;
        let ctx = make_test_ctx();
        let args = serde_json::json!({});
        let result = "Config: sk-abcdefghijklmnopqrstuvwxyz1234 is loaded";

        let post = hook.post_tool("read_file", &args, result, &ctx).await;
        let output = post.modified_result.as_deref().unwrap_or(result);
        assert!(!output.contains("sk-abcdefghijklmnopqrstuvwxyz1234"), "OpenAI key should be redacted");
        assert!(output.contains("REDACTED"), "should contain redaction marker");
    }

    #[tokio::test]
    async fn test_sensitive_filter_redacts_generic_secret() {
        let hook = SensitiveDataFilterHook;
        let ctx = make_test_ctx();
        let args = serde_json::json!({});
        let result = "api_key = 'sk_live_abcdef1234567890abcdef'";

        let post = hook.post_tool("read_file", &args, result, &ctx).await;
        let output = post.modified_result.as_deref().unwrap_or(result);
        assert!(!output.contains("sk_live_abcdef1234567890abcdef"), "secret value should be redacted");
    }

    #[tokio::test]
    async fn test_sensitive_filter_no_false_positive() {
        let hook = SensitiveDataFilterHook;
        let ctx = make_test_ctx();
        let args = serde_json::json!({});
        let result = "This is a normal log message with no secrets";

        let post = hook.post_tool("read_file", &args, result, &ctx).await;
        assert!(post.modified_result.is_none(), "normal text should not be modified");
    }

    // ── ErrorPatternHook tests ──

    #[tokio::test]
    async fn test_error_pattern_no_warning_on_success() {
        let hook = ErrorPatternHook::new();
        let ctx = make_test_ctx();
        let args = serde_json::json!({ "file_path": "main.rs" });
        let result = "File written successfully";

        let post = hook.post_tool("write_file", &args, result, &ctx).await;
        assert!(post.additional_messages.is_empty(), "success should not trigger warning");
    }

    #[tokio::test]
    async fn test_error_pattern_warns_after_3_consecutive_failures() {
        let hook = ErrorPatternHook::new();
        let ctx = make_test_ctx();
        let args = serde_json::json!({ "file_path": "main.rs" });

        // First two failures — no warning yet
        for _ in 0..2 {
            let post = hook.post_tool("edit", &args, "error: mismatched types", &ctx).await;
            assert!(post.additional_messages.is_empty(), "should not warn after < 3 failures");
        }

        // Third consecutive failure on same file — warning!
        let post = hook.post_tool("edit", &args, "error: mismatched types", &ctx).await;
        assert_eq!(post.additional_messages.len(), 1, "should warn after 3 consecutive failures");
        assert!(
            post.additional_messages[0].content.contains("STOP retrying"),
            "warning should tell LLM to change strategy"
        );
    }

    #[tokio::test]
    async fn test_error_pattern_no_warning_for_different_files() {
        let hook = ErrorPatternHook::new();
        let ctx = make_test_ctx();

        // Errors on different files — no warning
        for file in &["a.rs", "b.rs", "c.rs"] {
            let args = serde_json::json!({ "file_path": file });
            let post = hook.post_tool("edit", &args, "error: not found", &ctx).await;
            assert!(post.additional_messages.is_empty(), "different files should not trigger warning");
        }
    }

    // ── AuditLogHook tests ──

    #[tokio::test]
    async fn test_audit_log_writes_to_file() {
        let tmp_dir = std::env::temp_dir().join("neecoder_test_audit");
        let _ = std::fs::create_dir_all(&tmp_dir);

        let hook = AuditLogHook::new(Some(tmp_dir.clone()));
        let ctx = make_test_ctx();
        let args = serde_json::json!({ "file_path": "test.rs" });
        let result = "File written successfully";

        let post = hook.post_tool("write_file", &args, result, &ctx).await;
        assert!(post.modified_result.is_none(), "audit hook should not modify result");
        assert!(post.additional_messages.is_empty(), "audit hook should not inject messages");

        // Verify log file was created
        let log_path = tmp_dir.join("agent_audit_test-session.jsonl");
        assert!(log_path.exists(), "audit log file should exist");

        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("write_file"), "log should contain tool name");
        assert!(content.contains("test.rs"), "log should contain file path");

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[tokio::test]
    async fn test_audit_log_no_dir_graceful() {
        let hook = AuditLogHook::new(None);
        let ctx = make_test_ctx();
        let args = serde_json::json!({});
        let result = "ok";

        // Should not panic when log_dir is None
        let post = hook.post_tool("read_file", &args, result, &ctx).await;
        assert!(post.modified_result.is_none());
    }

    // ── FileChangeTrackerHook tests ──

    #[tokio::test]
    async fn test_file_change_tracker_ignores_non_file_tools() {
        let hook = FileChangeTrackerHook;
        let ctx = make_test_ctx();
        let args = serde_json::json!({ "query": "test" });
        let result = "search results";

        let post = hook.post_tool("search_codebase", &args, result, &ctx).await;
        assert!(post.modified_result.is_none());
        assert!(post.additional_messages.is_empty());
    }

    #[tokio::test]
    async fn test_file_change_tracker_handles_file_tools() {
        let hook = FileChangeTrackerHook;
        let ctx = make_test_ctx(); // app = None, so emit won't fire — but should not panic
        let args = serde_json::json!({ "file_path": "main.rs" });
        let result = "File written successfully";

        let post = hook.post_tool("write_file", &args, result, &ctx).await;
        assert!(post.modified_result.is_none(), "tracker should not modify result");
        assert!(post.additional_messages.is_empty(), "tracker should not inject messages");
    }
}
