use crate::completion::{
    CompletionContext, FnSignature, build_fim_prompt, post_process_completion,
};

fn make_ctx(prefix: &str, suffix: &str, language: &str) -> CompletionContext {
    CompletionContext {
        file_path: format!(
            "test.{}",
            if language == "typescript" {
                "ts"
            } else {
                language
            }
        ),
        language: language.to_string(),
        prefix: prefix.to_string(),
        suffix: suffix.to_string(),
        imports: vec![],
        enclosing_fn: None,
        cursor_line: 10,
        cursor_column: 0,
        recent_lines: vec![],
        related_context: None,
        recent_edits: vec![],
    }
}

// ── build_fim_prompt tests ──

#[test]
fn test_fim_prompt_contains_language() {
    let ctx = make_ctx("", "", "rust");
    let prompt = build_fim_prompt(&ctx, "deepseek");
    assert!(prompt.contains("Language: rust"));
}

#[test]
fn test_fim_prompt_contains_prefix_suffix() {
    let ctx = make_ctx("fn main() {", "}", "rust");
    let prompt = build_fim_prompt(&ctx, "deepseek");
    assert!(prompt.contains("<PRE>\nfn main() {"));
    assert!(prompt.contains("<SUF>\n}"));
    assert!(prompt.contains("<MID>"));
}

#[test]
fn test_fim_prompt_with_imports() {
    let mut ctx = make_ctx("", "", "rust");
    ctx.imports = vec!["use std::io;".to_string(), "use std::fs;".to_string()];
    let prompt = build_fim_prompt(&ctx, "deepseek");
    assert!(prompt.contains("--- Imports ---"));
    assert!(prompt.contains("use std::io;"));
    assert!(prompt.contains("use std::fs;"));
    assert!(prompt.contains("--- End Imports ---"));
}

#[test]
fn test_fim_prompt_with_enclosing_fn() {
    let mut ctx = make_ctx("    let x = 1;", "    return x;", "rust");
    ctx.enclosing_fn = Some(FnSignature {
        name: "compute".to_string(),
        signature: "fn compute(a: i32) -> i32".to_string(),
        start_line: 1,
        end_line: 10,
    });
    let prompt = build_fim_prompt(&ctx, "deepseek");
    assert!(prompt.contains("--- Context: fn compute(a: i32) -> i32 ---"));
}

#[test]
fn test_fim_prompt_empty_prefix_suffix() {
    let ctx = make_ctx("", "", "python");
    let prompt = build_fim_prompt(&ctx, "deepseek");
    assert!(prompt.contains("<PRE>\n\n<SUF>\n\n<MID>"));
}

#[test]
fn test_fim_prompt_typescript() {
    let ctx = make_ctx("const x = ", ";", "typescript");
    let prompt = build_fim_prompt(&ctx, "openai");
    assert!(prompt.contains("Language: typescript"));
    assert!(prompt.contains("const x = "));
}

#[test]
fn test_fim_prompt_structure_order() {
    let mut ctx = make_ctx("prefix_code", "suffix_code", "python");
    ctx.imports = vec!["import os".to_string()];
    ctx.enclosing_fn = Some(FnSignature {
        name: "main".to_string(),
        signature: "def main():".to_string(),
        start_line: 1,
        end_line: 20,
    });
    let prompt = build_fim_prompt(&ctx, "deepseek");

    let lang_pos = prompt.find("Language:").unwrap();
    let imports_pos = prompt.find("--- Imports ---").unwrap();
    let ctx_pos = prompt.find("--- Context:").unwrap();
    let code_pos = prompt.find("--- Code ---").unwrap();
    let pre_pos = prompt.find("<PRE>").unwrap();
    let suf_pos = prompt.find("<SUF>").unwrap();
    let mid_pos = prompt.find("<MID>").unwrap();

    assert!(lang_pos < imports_pos);
    assert!(imports_pos < ctx_pos);
    assert!(ctx_pos < code_pos);
    assert!(code_pos < pre_pos);
    assert!(pre_pos < suf_pos);
    assert!(suf_pos < mid_pos);
}

#[test]
fn test_fim_prompt_with_recent_edits() {
    let mut ctx = make_ctx("", "", "rust");
    ctx.recent_edits = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
    let prompt = build_fim_prompt(&ctx, "deepseek");
    assert!(prompt.contains("--- Recent Changes ---"));
    assert!(prompt.contains("// modified: src/main.rs"));
    assert!(prompt.contains("// modified: src/lib.rs"));
    assert!(prompt.contains("--- End Recent Changes ---"));
}

#[test]
fn test_fim_prompt_no_recent_edits_section() {
    let ctx = make_ctx("", "", "rust");
    let prompt = build_fim_prompt(&ctx, "deepseek");
    assert!(!prompt.contains("--- Recent Changes ---"));
}

// ── post_process_completion tests ──

#[test]
fn test_post_process_trim_leading_whitespace() {
    let ctx = make_ctx("", "", "rust");
    let result = post_process_completion("\n\n  hello world", &ctx);
    assert!(
        result.starts_with("hello")
            || result.starts_with("  hello")
            || result.trim().starts_with("hello")
    );
}

#[test]
fn test_post_process_remove_trailing_whitespace() {
    let ctx = make_ctx("", "", "rust");
    let result = post_process_completion("hello   \nworld   ", &ctx);
    assert!(result.contains("hello\n") || result.contains("hello\nworld"));
    assert!(!result.ends_with(' '));
}

#[test]
fn test_post_process_indent_preservation() {
    let ctx = make_ctx("    let x = 1;", "", "rust");
    let result = post_process_completion("let y = 2;\nlet z = x + y;", &ctx);
    // Should add 4-space indent to both lines
    assert!(result.contains("    let y = 2;"));
}

#[test]
fn test_post_process_empty_completion() {
    let ctx = make_ctx("fn main() {", "}", "rust");
    let result = post_process_completion("", &ctx);
    assert!(result.is_empty() || result.trim().is_empty());
}

#[test]
fn test_post_process_no_indent_when_no_prefix_indent() {
    let ctx = make_ctx("x = 1", "", "python");
    let result = post_process_completion("y = 2", &ctx);
    // No indent in prefix's last line, so no indent should be added
    assert_eq!(result, "y = 2");
}

#[test]
fn test_post_process_multiline_indent() {
    let ctx = make_ctx("    print('a')", "", "python");
    let result = post_process_completion("print('b')\nprint('c')", &ctx);
    // Both lines should be indented
    assert!(result.contains("    print('b')"));
    assert!(result.contains("    print('c')"));
}

// ── COMPLETION_SYSTEM_PROMPT ──

#[test]
fn test_completion_system_prompt_not_empty() {
    use crate::completion::COMPLETION_SYSTEM_PROMPT;
    assert!(!COMPLETION_SYSTEM_PROMPT.is_empty());
    assert!(COMPLETION_SYSTEM_PROMPT.contains("code completion"));
    assert!(COMPLETION_SYSTEM_PROMPT.contains("<MID>"));
}
