//! generate_tests tool: LLM-generated tests with an automatic run loop.
//!
//! Reads a target source file, asks the LLM to write tests for it using the
//! project's test framework, writes them to the conventional location and —
//! by default — runs them, returning the test output so the agent can fix
//! failures and close the "generate → run → fix" loop.

use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};

use super::{Tool, ToolContext};

pub struct GenerateTests;

/// Detect the primary language of a project directory.
fn detect_language(work_dir: &str) -> Option<&'static str> {
    let p = Path::new(work_dir);
    if p.join("Cargo.toml").exists() {
        Some("rust")
    } else if p.join("package.json").exists() {
        Some("javascript")
    } else if p.join("pyproject.toml").exists()
        || p.join("pytest.ini").exists()
        || p.join("setup.py").exists()
    {
        Some("python")
    } else if p.join("go.mod").exists() {
        Some("go")
    } else {
        None
    }
}

/// Conventional test file path for a target source file and language.
/// Returns None when the layout is unknown.
fn detect_test_file_path(work_dir: &str, lang: &str, target: &Path) -> Option<PathBuf> {
    let stem = target
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled");
    let tests_dir = Path::new(work_dir).join("tests");
    match lang {
        "rust" => Some(tests_dir.join(format!("{}_tests.rs", stem))),
        "javascript" => {
            let ext = target.extension().and_then(|e| e.to_str()).unwrap_or("js");
            let test_ext = match ext {
                "tsx" => "tsx",
                "ts" => "ts",
                "jsx" => "jsx",
                _ => "js",
            };
            Some(target.with_file_name(format!("{}.test.{}", stem, test_ext)))
        }
        "python" => Some(
            Path::new(work_dir)
                .join("tests")
                .join(format!("test_{}.py", stem)),
        ),
        "go" => Some(target.with_file_name(format!("{}_test.go", stem))),
        _ => None,
    }
}

/// Build the LLM prompt that writes tests for a source file.
fn build_generate_tests_prompt(
    content: &str,
    lang: &str,
    framework: Option<&str>,
    target_name: &str,
) -> String {
    let framework_hint = framework
        .map(|f| format!(" Use the '{}' test framework.", f))
        .unwrap_or_default();
    let lang_hint = match lang {
        "rust" => {
            "Rust integration test file (tests/ dir, `#[test]` fns, `use` the crate under test)"
        }
        "javascript" => {
            "JS/TS test file (describe/it or test() blocks, assert real behavior, no mocks unless needed)"
        }
        "python" => "Python pytest test module (plain `def test_*` functions, no unittest classes)",
        "go" => "Go test file (`func TestXxx(t *testing.T)`, table-driven where sensible)",
        _ => "appropriate tests for the language",
    };
    format!(
        "Write unit/integration tests for the source file below.{} \n\
         Requirements:\n\
         1. Test the public behavior and edge cases (empty input, errors, boundaries).\n\
         2. Follow the conventions of a {}. Do NOT modify the source file.\n\
         3. Return ONLY the complete test code in a single fenced code block, no explanation.\n\
         4. The tests must be runnable as-is by the project's test runner.\n\
         \n\
         Target file: {}\n\
         Source content:\n\
         ```\n{}\n```",
        framework_hint, lang_hint, target_name, content
    )
}

/// Extract a fenced code block from LLM output; fall back to the raw text.
fn extract_code_block(text: &str) -> String {
    let trimmed = text.trim();
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        let after = after
            .strip_prefix("rust\n")
            .or_else(|| after.strip_prefix("javascript\n"))
            .or_else(|| after.strip_prefix("typescript\n"))
            .or_else(|| after.strip_prefix("python\n"))
            .or_else(|| after.strip_prefix("go\n"))
            .or_else(|| after.strip_prefix("ts\n"))
            .or_else(|| after.strip_prefix("js\n"))
            .unwrap_or(after);
        if let Some(end) = after.find("```") {
            return after[..end].trim().to_string();
        }
        // Unterminated fence: return everything after the opening fence
        return after.trim().to_string();
    }
    trimmed.to_string()
}

