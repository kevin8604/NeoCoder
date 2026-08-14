use tauri::{Emitter, State};
use crate::config::AppSettings;
use crate::llm::{self, ChatMessage, ChatRequestParams};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

// ── Request / Response types ──────────────────────────────────────────────

/// One prior turn of an inline-chat conversation: the instruction that was
/// issued and the code the model produced for it (kept so later turns can
/// refine that output instead of re-editing the original selection).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditTurn {
    pub instruction: String,
    pub edited: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditInlineRequest {
    pub instruction: String,
    pub file_path: String,
    pub selected_code: String,
    pub prefix_context: String,
    pub suffix_context: String,
    /// Previous turns of this conversation (multi-turn inline chat).
    #[serde(default)]
    pub history: Vec<EditTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditInlineResponse {
    pub original: String,
    pub edited: String,
    pub diff_lines: Vec<String>,
}

/// Compute line-level diff between original and edited text
fn compute_diff_lines(original: &str, edited: &str) -> Vec<String> {
    let orig_lines: Vec<&str> = original.lines().collect();
    let edit_lines: Vec<&str> = edited.lines().collect();
    let mut diff = Vec::new();

    let max_len = orig_lines.len().max(edit_lines.len());
    for i in 0..max_len {
        let o = orig_lines.get(i).copied();
        let e = edit_lines.get(i).copied();
        match (o, e) {
            (Some(ol), Some(el)) if ol != el => {
                diff.push(format!("- {}", ol));
                diff.push(format!("+ {}", el));
            }
            (Some(ol), None) => diff.push(format!("- {}", ol)),
            (None, Some(el)) => diff.push(format!("+ {}", el)),
            _ => {} // identical lines omitted
        }
    }
    diff
}

/// Streaming event for inline edit progress
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EditInlineEvent {
    Started,
    Delta { token: String },
    Finished { edited: String },
    Error { message: String },
}

// ── System prompt for inline editing ──────────────────────────────────────

const INLINE_EDIT_SYSTEM_PROMPT: &str = "You are a precise code editor AI. Your task is to modify code according to the user's instruction.

Rules:
1. Output ONLY the modified code - no explanations, no markdown formatting, no code blocks
2. Preserve the original indentation and code style
3. Make minimal, targeted changes based on the instruction
4. If the instruction asks to add something, add it at the appropriate location
5. If the instruction asks to fix/modify something, make the minimal necessary changes
6. Maintain all existing functionality unless explicitly told to remove it
7. Do NOT add explanatory comments unless the instruction explicitly asks for them";

// ── Command ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn edit_inline(
    app: tauri::AppHandle,
    request: EditInlineRequest,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
) -> Result<EditInlineResponse, String> {
    let settings = settings.read().await;

    // Emit started event
    let _ = app.emit("edit-inline-event", EditInlineEvent::Started);

    // Build the user message with full context
    let mut user_prompt = format!(
        "File: {}\n\nInstruction: {}\n\n--- Code to edit ---\n{}\n--- End code ---",
        request.file_path,
        request.instruction,
        request.selected_code
    );

    // Multi-turn support: tell the model what was already produced in this
    // conversation so it refines the latest output rather than redoing work.
    if !request.history.is_empty() {
        let mut history_block =
            String::from("\n\nPrevious turns of this conversation (their results are already applied to the code above — do not redo them, refine the latest result):\n");
        for (i, turn) in request.history.iter().enumerate() {
            history_block.push_str(&format!(
                "Turn {} instruction: {}\nTurn {} result:\n{}\n",
                i + 1,
                turn.instruction,
                i + 1,
                turn.edited
            ));
        }
        user_prompt.push_str(&history_block);
    }

    // Build context-enriched system prompt
    let mut system_prompt = INLINE_EDIT_SYSTEM_PROMPT.to_string();

    // Add surrounding context if available
    if !request.prefix_context.is_empty() || !request.suffix_context.is_empty() {
        system_prompt.push_str("\n\nSurrounding context for reference:\n");
        if !request.prefix_context.is_empty() {
            system_prompt.push_str(&format!("--- Before selection ---\n{}\n", request.prefix_context));
        }
        if !request.suffix_context.is_empty() {
            system_prompt.push_str(&format!("--- After selection ---\n{}\n", request.suffix_context));
        }
    }

    let messages = vec![
        ChatMessage::text("system", &system_prompt),
        ChatMessage::text("user", &user_prompt),
    ];

    let chat_request = ChatRequestParams {
        model: settings.chat_model.clone(),
        messages,
        system: system_prompt.clone(),
        max_tokens: 4096,
        temperature: 0.1, // Low temperature for precise edits
        thinking_enabled: false, // Disable thinking for inline edit (speed)
        thinking_budget: 0,
    };

    let provider = settings.llm_provider.clone();
    let api_key = settings.api_key.clone();

    // Accumulate streamed tokens
    let full_text = Arc::new(std::sync::Mutex::new(String::new()));
    let full_text_clone = full_text.clone();
    let app_clone = app.clone();

    let result = llm::stream_chat(
        &provider,
        &api_key,
        None,
        chat_request,
        |token| {
            if let Ok(mut buf) = full_text_clone.lock() {
                buf.push_str(&token);
            }
            let _ = app_clone.emit("edit-inline-event", EditInlineEvent::Delta { token });
            Ok(())
        },
        None,
    )
    .await;

    match result {
        Ok(()) => {
            let edited = full_text.lock().map(|s| s.clone()).unwrap_or_default();

            // Post-process: strip markdown code fences if present
            let edited = strip_code_fences(&edited);
            // Trim a trailing newline the model often appends after a code block
            let edited = edited.trim_end().to_string();

            let _ = app.emit(
                "edit-inline-event",
                EditInlineEvent::Finished {
                    edited: edited.clone(),
                },
            );

            let diff_lines = compute_diff_lines(&request.selected_code, &edited);
            Ok(EditInlineResponse {
                original: request.selected_code,
                edited,
                diff_lines,
            })
        }
        Err(e) => {
            let _ = app.emit(
                "edit-inline-event",
                EditInlineEvent::Error {
                    message: e.clone(),
                },
            );
            Err(e)
        }
    }
}

/// Strip markdown code fences from LLM output
fn strip_code_fences(text: &str) -> String {
    let trimmed = text.trim();

    // Check for ```lang\n...\n``` pattern
    if trimmed.starts_with("```") {
        let lines: Vec<&str> = trimmed.lines().collect();
        if lines.len() >= 2 {
            // Find the closing fence
            let start = 1; // Skip the opening ```lang line
            let end = if lines.last().map(|l| l.trim()) == Some("```") {
                lines.len() - 1
            } else {
                lines.len()
            };
            return lines[start..end].join("\n");
        }
    }

    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_code_fences_plain() {
        let input = "fn hello() { println!(\"hi\"); }";
        assert_eq!(strip_code_fences(input), input);
    }

    #[test]
    fn test_strip_code_fences_with_lang() {
        let input = "```rust\nfn hello() {\n    println!(\"hi\");\n}\n```";
        let expected = "fn hello() {\n    println!(\"hi\");\n}";
        assert_eq!(strip_code_fences(input), expected);
    }

    #[test]
    fn test_strip_code_fences_no_closing() {
        let input = "```\nfn hello() {\n    println!(\"hi\");\n}";
        let expected = "fn hello() {\n    println!(\"hi\");\n}";
        assert_eq!(strip_code_fences(input), expected);
    }
}
