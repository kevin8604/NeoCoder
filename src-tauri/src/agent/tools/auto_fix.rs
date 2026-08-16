//! auto_fix tool: LLM-powered diagnosis of a failed command with a fix plan.
//!
//! Given the command that failed and its error output (optionally plus a
//! relevant file), the LLM returns a structured plan: root cause, concrete
//! fix steps and a suggested retry command. The agent executes the plan,
//! closing the "fail → diagnose → fix → retry" loop without burning context
//! on raw error dumps.
//!
//! ```text
//! auto_fix { command: "cargo test --lib", error: "error[E0425]: cannot find value `foo`", file_path: "src/lib.rs" }
//! ```

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolContext};

pub struct AutoFix;

/// Build the LLM prompt that diagnoses a failed command.
fn build_auto_fix_prompt(command: &str, error: &str, file_snippet: Option<&str>) -> String {
    let file_section = match file_snippet {
        Some(snippet) if !snippet.is_empty() => {
            format!(
                "\nRelevant file content (truncated):\n```\n{}\n```\n",
                snippet
            )
        }
        _ => String::new(),
    };
    format!(
        "A developer command failed. Diagnose the root cause and produce a minimal fix plan.\n\n\
         Failed command:
```
{}
```
\
         Error output (truncated):
```
{}
```
{}\
         \n\
         Respond with ONLY a JSON object:\n\
         {{\n  \"root_cause\": \"one-sentence explanation\",\n\
         \x20 \"fix_steps\": [\"concrete step 1\", \"step 2\"],\n\
         \x20 \"retry_command\": \"exact command to re-run after fixing (or empty string)\"\n\
         }}\n\
         Rules: fix_steps must be actionable without further analysis; retry_command must be a\n\
         single shell command; if the fix is a code edit, say so in fix_steps.",
        command, error, file_section
    )
}

/// Extract the first ```json ... ``` block from LLM output (or a bare JSON object).
fn extract_json_block(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        let after = after
            .strip_prefix("json\n")
            .or_else(|| after.strip_prefix("json"))
            .unwrap_or(after);
        if let Some(end) = after.find("```") {
            return Some(after[..end].trim().to_string());
        }
    }
    // Bare JSON fallback: text that starts with '{' and ends with '}'
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end > start {
        Some(trimmed[start..=end].to_string())
    } else {
        None
    }
}

/// Parse the LLM plan into a struct; falls back to a pass-through plan.
fn parse_auto_fix(text: &str) -> AutoFixPlan {
    let block = extract_json_block(text);
    if let Some(block) = block
        && let Ok(v) = serde_json::from_str::<Value>(&block)
    {
        let root_cause = v["root_cause"].as_str().unwrap_or("").trim().to_string();
        let retry_command = v["retry_command"].as_str().unwrap_or("").trim().to_string();
        let fix_steps: Vec<String> = v["fix_steps"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        if !root_cause.is_empty() || !fix_steps.is_empty() {
            return AutoFixPlan {
                root_cause,
                fix_steps,
                retry_command,
                raw: text.trim().to_string(),
            };
        }
    }
    // Unstructured fallback: the whole answer is the analysis
    AutoFixPlan {
        root_cause: "(unstructured analysis — see below)".to_string(),
        fix_steps: vec![text.trim().to_string()],
        retry_command: String::new(),
        raw: text.trim().to_string(),
    }
}

/// A structured diagnosis + fix plan.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoFixPlan {
    pub root_cause: String,
    pub fix_steps: Vec<String>,
    pub retry_command: String,
    pub raw: String,
}

impl AutoFixPlan {
    /// Render the plan as the tool's final text output.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("## Root Cause\n");
        out.push_str(&self.root_cause);
        out.push('\n');
        if !self.fix_steps.is_empty() {
            out.push_str("\n## Fix Steps\n");
            for (i, step) in self.fix_steps.iter().enumerate() {
                out.push_str(&format!("{}. {}\n", i + 1, step));
            }
        }
        if !self.retry_command.is_empty() {
            out.push_str(&format!("\n## Suggested Retry\n$ {}\n", self.retry_command));
        }
        out.push_str("\nApply the fix (edit files or run commands), then re-run and verify.");
        out
    }
}

