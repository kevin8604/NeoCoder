//! Model pricing and USD cost estimation.
//!
//! The agent already accumulates real `prompt_tokens` / `completion_tokens`
//! per session (from the provider's `TokenUsage`). This module turns those
//! counts into a dollar figure using a small, conservative price table, so the
//! cost of a session — and the savings from model routing — becomes visible.
//!
//! Prices are **per million tokens**, approximate, and intentionally incomplete:
//! unknown models return `None` (treated as $0) rather than a guessed number,
//! so the dashboard never shows a fabricated cost. Local models (Ollama) and
//! the coding models that run locally are $0 by definition.

/// (input, output) price per million tokens, in USD.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PricePerMtok {
    pub input: f64,
    pub output: f64,
}

/// Known model prices. Kept as data so the table is easy to audit/extend.
pub fn price_for(model: &str) -> Option<PricePerMtok> {
    // Exact-name table for the common hosted models. Match case-insensitively
    // because settings store free-form model strings.
    let m = model.to_ascii_lowercase();
    let price = |input: f64, output: f64| Some(PricePerMtok { input, output });
    match m.as_str() {
        // DeepSeek
        "deepseek-chat" | "deepseek-v3" => price(0.27, 1.10),
        "deepseek-reasoner" | "deepseek-r1" => price(0.55, 2.19),
        // OpenAI
        "gpt-4o" => price(2.50, 10.00),
        "gpt-4o-mini" => price(0.15, 0.60),
        "gpt-4" | "gpt-4-turbo" => price(10.00, 30.00),
        "o1" | "o1-preview" => price(15.00, 60.00),
        "o3-mini" => price(1.10, 4.40),
        // Anthropic
        "claude-3-5-sonnet" | "claude-3-5-sonnet-20241022" => price(3.00, 15.00),
        "claude-3-5-haiku" | "claude-3-5-haiku-20241022" => price(0.80, 4.00),
        "claude-3-7-sonnet" => price(3.00, 15.00),
        // Local / open models are free at the point of use.
        _ if is_local(&m) => price(0.0, 0.0),
        _ => None,
    }
}

/// Heuristic: does this model name refer to a local (self-hosted) runner?
fn is_local(model: &str) -> bool {
    model.starts_with("ollama")
        || model.starts_with("qwen")
        || model.starts_with("llama")
        || model.starts_with("mistral")
        || model.starts_with("deepseek-r1:")
        || model.contains(":latest")
        || model.contains(":q4")
        || model.contains(":8b")
        || model.contains(":70b")
}

/// Estimate USD cost for `prompt_tokens` + `completion_tokens` at `model`'s
/// price. Unknown models cost $0 (never fabricate). Pure and exact.
pub fn estimate_cost_usd(model: &str, prompt_tokens: usize, completion_tokens: usize) -> f64 {
    match price_for(model) {
        Some(p) => {
            (prompt_tokens as f64 / 1_000_000.0) * p.input
                + (completion_tokens as f64 / 1_000_000.0) * p.output
        }
        None => 0.0,
    }
}

/// Sum per-call costs. Used to fold in the fact that routing may use different
/// models across a session.
pub fn total_cost_usd(calls: &[(&str, usize, usize)]) -> f64 {
    calls
        .iter()
        .map(|(m, p, c)| estimate_cost_usd(m, *p, *c))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_model_math() {
        // deepseek-chat: $0.27/M in, $1.10/M out.
        // 1_000_000 in + 1_000_000 out → 0.27 + 1.10 = 1.37
        let c = estimate_cost_usd("deepseek-chat", 1_000_000, 1_000_000);
        assert!((c - 1.37).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn case_insensitive_lookup() {
        assert!(price_for("DeepSeek-Chat").is_some());
        assert!(price_for("GPT-4O").is_some());
    }

    #[test]
    fn unknown_model_is_zero_not_fabricated() {
        assert!(price_for("totally-made-up-model").is_none());
        assert_eq!(
            estimate_cost_usd("totally-made-up-model", 5_000_000, 5_000_000),
            0.0
        );
    }

    #[test]
    fn local_models_are_free() {
        assert_eq!(
            estimate_cost_usd("qwen2.5-coder:7b", 1_000_000, 1_000_000),
            0.0
        );
        assert_eq!(estimate_cost_usd("llama-3-70b", 999_999, 0), 0.0);
    }

    #[test]
    fn zero_tokens_is_zero() {
        assert_eq!(estimate_cost_usd("gpt-4o", 0, 0), 0.0);
    }

    #[test]
    fn total_sums_per_model() {
        let calls = vec![
            ("gpt-4o", 1_000_000usize, 0usize),        // $2.50
            ("deepseek-chat", 0usize, 1_000_000usize), // $1.10
        ];
        let t = total_cost_usd(&calls);
        assert!((t - 3.60).abs() < 1e-9, "got {t}");
    }
}
