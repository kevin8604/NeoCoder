//! Task-progress summarizer for long-running agents.
//!
//! Proactive companion to `context::compact_if_needed`:
//! - `compact_if_needed`: reactive, emergency compaction at ~80% of the token
//!   budget — it *deletes* the middle of the conversation.
//! - `task_summarizer`: proactive compression at a lower threshold (~55% of
//!   budget, or 30k tokens for large budgets), *replacing* older messages with
//!   a structured "task progress" summary so completed steps and current state
//!   survive instead of being lost.
//!
//! Routing: uses the LLM Router with `TaskType::Dreaming` — local Ollama model
//! first (free + private), automatic fallback to the remote provider.

use crate::agent::token_count;
use crate::config::{LlmProvider, LocalModelConfig};
use crate::llm;
use crate::llm::health;
use crate::llm::router::{LlmRouter, TaskType};

/// Absolute token floor — summarization happens no later than this (for large budgets).
pub const SUMMARY_TOKEN_THRESHOLD: usize = 30_000;

/// Fraction of the context budget that triggers proactive summarization.
const TRIGGER_FRACTION: f64 = 0.55;

/// Minimum messages before summarization is considered.
const MIN_MESSAGES_FOR_SUMMARY: usize = 10;

/// Number of recent messages to always preserve.
const PRESERVE_RECENT: usize = 8;

/// Maximum summary characters (truncation for token safety).
const MAX_SUMMARY_CHARS: usize = 2500;

/// Maximum characters of source conversation text fed to the LLM.
const MAX_SOURCE_CHARS: usize = 16000;

/// Result of a summarization attempt.
pub struct SummarizeOutcome {
    /// The (possibly rewritten) message list.
    pub messages: Vec<llm::ChatMessage>,
    /// Whether a summary was produced and injected.
    pub performed: bool,
    /// The generated summary text (empty if not performed).
    pub summary: String,
}

/// Build a "task progress" summary prompt from the middle messages.
fn build_progress_prompt(middle: &[llm::ChatMessage]) -> String {
    let mut conversation_text = String::new();
    for msg in middle {
        let role_label = match msg.role.as_str() {
            "assistant" => "Assistant",
            "tool" => "Tool",
            "user" => "User",
            _ => "System",
        };
        let content_preview = if msg.content.len() > 1500 {
            format!("{}...", crate::agent::utils::safe_truncate(&msg.content, 1500))
        } else {
            msg.content.clone()
        };
        let line = format!("[{}]: {}\n\n", role_label, content_preview);
        if conversation_text.len() + line.len() > MAX_SOURCE_CHARS {
            conversation_text.push_str("... [truncated]");
            break;
        }
        conversation_text.push_str(&line);
    }

    format!(
        "You are a task-progress tracking engine for a coding agent. Summarize what happened \
         so far in this long-running task, as a compact status report.\n\n\
         Output at most 10 bullet points covering:\n\
         1. Task goal and current status (done / in-progress / blocked)\n\
         2. Completed steps and what they produced\n\
         3. Files created or modified (include exact paths) and key changes\n\
         4. Current step / what remains to be done\n\
         5. Errors encountered and how they were resolved\n\
         6. Important decisions or constraints the agent must remember\n\n\
         Rules:\n\
         - Be concise. Each bullet point must be one sentence.\n\
         - Do NOT restate conversation verbatim. Synthesize.\n\
         - Omit trivial tool outputs (file reads, directory listings, command logs).\n\
         - Keep file paths and error messages exact.\n\
         - Output ONLY the bullet points, no preamble or closing.\n\n\
         Conversation so far:\n{}",
        conversation_text
    )
}

