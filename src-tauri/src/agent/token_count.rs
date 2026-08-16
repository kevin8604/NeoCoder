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