#[async_trait]
impl Tool for AutoFix {
    fn name(&self) -> &str {
        "auto_fix"
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> String {
        let command = args["command"].as_str().unwrap_or("").trim();
        let error = args["error"]
            .as_str()
            .or_else(|| args["output"].as_str())
            .or_else(|| args["stderr"].as_str())
            .unwrap_or("")
            .trim();
        if command.is_empty() && error.is_empty() {
            return "Error: auto_fix requires a 'command' (the failed command) and an 'error' \
                    argument (the error output). Optionally pass 'file_path' for context."
                .to_string();
        }
        if error.is_empty() {
            return "Error: 'error' (the failed command's output) is required — pass it so the \
                    LLM can diagnose the root cause."
                .to_string();
        }

        // Optional file context: read + truncate the referenced file
        let file_snippet = args["file_path"].as_str().and_then(|fp| {
            let work_dir = ctx.project_path.as_deref().unwrap_or(".");
            let resolved = crate::agent::utils::resolve_path(Some(work_dir), fp);
            std::fs::read_to_string(&resolved).ok().map(|content| {
                const MAX: usize = 6000;
                if content.chars().count() > MAX {
                    content.chars().take(MAX).collect::<String>() + "\n... (truncated)"
                } else {
                    content
                }
            })
        });

        // Truncate the error to a sane size for the prompt
        let error_cut: String = {
            const MAX: usize = 6000;
            if error.chars().count() > MAX {
                error.chars().take(MAX).collect::<String>() + "\n... (truncated)"
            } else {
                error.to_string()
            }
        };

        let prompt = build_auto_fix_prompt(command, &error_cut, file_snippet.as_deref());
        let request = crate::llm::ChatRequestParams {
            model: ctx.llm_model.clone(),
            messages: vec![crate::llm::ChatMessage {
                role: "user".into(),
                content: prompt,
                images: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            system: "You are a debugging expert. Output strictly the JSON plan described in the \
                     user message — no markdown outside the JSON block."
                .into(),
            max_tokens: 900,
            temperature: 0.2,
            thinking_enabled: false,
            thinking_budget: 0,
        };
        let empty_tools: Vec<Value> = vec![];
        let llm_text = match crate::llm::chat_with_tools(
            &ctx.llm_provider,
            &ctx.llm_api_key,
            ctx.llm_base_url.as_deref(),
            request,
            &empty_tools,
            None,
        )
        .await
        {
            Ok((crate::llm::LlmResponse::Text(text), _)) => text,
            Ok(_) => return "Error: LLM returned a non-text response during diagnosis".to_string(),
            Err(e) => {
                return format!(
                    "Error: LLM call failed ({}). The command failed with:\n```\n{}\n```\n\
                     Re-run it after inspecting the output above.",
                    e, error_cut
                );
            }
        };

        let plan = parse_auto_fix(&llm_text);
        log::info!(
            "[AutoFix] Diagnosed '{}': {} ({} steps, retry: {})",
            command,
            plan.root_cause.chars().take(60).collect::<String>(),
            plan.fix_steps.len(),
            if plan.retry_command.is_empty() {
                "none"
            } else {
                "yes"
            }
        );
        plan.render()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_contains_command_error_and_rules() {
        let p = build_auto_fix_prompt("cargo test", "error[E0425]: cannot find value `foo`", None);
        assert!(p.contains("cargo test"), "{}", p);
        assert!(p.contains("E0425"), "{}", p);
        assert!(p.contains("retry_command"), "{}", p);
        assert!(p.contains("fix_steps"), "{}", p);

        let with_file = build_auto_fix_prompt("npm test", "FAIL", Some("const x = 1;"));
        assert!(with_file.contains("const x = 1;"), "{}", with_file);
    }

    #[test]
    fn extracts_json_from_fenced_block() {
        let text = "Here is the plan:\n```json\n{\"root_cause\": \"x\", \"fix_steps\": [], \"retry_command\": \"\"}\n```\nDone";
        assert_eq!(
            extract_json_block(text).unwrap(),
            "{\"root_cause\": \"x\", \"fix_steps\": [], \"retry_command\": \"\"}"
        );
        // Bare object inside prose
        let text = "analysis {\"root_cause\": \"y\"} more";
        assert_eq!(extract_json_block(text).unwrap(), "{\"root_cause\": \"y\"}");
        // No JSON at all
        assert_eq!(extract_json_block("plain text"), None);
    }

    #[test]
    fn parses_structured_plan() {
        let text = r#"```json
{"root_cause": "missing import", "fix_steps": ["add use statement", "re-run"], "retry_command": "cargo build"}
```"#;
        let plan = parse_auto_fix(text);
        assert_eq!(plan.root_cause, "missing import");
        assert_eq!(plan.fix_steps.len(), 2);
        assert_eq!(plan.retry_command, "cargo build");
        let rendered = plan.render();
        assert!(rendered.contains("## Root Cause"), "{}", rendered);
        assert!(rendered.contains("$ cargo build"), "{}", rendered);
    }

    #[test]
    fn falls_back_to_unstructured_text() {
        let plan = parse_auto_fix("the answer is 42");
        assert_eq!(plan.fix_steps, vec!["the answer is 42"]);
        assert_eq!(plan.retry_command, "");
        assert!(plan.render().contains("42"));
    }
}
