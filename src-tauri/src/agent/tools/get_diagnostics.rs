use super::{Tool, ToolContext};

pub struct GetDiagnostics;

/// Run a diagnostic command and return filtered output for a specific file.
async fn run_diagnostic_cmd(
    cmd: &str,
    args: &[&str],
    work_dir: &str,
    file_hint: Option<&str>,
) -> String {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new(cmd)
            .args(args)
            .current_dir(work_dir)
            .output(),
    )
    .await;

    match output {
        Ok(Ok(out)) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            let combined = format!("{}{}", stdout, stderr);
            if combined.trim().is_empty() {
                "✅ No diagnostics found. Code is clean.\n".to_string()
            } else if let Some(hint) = file_hint {
                // Filter lines relevant to the target file
                let mut relevant: Vec<&str> = Vec::new();
                for line in combined.lines() {
                    if line.contains(hint) || line.contains("error") || line.contains("Error") {
                        relevant.push(line);
                    }
                }
                if relevant.is_empty() {
                    format!("(No diagnostics specific to {})\n", hint)
                } else {
                    relevant.join("\n") + "\n"
                }
            } else {
                if combined.len() > 4000 {
                    format!("{}\n... (truncated at 4KB)", crate::agent::utils::safe_truncate(&combined, 4000))
                } else {
                    combined
                }
            }
        }
        Ok(Err(e)) => format!("Could not run `{}`: {}\n", cmd, e),
        Err(_) => format!("`{}` timed out after 30 seconds\n", cmd),
    }
}

#[async_trait::async_trait]
impl Tool for GetDiagnostics {
    fn name(&self) -> &str {
        "get_diagnostics"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let file_path = args["file_path"].as_str();

        // Resolve file path
        let resolved = match file_path {
            Some(p) => crate::agent::utils::resolve_path(ctx.project_path.as_deref(), p),
            None => {
                return "Error: file_path is required. Specify the file to get diagnostics for.".to_string();
            }
        };

        let path_str = resolved.to_string_lossy().to_string();
        let file_name = resolved.file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();

        // Determine language
        let language = crate::lsp::detect_language(&path_str);
        let lsp_lang = if language == "typescript" || language == "javascript" {
            "typescript"
        } else {
            &language
        };

        let work_dir = ctx.project_path.as_deref().unwrap_or(".");
        let mut output = format!("Diagnostics for: {}\n", path_str);

        match lsp_lang {
            "rust" => {
                let result = run_diagnostic_cmd(
                    "cargo", &["check", "--message-format=short"], work_dir, Some(&path_str),
                ).await;
                output.push_str(&result);
            }
            "typescript" | "javascript" => {
                // Try tsc first
                let result = run_diagnostic_cmd(
                    "npx", &["tsc", "--noEmit", "--pretty", "false"], work_dir, Some(&file_name),
                ).await;
                output.push_str(&result);
            }
            "python" => {
                // Try py_compile
                let result = run_diagnostic_cmd(
                    "python", &["-m", "py_compile", &path_str], work_dir, None,
                ).await;
                output.push_str(&result);
            }
            "go" => {
                let result = run_diagnostic_cmd(
                    "go", &["vet", "./..."], work_dir, Some(&file_name),
                ).await;
                output.push_str(&result);
            }
            "c" | "cpp" => {
                // Try gcc/g++ syntax check
                let compiler = if lsp_lang == "c" { "gcc" } else { "g++" };
                let result = run_diagnostic_cmd(
                    compiler, &["-fsyntax-only", "-Wall", &path_str], work_dir, Some(&file_name),
                ).await;
                output.push_str(&result);
            }
            "java" => {
                let result = run_diagnostic_cmd(
                    "javac", &["-Xlint:all", &path_str], work_dir, Some(&file_name),
                ).await;
                output.push_str(&result);
            }
            _ => {
                output.push_str("Language-specific diagnostics not available. ");
                output.push_str("Supported: Rust (cargo check), TypeScript (tsc), Python (py_compile), Go (go vet), C/C++ (gcc/g++), Java (javac).\n");
                output.push_str("Use `run_terminal_command` with your project's linter for other languages.\n");
            }
        }

        output
    }
}
