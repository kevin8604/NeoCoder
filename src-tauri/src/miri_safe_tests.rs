//! Pure-logic tests that are safe to run under Miri (no FFI, no tokio, no
//! threads, no file system access). Miri executes these as an interpreter
//! with an UB sanitizer (out-of-bounds, use-after-free, misaligned accesses,
//! invalid enum discriminant reads, ...).
//!
//! Run with:  `cargo +nightly miri test --lib miri_safe`
//!
//! Keep this module free of: `tokio::*`, `std::fs`, `std::net`, `std::thread`,
//! and any C FFI (includes tauri runtime types). Pure computation only.

// ── Token estimation (agent/token_count) ──────────────────────────────────

#[test]
fn miri_token_count_empty_text() {
    let n = crate::agent::token_count::count_tokens("", "");
    assert_eq!(n, 0, "empty input must produce zero tokens");
}

#[test]
fn miri_token_count_bounded() {
    let text = "hello world foo bar baz";
    let n = crate::agent::token_count::count_tokens(text, "");
    // Both the tiktoken path and the chars/3 fallback stay within these bounds.
    assert!(n > 0, "non-empty input must produce tokens");
    assert!(
        n <= text.chars().count(),
        "token count cannot exceed char count"
    );
}

// ── Ebbinghaus memory math (memory/ebbinghaus) ────────────────────────────

#[test]
fn miri_stability_growth_monotonic() {
    use crate::memory::ebbinghaus::MemoryCategory;
    let cat = MemoryCategory::Coding;
    let s1 = cat.stability_growth(1);
    let s2 = cat.stability_growth(2);
    let s3 = cat.stability_growth(5);
    assert!(
        s1 < s2 && s2 < s3,
        "stability growth must be monotonic: {s1} {s2} {s3}"
    );
}

#[test]
fn miri_core_growth_is_zero() {
    use crate::memory::ebbinghaus::MemoryCategory;
    assert_eq!(MemoryCategory::Core.stability_growth(10), 0.0);
}

#[test]
fn miri_similarity_bounds_and_symmetry() {
    use crate::memory::ebbinghaus::compute_similarity;
    let a = "rust cargo clippy lint";
    let b = "rust cargo build test";
    let s = compute_similarity(a, b);
    assert!((0.0..=1.0).contains(&s), "similarity out of bounds: {s}");
    assert_eq!(s, compute_similarity(b, a), "similarity must be symmetric");
    assert_eq!(compute_similarity(a, a), 1.0, "identical texts score 1.0");
    assert_eq!(compute_similarity("", ""), 1.0);
}

#[test]
fn miri_coding_relevance_bounds() {
    use crate::memory::ebbinghaus::compute_coding_relevance;
    assert_eq!(compute_coding_relevance(""), 0.0);
    let r = compute_coding_relevance("fix the rust borrow checker error in async fn");
    assert!((0.0..=1.0).contains(&r), "relevance out of bounds: {r}");
    assert!(r > 0.0, "coding keywords should match");
}

// ── AST context detection (completion) ────────────────────────────────────

#[test]
fn miri_ast_context_function() {
    let ctx =
        crate::completion::detect_ast_context("fn main() {\n    let x = 1;\n    x + 1", "rust");
    let ctx = ctx.expect("function body should be detected");
    assert!(ctx.contains("function"), "unexpected context: {ctx}");
}

#[test]
fn miri_ast_context_empty_prefix() {
    assert_eq!(crate::completion::detect_ast_context("", "rust"), None);
}

// ── Model context window mapping (config) ─────────────────────────────────

#[test]
fn miri_model_context_window_mapping() {
    use crate::config::model_context_window;
    assert_eq!(model_context_window("gpt-4"), 8_192);
    assert_eq!(model_context_window("gpt-4o-mini"), 128_000);
    assert_eq!(model_context_window("deepseek-chat"), 64_000);
    assert_eq!(model_context_window("deepseek-v3"), 128_000);
    assert_eq!(model_context_window("claude-3-5-sonnet-20241022"), 200_000);
    assert_eq!(model_context_window("qwen2.5-coder"), 128_000);
    assert_eq!(model_context_window("llama-3-70b"), 128_000);
    assert_eq!(model_context_window("some-unknown-model"), 32_000);
}

// ── RAG context builder (rag) ─────────────────────────────────────────────

#[test]
fn miri_rag_build_context_truncates() {
    use crate::rag::{ChunkType, CodeChunk, build_rag_context};
    let chunks: Vec<CodeChunk> = (0..10)
        .map(|i| CodeChunk {
            id: format!("id{i}"),
            file_path: format!("f{i}.rs"),
            start_line: i + 1,
            end_line: i + 1,
            language: "rust".into(),
            chunk_type: ChunkType::Function,
            content: format!("fn f{i}() {{}}"),
            summary: String::new(),
        })
        .collect();
    let ctx = build_rag_context(&chunks, 3);
    assert!(ctx.contains("f0.rs"), "first chunk must be included");
    assert!(ctx.contains("f2.rs"), "third chunk must be included");
    assert!(
        !ctx.contains("f3.rs"),
        "chunks beyond max_chunks must be truncated"
    );
}

// ── SSE / stream parsing helpers stay covered elsewhere; these pure pieces ─
// ── exercise the arithmetic that UB bugs most often hide in.             ──
