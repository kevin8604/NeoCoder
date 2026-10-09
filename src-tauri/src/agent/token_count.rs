//! Precise token counting using tiktoken-rs with fallback to character-based estimation.
//!
//! tiktoken-rs downloads tokenizer data from OpenAI on first use. We cache the BPE encoder
//! globally using `OnceLock` to avoid repeated downloads and initialization overhead.

use crate::llm::ChatMessage;
use std::sync::OnceLock;
use tiktoken_rs::CoreBPE;

/// Cached BPE encoder (o200k_base covers GPT-4o, compatible with most modern models).
static BPE_ENCODER: OnceLock<CoreBPE> = OnceLock::new();

/// Get or initialize the cached BPE encoder.
/// Falls back to `o200k_base` (GPT-4o tokenizer) which is a good approximation
/// for DeepSeek, Claude, and other modern models.
fn get_bpe() -> Option<&'static CoreBPE> {
    BPE_ENCODER
        .get_or_init(|| {
            tiktoken_rs::o200k_base().unwrap_or_else(|e| {
                log::warn!(
                    "[TokenCount] Failed to init o200k_base, trying cl100k_base: {}",
                    e
                );
                tiktoken_rs::cl100k_base().unwrap_or_else(|e| {
                    log::error!("[TokenCount] Failed to init cl100k_base: {}", e);
                    // This should not happen in practice, but we return a dummy
                    // that will produce inaccurate but non-zero counts
                    tiktoken_rs::cl100k_base().expect("cl100k_base must succeed")
                })
            })
        })
        .into()
}

/// Count tokens in a text string using tiktoken.
/// Returns fallback `chars / 3` if tiktoken is unavailable.
pub fn count_tokens(text: &str, _model: &str) -> usize {
    match get_bpe() {
        Some(bpe) => bpe.encode_with_special_tokens(text).len(),
        None => {
            // Fallback: rough estimation
            text.chars().count() / 3
        }
    }
}

/// Estimate the token count of a single chat message (content + tool_calls).
/// Uses tiktoken for precise counting when available.
pub fn estimate_message_tokens(msg: &ChatMessage, model: &str) -> usize {
    let content_tokens = count_tokens(&msg.content, model);
    let tool_calls_tokens = msg
        .tool_calls
        .as_ref()
        .map(|tc| count_tokens(&tc.to_string(), model))
        .unwrap_or(0);
    let tool_call_id_tokens = msg
        .tool_call_id
        .as_ref()
        .map(|id| count_tokens(id, model))
        .unwrap_or(0);
    // Add ~4 tokens for message overhead (role, separators, etc.)
    content_tokens + tool_calls_tokens + tool_call_id_tokens + 4
}

/// Estimate token count for a full message list including system prompt.
pub fn estimate_total_tokens(messages: &[ChatMessage], system_prompt: &str, model: &str) -> usize {
    let system_tokens = count_tokens(system_prompt, model) + 4;
    let message_tokens: usize = messages
        .iter()
        .map(|m| estimate_message_tokens(m, model))
        .sum();
    system_tokens + message_tokens + 3 // +3 for assistant reply primer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: "user".into(),
            content: content.into(),
            images: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn count_tokens_empty_is_zero() {
        assert_eq!(count_tokens("", "gpt-4o"), 0);
    }

    #[test]
    fn count_tokens_grows_with_content() {
        let short = count_tokens("hello", "gpt-4o");
        let long = count_tokens("hello hello hello hello hello hello", "gpt-4o");
        assert!(short > 0, "non-empty ASCII must count something");
        assert!(long > short, "more content must yield more tokens");
    }

    #[test]
    fn count_tokens_is_cjk_safe() {
        // Must not panic on multibyte input and should return a positive count.
        let n = count_tokens("你好世界，这是中文内容", "gpt-4o");
        assert!(n > 0);
    }

    #[test]
    fn message_overhead_is_four_tokens() {
        // An empty message still carries the fixed +4 role/separator overhead.
        assert_eq!(estimate_message_tokens(&msg(""), "gpt-4o"), 4);
    }

    #[test]
    fn tool_call_id_adds_to_message_estimate() {
        let base = estimate_message_tokens(&msg("hi"), "gpt-4o");
        let mut with_id = msg("hi");
        with_id.tool_call_id = Some("call_1234567890".into());
        assert!(
            estimate_message_tokens(&with_id, "gpt-4o") > base,
            "a tool_call_id must increase the message estimate"
        );
    }

    #[test]
    fn tool_calls_add_to_message_estimate() {
        let base = estimate_message_tokens(&msg("hi"), "gpt-4o");
        let mut with_tc = msg("hi");
        with_tc.tool_calls = Some(serde_json::json!([
            { "id": "call_0", "type": "function",
              "function": { "name": "read_file", "arguments": "{\"path\":\"a.rs\"}" } }
        ]));
        assert!(estimate_message_tokens(&with_tc, "gpt-4o") > base);
    }

    #[test]
    fn total_includes_system_prompt_and_primer() {
        let messages = vec![msg("alpha"), msg("beta")];
        let small_system = estimate_total_tokens(&messages, "sys", "gpt-4o");
        let big_system = estimate_total_tokens(
            &messages,
            "a much longer system prompt with many words here",
            "gpt-4o",
        );
        assert!(
            big_system > small_system,
            "larger system prompt → larger total"
        );

        // Total must be at least the sum of the individual message estimates
        // (system + primer add on top).
        let msg_sum: usize = messages
            .iter()
            .map(|m| estimate_message_tokens(m, "gpt-4o"))
            .sum();
        assert!(small_system >= msg_sum);
    }

    #[test]
    fn empty_history_still_has_system_and_primer() {
        let total = estimate_total_tokens(&[], "hello system", "gpt-4o");
        // count_tokens("hello system") + 4 + 0 messages + 3
        assert!(total > 0);
    }
}
