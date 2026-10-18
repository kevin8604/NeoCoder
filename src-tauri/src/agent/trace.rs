//! Pure prompt-assembly observability.
//!
//! The eval harness can't see the *content* of the system prompt (the LLM is
//! mocked), which makes "what did we actually inject this turn?" invisible. The
//! system prompt is assembled with `## <Section>` markdown headers (base prompt,
//! Cross-session Memory, Project Failure Lessons, preferences, RAG, …), so we can
//! recover a per-section size breakdown by splitting on those headers — no
//! restructuring of the (large) builder required.
//!
//! Everything here is pure string processing and headless-testable.

use crate::chat::PromptSection;

/// Split an assembled system prompt into its `## `-delimited sections with their
/// character counts. Text before the first `## ` header is reported as "base".
/// Only level-2 `## ` headers start a section (a `### ` sub-header does not).
pub fn prompt_sections(prompt: &str) -> Vec<PromptSection> {
    let mut sections: Vec<PromptSection> = Vec::new();
    let mut title = "base".to_string();
    let mut chars = 0usize;

    for line in prompt.lines() {
        let is_h2 = line.starts_with("## ") && !line.starts_with("### ");
        if is_h2 {
            // Flush the accumulated run under its current title.
            if chars > 0 || title != "base" {
                sections.push(PromptSection {
                    title: title.clone(),
                    chars,
                });
            }
            title = line[3..].trim().to_string();
            chars = 0;
        } else {
            // Count the line plus its newline (approximates the real byte cost
            // of what we send, without needing exact offsets).
            chars += line.chars().count() + 1;
        }
    }
    if chars > 0 || title != "base" {
        sections.push(PromptSection { title, chars });
    }
    sections
}

/// Total characters across all sections (== prompt length, up to newline
/// accounting). Handy for a quick "how big is the prompt" readout.
pub fn prompt_char_count(prompt: &str) -> usize {
    prompt.chars().count()
}

/// Fraction of prompt tokens served from the provider cache, clamped to 0..=1.
/// Returns 0.0 when there were no prompt tokens (avoids divide-by-zero).
pub fn cache_hit_rate(cached_tokens: usize, prompt_tokens: usize) -> f64 {
    if prompt_tokens == 0 {
        0.0
    } else {
        (cached_tokens as f64 / prompt_tokens as f64).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_h2_headers() {
        let p = "You are helpful.\n\n## Cross-session Memory\nmem text\n\n## Project Failure Lessons\nE0308 at a.rs:1\n";
        let secs = prompt_sections(p);
        assert_eq!(secs.len(), 3);
        assert_eq!(secs[0].title, "base");
        assert!(secs[0].chars > 0);
        assert_eq!(secs[1].title, "Cross-session Memory");
        assert!(secs[1].chars >= "mem text".len());
        assert_eq!(secs[2].title, "Project Failure Lessons");
    }

    #[test]
    fn h3_is_not_a_boundary() {
        let p = "base\n## Section\n### Sub\nstill here\n";
        let secs = prompt_sections(p);
        assert_eq!(secs.len(), 2, "base + Section (### stays inside Section)");
        assert_eq!(secs[1].title, "Section");
        assert!(secs[1].chars >= "### Sub\nstill here".len());
    }

    #[test]
    fn no_headers_is_single_base() {
        let secs = prompt_sections("just a plain prompt, no sections");
        assert_eq!(secs.len(), 1);
        assert_eq!(secs[0].title, "base");
        assert!(secs[0].chars > 0);
    }

    #[test]
    fn empty_prompt_yields_base_zero() {
        let secs = prompt_sections("");
        // Empty input: the final flush is skipped (chars==0 and title=="base").
        assert!(secs.is_empty() || (secs.len() == 1 && secs[0].chars == 0));
    }

    #[test]
    fn char_count_counts_codepoints() {
        assert_eq!(prompt_char_count("héllo"), 5);
        assert_eq!(prompt_char_count("你好"), 2, "CJK counted per character");
    }

    #[test]
    fn cache_hit_rate_basics() {
        assert_eq!(cache_hit_rate(0, 0), 0.0, "no prompt tokens → 0");
        assert!((cache_hit_rate(500, 1000) - 0.5).abs() < 1e-9);
        assert!(
            (cache_hit_rate(2000, 1000) - 1.0).abs() < 1e-9,
            "clamped to 1.0"
        );
    }
}
