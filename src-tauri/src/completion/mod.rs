use serde::{Deserialize, Serialize};

pub mod multi_file;
pub use multi_file::RelatedContext;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionContext {
    pub file_path: String,
    pub language: String,
    pub prefix: String,
    pub suffix: String,
    pub imports: Vec<String>,
    pub enclosing_fn: Option<FnSignature>,
    pub cursor_line: u32,
    pub cursor_column: u32,
    pub recent_lines: Vec<String>,
    /// Related file context for multi-file awareness (optional, populated by backend)
    #[serde(default)]
    pub related_context: Option<RelatedContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FnSignature {
    pub name: String,
    pub signature: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Completion {
    pub id: String,
    pub text: String,
    pub rank: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompletionEvent {
    Started { id: String },
    Delta { id: String, token: String },
    Finished { id: String, full_text: String },
    Error { id: String, message: String },
    Cancelled { id: String },
}

/// Detect AST context from code prefix (lightweight, pattern-based).
/// Returns a description of the current code structure (e.g., "inside function body", "inside match arm").
pub fn detect_ast_context(prefix: &str, language: &str) -> Option<String> {
    let lines: Vec<&str> = prefix.lines().collect();
    if lines.is_empty() {
        return None;
    }

    // Walk backwards to find enclosing structure
    let mut brace_depth = 0;
    let mut paren_depth = 0;
    let mut context_parts: Vec<String> = Vec::new();

    for line in lines.iter().rev() {
        let trimmed = line.trim();
        
        // Count braces (rough heuristic)
        for ch in trimmed.chars().rev() {
            match ch {
                '}' => brace_depth += 1,
                '{' => brace_depth -= 1,
                ')' => paren_depth += 1,
                '(' => paren_depth -= 1,
                _ => {}
            }
        }

        // Detect function/impl/module declarations
        if trimmed.starts_with("fn ") || trimmed.starts_with("pub fn ") || trimmed.starts_with("async fn ") {
            context_parts.push(format!("function: {}", trimmed.trim_end_matches('{').trim()));
            break;
        }
        if trimmed.starts_with("impl ") {
            context_parts.push(format!("impl block: {}", trimmed.trim_end_matches('{').trim()));
            break;
        }
        if trimmed.starts_with("mod ") || trimmed.starts_with("pub mod ") {
            context_parts.push(format!("module: {}", trimmed.trim_end_matches('{').trim()));
            break;
        }
        if trimmed.starts_with("match ") {
            context_parts.push(format!("match expression: {}", trimmed.trim_end_matches('{').trim()));
            break;
        }
        if trimmed.starts_with("struct ") || trimmed.starts_with("pub struct ") {
            context_parts.push(format!("struct: {}", trimmed.trim_end_matches('{').trim()));
            break;
        }
        if trimmed.starts_with("enum ") || trimmed.starts_with("pub enum ") {
            context_parts.push(format!("enum: {}", trimmed.trim_end_matches('{').trim()));
            break;
        }

        // Language-specific patterns
        if language == "python" || language == "py" {
            if trimmed.starts_with("def ") {
                context_parts.push(format!("function: {}", trimmed));
                break;
            }
            if trimmed.starts_with("class ") {
                context_parts.push(format!("class: {}", trimmed));
                break;
            }
        }
        if language == "typescript" || language == "javascript" || language == "ts" || language == "js" {
            if trimmed.starts_with("function ") || trimmed.contains("=> {") || trimmed.contains("function(") {
                context_parts.push(format!("function: {}", trimmed.trim_end_matches('{').trim()));
                break;
            }
            if trimmed.starts_with("class ") {
                context_parts.push(format!("class: {}", trimmed.trim_end_matches('{').trim()));
                break;
            }
        }
    }

    if context_parts.is_empty() {
        None
    } else {
        Some(context_parts.join(", "))
    }
}

/// Build a Fill-in-the-Middle prompt for code completion
pub fn build_fim_prompt(ctx: &CompletionContext, _provider: &str) -> String {
    let lang = &ctx.language;
    let _file_ext = ctx
        .file_path
        .rsplit('.')
        .next()
        .unwrap_or("rs");

    let mut prompt = String::new();

    // Language hint
    prompt.push_str(&format!("Language: {}\n", lang));

    // Imports section
    if !ctx.imports.is_empty() {
        prompt.push_str("--- Imports ---\n");
        for imp in &ctx.imports {
            prompt.push_str(imp);
            prompt.push('\n');
        }
        prompt.push_str("--- End Imports ---\n");
    }

    // Related file context (multi-file awareness)
    if let Some(related) = &ctx.related_context {
        if !related.files.is_empty() {
            prompt.push_str("--- Related Code ---\n");
            for file in &related.files {
                prompt.push_str(&format!("// File: {}\n", file.path));
                for sym in &file.symbols {
                    prompt.push_str(&format!("{}\n", sym.signature));
                }
            }
            prompt.push_str("--- End Related ---\n");
        }
    }

    // Enclosing function context
    if let Some(fn_sig) = &ctx.enclosing_fn {
        prompt.push_str(&format!("--- Context: {} ---\n", fn_sig.signature));
    }

    // AST context (lightweight structural awareness)
    if let Some(ast_ctx) = detect_ast_context(&ctx.prefix, &ctx.language) {
        prompt.push_str(&format!("--- AST Context: {} ---\n", ast_ctx));
    }

    // FIM format with prefix and suffix
    prompt.push_str("--- Code ---\n");
    prompt.push_str("<PRE>\n");
    prompt.push_str(&ctx.prefix);
    prompt.push_str("\n<SUF>\n");
    prompt.push_str(&ctx.suffix);
    prompt.push_str("\n<MID>\n");

    prompt
}

/// System prompt for code completion
pub const COMPLETION_SYSTEM_PROMPT: &str = "You are a code completion AI. \
Your task is to complete the code at the <MID> position based on the context before (<PRE>) and after (<SUF>) the cursor. \
Rules:
1. Output ONLY the code that should fill the middle position
2. Do NOT include any explanation, comments about what you're doing, or markdown formatting
3. Match the existing code style (indentation, naming conventions)
4. Produce idiomatic code for the detected language
5. Keep completions concise but complete
6. If you can't determine the right completion, output nothing";

/// Post-process a completion result
pub fn post_process_completion(completion: &str, _ctx: &CompletionContext) -> String {
    let mut result = completion.to_string();

    // Trim leading whitespace/newlines
    result = result.trim_start().to_string();

    // Fix indentation: align with the cursor line's indentation
    if let Some(last_line) = _ctx.prefix.lines().last() {
        let indent: String = last_line
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        if !indent.is_empty() {
            // Add indent to the first line if it doesn't already have it
            if !result.starts_with(&indent) {
                result = format!("{}{}", indent, result);
            }
            // Ensure all lines have proper indentation (simple approach)
            let mut indented = String::new();
            for (i, line) in result.lines().enumerate() {
                if i > 0 && !line.is_empty() && !line.starts_with('\n') {
                    let line_indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                    if line_indent.is_empty() && !line.trim().is_empty() {
                        indented.push_str(&indent);
                    }
                }
                indented.push_str(line);
                indented.push('\n');
            }
            result = indented;
        }
    }

    // Remove trailing whitespace
    result = result
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n");

    result
}
