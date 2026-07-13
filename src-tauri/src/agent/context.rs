//! Context compaction strategy for managing long conversation histories.
//!
//! When the message history exceeds a token budget threshold, the middle
//! portion is summarized by the LLM and replaced with a compact summary message.
//! This preserves the task description (first user message) and recent context
//! (last N messages) while compressing older interactions.

use crate::agent::token_count;
use crate::config::LlmProvider;
use crate::llm;

/// Threshold (fraction of max_context_tokens) that triggers compaction.
const COMPACT_THRESHOLD: f64 = 0.80;

/// Number of recent messages to always preserve (increased from 4 for tool-call safety).
const PRESERVE_RECENT: usize = 6;

/// Minimum messages required before compaction is considered.
const MIN_MESSAGES_FOR_COMPACT: usize = 8;

/// Maximum summary characters (truncation for token safety).
const COMPACTION_MAX_SUMMARY_CHARS: usize = 2000;

/// Maximum characters of source conversation text fed to the LLM.
const COMPACTION_MAX_SOURCE_CHARS: usize = 12000;

/// Maximum number of persistent facts extracted during pre-compaction flush.
const COMPACTION_MAX_FLUSH_FACTS: usize = 8;

/// Summarize the middle portion of messages when total tokens exceed the budget.
///
/// Returns the (possibly compacted) message list. If compaction was performed,
/// the returned list will be shorter with a summary message injected.
///
/// Strategy:
/// 1. Calculate total tokens using precise tiktoken counting.
/// 2. If total <= 80% of budget → return unchanged.
/// 3. Preserve: first user message + last 4 messages.
/// 4. Compress: everything in between → LLM summary.
/// 5. Return: [first_user_msg, summary_msg, ...recent_messages]
pub async fn compact_if_needed(
    messages: &[llm::ChatMessage],
    system_prompt: &str,
    max_context_tokens: usize,
    provider: &LlmProvider,
    api_key: &str,
    base_url: Option<&str>,
    model: &str,
) -> Result<Vec<llm::ChatMessage>, String> {
    if messages.len() < MIN_MESSAGES_FOR_COMPACT {
        return Ok(messages.to_vec());
    }

    let total_tokens = token_count::estimate_total_tokens(messages, system_prompt, model);
    let threshold = (max_context_tokens as f64 * COMPACT_THRESHOLD) as usize;

    if total_tokens <= threshold {
        return Ok(messages.to_vec());
    }

    log::info!(
        "[Context] Compaction triggered: {} tokens exceeds 80% of {} budget (threshold={})",
        total_tokens, max_context_tokens, threshold
    );

    // Find the first user message (task description)
    let first_user_idx = messages.iter().position(|m| m.role == "user");
    let first_user_idx = match first_user_idx {
        Some(idx) => idx,
        None => return Ok(messages.to_vec()), // No user message found, skip
    };

    // Determine the compaction boundaries
    let preserve_recent_start = if messages.len() > PRESERVE_RECENT {
        let mut boundary = messages.len() - PRESERVE_RECENT;
        // ── Tool-call safety boundary ──
        // Ensure we don't split an assistant(tool_calls) → tool response chain.
        // If the boundary falls on a tool message, walk backward until we find
        // the preceding assistant message (with tool_calls) and include both.
        if boundary < messages.len() {
            // Walk backward: if current msg is a tool response, include its assistant
            let mut safety = boundary;
            while safety > first_user_idx {
                let msg = &messages[safety];
                if msg.role == "tool" {
                    // Move boundary back to include the assistant that called this tool
                    safety = safety.saturating_sub(1);
                    continue;
                } else if msg.role == "assistant" && msg.tool_calls.is_some() {
                    // Assistant with tool_calls — its tool responses are after it,
                    // so we need to move forward past all of them.
                    // Actually, if we're at an assistant(tool_calls) that is before
                    // the original boundary, we should push the boundary past its
                    // tool results to keep the chain intact.
                    // But this means we'd include more messages than PRESERVE_RECENT.
                    // The safest approach: if the current boundary is inside a
                    // tool-call chain, extend backward to include the assistant.
                    let chain_start = safety;
                    // Walk forward from assistant to find where tool responses end
                    let mut chain_end = chain_start + 1;
                    while chain_end < messages.len() && messages[chain_end].role == "tool" {
                        chain_end += 1;
                    }
                    // If the original boundary falls within this chain, adjust it
                    if boundary < chain_end && boundary > chain_start {
                        // Tool chain extends past the boundary — we have two options:
                        // 1. Pull boundary back to chain_start (include assistant+all tools)
                        // 2. Push boundary forward past chain_end (exclude all)
                        // We choose option 1 for safety: include the whole chain
                        boundary = chain_start;
                        break;
                    }
                    break;
                }
                break;
            }
        }
        boundary
    } else {
        return Ok(messages.to_vec()); // Not enough messages to compact
    };

    // Middle section to summarize: (first_user_idx+1)..preserve_recent_start
    let middle_start = first_user_idx + 1;
    let middle_end = preserve_recent_start;

    if middle_start >= middle_end {
        return Ok(messages.to_vec()); // Nothing to compact
    }

    let middle_messages = &messages[middle_start..middle_end];

    // Build the summary prompt (with source truncation)
    let mut conversation_text = String::new();
    for msg in middle_messages {
        let role_label = match msg.role.as_str() {
            "assistant" => "Assistant",
            "tool" => "Tool",
            "user" => "User",
            _ => "System",
        };
        let content_preview = if msg.content.len() > 2000 {
            format!("{}...", crate::agent::utils::safe_truncate(&msg.content, 2000))
        } else {
            msg.content.clone()
        };
        let line = format!("[{}]: {}\n\n", role_label, content_preview);
        // Enforce source text budget
        if conversation_text.len() + line.len() > COMPACTION_MAX_SOURCE_CHARS {
            conversation_text.push_str("... [truncated]");
            break;
        }
        conversation_text.push_str(&line);
    }

    // Pre-compaction memory flush: extract persistent facts before summarization
    let mut flush_facts: Vec<String> = Vec::new();
    for msg in middle_messages {
        if flush_facts.len() >= COMPACTION_MAX_FLUSH_FACTS {
            break;
        }
        for line in msg.content.lines() {
            let trimmed = line.trim();
            if (trimmed.contains("[Lesson]") || trimmed.contains("[Decision]")) && trimmed.len() > 10 {
                if !flush_facts.contains(&trimmed.to_string()) {
                    flush_facts.push(trimmed.to_string());
                    if flush_facts.len() >= COMPACTION_MAX_FLUSH_FACTS {
                        break;
                    }
                }
            }
        }
    }

    let flush_context = if !flush_facts.is_empty() {
        format!("\n\nKey learnings extracted from this conversation (reference these):\n{}\n",
            flush_facts.iter().map(|f| format!("- {}", f)).collect::<Vec<_>>().join("\n"))
    } else {
        String::new()
    };

    let summary_prompt = format!(
        "You are a conversation compaction engine. Summarize this coding session.\n\n\
         Output at most 12 bullet points covering:\n\
         1. What was the user's goal and what was accomplished\n\
         2. Key technical decisions and their rationale\n\
         3. Files created, modified, or deleted (include exact paths)\n\
         4. Errors encountered and how they were resolved\n\
         5. Current task state — what's done, what's pending\n\
         6. Important context the assistant needs to continue\n\n\
         Rules:\n\
         - Be concise. Each bullet point must be one sentence.\n\
         - Do NOT restate the conversation verbatim. Synthesize.\n\
         - Omit trivial tool outputs (file reads, directory listings).\n\
         - Output ONLY the bullet points, no preamble or closing.{}\n\n\
         Conversation:\n{}",
        flush_context, conversation_text
    );

    let request = llm::ChatRequestParams {
        model: model.to_string(),
        messages: vec![llm::ChatMessage {
            role: "user".into(),
            content: summary_prompt,
            images: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        system: "You are a context compression assistant. Summarize coding conversations \
                 concisely while preserving all critical information (file paths, decisions, \
                 errors, task progress). Output only bullet points, no preamble."
            .into(),
        max_tokens: 512,
        temperature: 0.2,
        thinking_enabled: false,
        thinking_budget: 0,
    };

    let mut summary = match llm::chat_with_tools(provider, api_key, base_url, request, &[], None).await
    {
        Ok((llm::LlmResponse::Text(text), _usage)) => text,
        Ok((llm::LlmResponse::ToolCalls { content, .. }, _usage)) => {
            content.unwrap_or_else(|| "[Compaction failed: unexpected tool response]".into())
        }
        Err(e) => {
            log::warn!("[Context] Compaction LLM call failed: {}", e);
            // Fallback: just drop middle messages without summary
            let mut result = Vec::new();
            result.push(messages[first_user_idx].clone());
            result.extend_from_slice(&messages[preserve_recent_start..]);
            // Sanitize to remove orphaned tool messages
            return Ok(llm::sanitize_messages(&result));
        }
    };

    // Truncate summary to safety limit
    if summary.len() > COMPACTION_MAX_SUMMARY_CHARS {
        // Find last complete sentence or bullet point within the limit
        let truncate_at = if let Some(cutoff) = summary[..COMPACTION_MAX_SUMMARY_CHARS]
            .rfind(|c: char| c == '.' || c == '\n') {
            cutoff + 1
        } else {
            COMPACTION_MAX_SUMMARY_CHARS
        };
        summary = format!("{}...", &summary[..truncate_at]);
    }

    let tokens_after = {
        let summary_tokens = token_count::count_tokens(&summary, model);
        let first_user_tokens = token_count::estimate_message_tokens(&messages[first_user_idx], model);
        let recent_tokens: usize = messages[preserve_recent_start..]
            .iter()
            .map(|m| token_count::estimate_message_tokens(m, model))
            .sum();
        let system_tokens = token_count::count_tokens(system_prompt, model) + 4;
        system_tokens + first_user_tokens + summary_tokens + recent_tokens + 3
    };

    log::info!(
        "[Context] Compaction complete: {} → {} tokens (removed {} middle messages)",
        total_tokens, tokens_after, middle_end - middle_start
    );

    // Build the compacted message list
    let mut result = Vec::new();

    // 1. First user message (task description)
    result.push(messages[first_user_idx].clone());

    // 2. Summary message
    result.push(llm::ChatMessage {
        role: "system".into(),
        content: format!(
            "[CONTEXT COMPACTED] The following is a summary of the previous conversation:\n\n{}",
            summary
        ),
        images: None,
        tool_calls: None,
        tool_call_id: None,
    });

    // 3. Recent messages (preserved)
    result.extend_from_slice(&messages[preserve_recent_start..]);

    // Sanitize: remove orphaned tool messages that may result from compaction
    // boundary cutting through an assistant(tool_calls) → tool chain
    Ok(llm::sanitize_messages(&result))
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

    #[test]
    fn test_compact_messages_under_budget() {
        // Messages well under the budget should not be compacted
        let messages = vec![
            make_msg("user", "Hello"),
            make_msg("assistant", "Hi there!"),
        ];

        // Use a large budget so no compaction is needed
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(compact_if_needed(
            &messages,
            "You are an assistant",
            100_000, // huge budget
            &LlmProvider::OpenAI,
            "fake-key",
            Some("http://localhost:9999"),
            "gpt-4o",
        ));

        let result = result.unwrap();
        assert_eq!(result.len(), 2, "Should not compact when under budget");
    }

    #[test]
    fn test_compact_messages_too_few() {
        // Fewer than MIN_MESSAGES_FOR_COMPACT should not trigger compaction
        let messages: Vec<llm::ChatMessage> = (0..5)
            .map(|i| make_msg(if i % 2 == 0 { "user" } else { "assistant" }, &format!("msg {}", i)))
            .collect();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(compact_if_needed(
            &messages,
            "system",
            100, // tiny budget, but too few messages
            &LlmProvider::OpenAI,
            "fake-key",
            Some("http://localhost:9999"),
            "gpt-4o",
        ));

        let result = result.unwrap();
        assert_eq!(result.len(), 5, "Should not compact with too few messages");
    }
}