/// Summarize older messages into a task-progress report when the context is
/// approaching the token budget (before the emergency compaction threshold).
///
/// On LLM failure the conversation is returned unchanged — summarization is an
/// enhancement, never a destructive step.
pub async fn summarize_task_progress_if_needed(
    messages: &[llm::ChatMessage],
    system_prompt: &str,
    max_context_tokens: usize,
    provider: &LlmProvider,
    api_key: &str,
    base_url: Option<&str>,
    model: &str,
    local: &LocalModelConfig,
) -> SummarizeOutcome {
    let unchanged = SummarizeOutcome {
        messages: messages.to_vec(),
        performed: false,
        summary: String::new(),
    };

    if messages.len() < MIN_MESSAGES_FOR_SUMMARY {
        return unchanged;
    }

    let total_tokens = token_count::estimate_total_tokens(messages, system_prompt, model);

    // Proactive threshold: 55% of budget, but never later than 30k tokens
    // (so huge budgets don't wait forever before progress is captured).
    let fraction_threshold = (max_context_tokens as f64 * TRIGGER_FRACTION) as usize;
    let threshold = fraction_threshold.min(SUMMARY_TOKEN_THRESHOLD);

    if total_tokens < threshold {
        return unchanged;
    }

    log::info!(
        "[TaskSummarizer] Triggered: {} tokens >= threshold {} (budget {})",
        total_tokens, threshold, max_context_tokens
    );

    // Locate boundaries: keep first user message (task description) + recent tail.
    let first_user_idx = match messages.iter().position(|m| m.role == "user") {
        Some(idx) => idx,
        None => return unchanged,
    };
    let middle_start = first_user_idx + 1;
    let middle_end = if messages.len() > PRESERVE_RECENT {
        let mut boundary = messages.len() - PRESERVE_RECENT;
        // ── Tool-call safety boundary ──
        // Never split an assistant(tool_calls) → tool response chain.
        while boundary > middle_start && boundary < messages.len() {
            if messages[boundary].role == "tool" || messages[boundary].role == "assistant" {
                // Walk back until we find the assistant that owns this chain,
                // then keep everything from there on.
                let mut start = boundary;
                while start > middle_start && (messages[start].role == "tool"
                    || (messages[start].role == "assistant" && messages[start].tool_calls.is_some()))
                {
                    start -= 1;
                }
                boundary = start;
                break;
            }
            break;
        }
        boundary
    } else {
        return unchanged;
    };

    if middle_start >= middle_end {
        return unchanged; // Nothing worth summarizing
    }

    let middle_messages = &messages[middle_start..middle_end];
    let summary_prompt = build_progress_prompt(middle_messages);

    // Route via Dreaming: local Ollama first (free + private), fallback remote
    let local_available = health::check_ollama(&local.base_url).await.running;
    let route = LlmRouter::route(
        TaskType::Dreaming,
        local,
        local_available,
        provider,
        api_key,
        model,
    );

    let request = llm::ChatRequestParams {
        model: route.model.clone(),
        messages: vec![llm::ChatMessage {
            role: "user".into(),
            content: summary_prompt,
            images: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        system: "You are a task-progress tracking assistant. Summarize the agent's progress \
                 concisely while preserving all critical information (file paths, decisions, \
                 errors, remaining work). Output only bullet points, no preamble."
            .into(),
        max_tokens: route.max_tokens,
        temperature: route.temperature,
        thinking_enabled: false,
        thinking_budget: 0,
    };

    let mut summary = match llm::chat_with_tools(
        &route.provider,
        &route.api_key,
        route.base_url.as_deref(),
        request,
        &[],
        None,
    )
    .await
    {
        Ok((llm::LlmResponse::Text(text), _usage)) => text,
        Ok((llm::LlmResponse::ToolCalls { content, .. }, _usage)) => {
            content.unwrap_or_default()
        }
        Err(e) => {
            log::warn!("[TaskSummarizer] LLM call failed ({:?}): {}", route.provider, e);
            return unchanged; // Never degrade destructively
        }
    };

    // Truncate summary to a safe length (prefer whole lines).
    if summary.len() > MAX_SUMMARY_CHARS {
        let cutoff = summary[..MAX_SUMMARY_CHARS]
            .rfind('\n')
            .unwrap_or(MAX_SUMMARY_CHARS);
        summary = format!("{}...", &summary[..cutoff]);
    }

    // Rebuild: [first user msg, progress summary, ...recent messages]
    let mut result = Vec::with_capacity(middle_messages.len() + PRESERVE_RECENT + 2);
    result.push(messages[first_user_idx].clone());
    result.push(llm::ChatMessage {
        role: "system".into(),
        content: format!(
            "[TASK PROGRESS] Summary of completed steps (continue from here):\n\n{}",
            summary
        ),
        images: None,
        tool_calls: None,
        tool_call_id: None,
    });
    result.extend_from_slice(&messages[middle_end..]);

    let removed = middle_messages.len();
    log::info!(
        "[TaskSummarizer] Replaced {} middle messages with task-progress summary ({} chars)",
        removed, summary.len()
    );

    SummarizeOutcome {
        messages: llm::sanitize_messages(&result),
        performed: true,
        summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(role: &str, content: &str) -> llm::ChatMessage {
        llm::ChatMessage {
            role: role.into(),
            content: content.into(),
            images: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn make_local() -> LocalModelConfig {
        LocalModelConfig {
            enabled: false,
            base_url: "http://localhost:11434".into(),
            dreaming_model: "qwen2.5:3b".into(),
            inference_model: String::new(),
            embedding_model: String::new(),
        }
    }

    #[test]
    fn test_threshold_calculation_for_large_budget() {
        // Large budget (100k) → 30k absolute floor applies
        let fraction = (100_000.0 * TRIGGER_FRACTION) as usize;
        let threshold = fraction.min(SUMMARY_TOKEN_THRESHOLD);
        assert_eq!(threshold, SUMMARY_TOKEN_THRESHOLD);

        // Small budget (40k) → 55% fraction applies (22k)
        let fraction = (40_000.0 * TRIGGER_FRACTION) as usize;
        let threshold = fraction.min(SUMMARY_TOKEN_THRESHOLD);
        assert_eq!(threshold, 22_000);
    }

    #[test]
    fn test_too_few_messages_skips() {
        let messages: Vec<llm::ChatMessage> = (0..5)
            .map(|i| make_msg(if i % 2 == 0 { "user" } else { "assistant" }, &format!("m{}", i)))
            .collect();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let outcome = rt.block_on(summarize_task_progress_if_needed(
            &messages,
            "system",
            40_000,
            &LlmProvider::OpenAI,
            "fake-key",
            Some("http://localhost:9999"),
            "gpt-4o",
            &make_local(),
        ));
        assert!(!outcome.performed);
        assert_eq!(outcome.messages.len(), 5);
    }

    #[test]
    fn test_under_threshold_skips() {
        // 10 small messages ≈ far below any threshold
        let messages: Vec<llm::ChatMessage> = (0..10)
            .map(|i| make_msg(if i % 2 == 0 { "user" } else { "assistant" }, "hi"))
            .collect();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let outcome = rt.block_on(summarize_task_progress_if_needed(
            &messages,
            "system",
            100_000,
            &LlmProvider::OpenAI,
            "fake-key",
            Some("http://localhost:9999"),
            "gpt-4o",
            &make_local(),
        ));
        assert!(!outcome.performed);
        assert_eq!(outcome.messages.len(), 10);
    }

    #[test]
    fn test_llm_failure_is_non_destructive() {
        // Tiny budget forces trigger; unreachable LLM must not destroy messages
        let mut messages: Vec<llm::ChatMessage> = Vec::new();
        messages.push(make_msg("user", "Implement feature X"));
        for i in 0..12 {
            messages.push(make_msg("assistant", &format!("Step {} executed...", i)));
        }
        let rt = tokio::runtime::Runtime::new().unwrap();
        let outcome = rt.block_on(summarize_task_progress_if_needed(
            &messages,
            "system",
            1000,
            &LlmProvider::OpenAI,
            "fake-key",
            Some("http://localhost:1"), // unreachable
            "gpt-4o",
            &make_local(),
        ));
        assert!(!outcome.performed);
        assert_eq!(outcome.messages.len(), messages.len(), "Messages must be untouched on failure");
    }

    #[test]
    fn test_build_progress_prompt_contains_roles() {
        let middle = vec![make_msg("assistant", "Edited src/lib.rs"), make_msg("tool", "ok")];
        let prompt = build_progress_prompt(&middle);
        assert!(prompt.contains("[Assistant]"));
        assert!(prompt.contains("Edited src/lib.rs"));
        assert!(prompt.contains("[Tool]"));
        assert!(prompt.contains("Task goal and current status"));
    }
}