/// Test run command for the generated file (language-specific).
fn test_run_command(lang: &str, test_file: &Path) -> String {
    match lang {
        "rust" => {
            let stem = test_file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            format!("cargo test --test {}", stem)
        }
        "javascript" => {
            let name = test_file
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            // npm test runs the whole suite; prefer the file for a tight loop
            format!("npx jest {} --silent", name)
        }
        "python" => format!("python -m pytest {} -q", test_file.display()),
        "go" => "go test ./...".to_string(),
        _ => String::new(),
    }
}

#[async_trait]
impl Tool for GenerateTests {
    fn name(&self) -> &str {
        "generate_tests"
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> String {
        let target = args["target"].as_str().unwrap_or("").trim();
        if target.is_empty() {
            return "Error: generate_tests requires a 'target' argument (path to a source file, e.g. 'src/lib.rs')"
                .to_string();
        }
        let work_dir = ctx.project_path.as_deref().unwrap_or(".");
        let resolved = crate::agent::utils::resolve_path(Some(work_dir), target);

        let Ok(content) = std::fs::read_to_string(&resolved) else {
            return format!(
                "Error: cannot read target file '{}'. Pass a path to an existing source file.",
                resolved.display()
            );
        };
        let Some(lang) = detect_language(work_dir) else {
            return "Error: cannot detect the project language (no Cargo.toml / package.json / pyproject.toml / go.mod found).".to_string();
        };
        let framework = args["framework"].as_str();

        let snippet: String = {
            const MAX_CHARS: usize = 12_000;
            if content.chars().count() > MAX_CHARS {
                content.chars().take(MAX_CHARS).collect::<String>() + "\n... (truncated)"
            } else {
                content
            }
        };

        let prompt = build_generate_tests_prompt(&snippet, lang, framework, target);
        let request = crate::llm::ChatRequestParams {
            model: ctx.llm_model.clone(),
            messages: vec![crate::llm::ChatMessage {
                role: "user".into(),
                content: prompt,
                images: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            system:
                "You are a senior test engineer. Write high-quality, runnable tests — nothing else."
                    .into(),
            max_tokens: 2500,
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
            Ok(_) => {
                return "Error: LLM returned a non-text response while generating tests"
                    .to_string();
            }
            Err(e) => return format!("Error: LLM call failed: {}", e),
        };
        let code = extract_code_block(&llm_text);
        if code.is_empty() {
            return "Error: LLM returned empty test code".to_string();
        }

        let write = args["write"].as_bool().unwrap_or(true);
        if !write {
            return format!(
                "Generated test code (write: false — not written to disk):\n```\n{}\n```\n\
                 Use write_file to save it, then run_tests to execute.",
                code
            );
        }

        let Some(test_path) = detect_test_file_path(work_dir, lang, &resolved) else {
            return format!(
                "Generated test code (no conventional location for '{}'):\n```\n{}\n```",
                lang, code
            );
        };
        if let Some(parent) = test_path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return format!("Error: cannot create test dir {}: {}", parent.display(), e);
        }
        if let Err(e) = std::fs::write(&test_path, &code) {
            return format!(
                "Error: cannot write test file {}: {}\nGenerated code:\n```\n{}\n```",
                test_path.display(),
                e,
                code
            );
        }

        let run = args["run"].as_bool().unwrap_or(true);
        if !run {
            return format!(
                "Generated tests written to {} (run: false — execute with run_tests).\n\n{}",
                test_path.display(),
                code
            );
        }

        // Run the generated tests (tight loop, no full-suite detour for rust)
        let cmd = test_run_command(lang, &test_path);
        if cmd.is_empty() {
            return format!(
                "Generated tests written to {} (no test runner detected).",
                test_path.display()
            );
        }
        let output = crate::terminal::run_one_shot(&cmd, work_dir, 180).await;
        match output {
            Ok(out) => {
                let mut result = format!(
                    "Generated tests written to {}.\n$ {}\nExit code: {}\n",
                    test_path.display(),
                    cmd,
                    out.exit_code
                );
                const MAX: usize = 40 * 1024;
                let stdout = if out.stdout.len() > MAX {
                    &out.stdout[..MAX]
                } else {
                    &out.stdout
                };
                if !stdout.is_empty() {
                    result.push_str("\n--- Test Output ---\n");
                    result.push_str(stdout);
                }
                if !out.stderr.is_empty() {
                    let stderr = if out.stderr.len() > MAX {
                        &out.stderr[..MAX]
                    } else {
                        &out.stderr
                    };
                    result.push_str("\n--- STDERR ---\n");
                    result.push_str(stderr);
                }
                result
            }
            Err(e) => format!(
                "Generated tests written to {}.\nTest run failed: {}",
                test_path.display(),
                e
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("nee-gentests-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn detects_languages() {
        // Each language gets its own temp dir: detection is priority-ordered
        // when multiple manifests exist, so keep the fixtures unambiguous.
        let rust_dir = temp_dir();
        std::fs::create_dir_all(&rust_dir).unwrap();
        std::fs::write(rust_dir.join("Cargo.toml"), "").unwrap();
        assert_eq!(detect_language(rust_dir.to_str().unwrap()), Some("rust"));

        let js_dir = temp_dir();
        std::fs::create_dir_all(&js_dir).unwrap();
        std::fs::write(js_dir.join("package.json"), "{}").unwrap();
        assert_eq!(
            detect_language(js_dir.to_str().unwrap()),
            Some("javascript")
        );

        let py_dir = temp_dir();
        std::fs::create_dir_all(&py_dir).unwrap();
        std::fs::write(py_dir.join("pyproject.toml"), "").unwrap();
        assert_eq!(detect_language(py_dir.to_str().unwrap()), Some("python"));

        let go_dir = temp_dir();
        std::fs::create_dir_all(&go_dir).unwrap();
        std::fs::write(go_dir.join("go.mod"), "").unwrap();
        assert_eq!(detect_language(go_dir.to_str().unwrap()), Some("go"));

        let empty = temp_dir();
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(detect_language(empty.to_str().unwrap()), None);

        for d in [&rust_dir, &js_dir, &py_dir, &go_dir, &empty] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn test_file_paths_follow_conventions() {
        let dir = temp_dir();
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();

        // Rust → tests/<stem>_tests.rs
        let rust_target = dir.join("src").join("lib.rs");
        let p = detect_test_file_path(dir.to_str().unwrap(), "rust", &rust_target).unwrap();
        assert_eq!(p, dir.join("tests").join("lib_tests.rs"));

        // TS → same dir, .test.ts
        let ts_target = dir.join("src").join("util.ts");
        let p = detect_test_file_path(dir.to_str().unwrap(), "javascript", &ts_target).unwrap();
        assert_eq!(p, dir.join("src").join("util.test.ts"));

        // Python → tests/test_<stem>.py
        let py_target = dir.join("pkg").join("math_utils.py");
        let p = detect_test_file_path(dir.to_str().unwrap(), "python", &py_target).unwrap();
        assert_eq!(p, dir.join("tests").join("test_math_utils.py"));

        // Go → same dir, <stem>_test.go
        let go_target = dir.join("pkg").join("math.go");
        let p = detect_test_file_path(dir.to_str().unwrap(), "go", &go_target).unwrap();
        assert_eq!(p, dir.join("pkg").join("math_test.go"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extracts_fenced_code_blocks() {
        let text = "Here are the tests:\n```rust\n#[test]\nfn works() {}\n```\nDone.";
        assert_eq!(extract_code_block(text), "#[test]\nfn works() {}");
        // No fence → raw trimmed text
        assert_eq!(extract_code_block("  plain text  "), "plain text");
        // Unclosed fence → content after the fence
        let text = "```python\nx = 1";
        assert_eq!(extract_code_block(text), "x = 1");
    }

    #[test]
    fn prompt_mentions_framework_and_target() {
        let p = build_generate_tests_prompt(
            "fn add(a: i32, b: i32) -> i32 { a + b }",
            "rust",
            Some("tokio"),
            "src/lib.rs",
        );
        assert!(p.contains("tokio"), "{}", p);
        assert!(p.contains("src/lib.rs"), "{}", p);
        assert!(p.contains("Do NOT modify the source file"), "{}", p);
    }

    #[test]
    fn run_commands_match_language() {
        assert_eq!(
            test_run_command("rust", Path::new("tests/lib_tests.rs")),
            "cargo test --test lib_tests"
        );
        assert_eq!(
            test_run_command("javascript", Path::new("src/util.test.ts")),
            "npx jest util.test.ts --silent"
        );
        assert!(test_run_command("python", Path::new("tests/test_a.py")).contains("pytest"));
        assert_eq!(
            test_run_command("go", Path::new("pkg/math_test.go")),
            "go test ./..."
        );
    }
}
