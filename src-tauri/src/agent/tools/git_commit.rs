use super::{Tool, ToolContext};

/// Git commit tool: stages changes and creates a commit.
///
/// Pass `auto_summary: true` (and omit `message`) to have the LLM derive a
/// concise commit message from the staged diff, mirroring the repository's
/// recent commit style. Falls back to a name-status summary when the LLM is
/// unavailable.
pub struct GitCommit;

/// Run a git command and return trimmed stdout (empty on failure).
async fn git_capture(work_dir: &str, args: &[&str]) -> Option<String> {
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(work_dir)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Build the LLM prompt that derives a commit message from the staged diff.
fn build_auto_summary_prompt(stat: &str, name_status: &str, recent_logs: &str) -> String {
    format!(
        "Write ONE concise git commit message (single line, imperative mood, no body) \
         summarizing the staged changes below.\n\n\
         Recent commits in this repository (match their style — same tone, no emojis, \
         no Conventional-commit prefixes unless the history uses them):\n\
         ```\n{}\n```\n\
         Staged diff stat:\n\
         ```\n{}\n```\n\
         Changed files (status\tpath):\n\
         ```\n{}\n```\n\
         Commit message:",
        recent_logs, stat, name_status
    )
}

/// Fallback summary built purely from `git diff --cached --name-status` output.
fn fallback_commit_summary(name_status: &str) -> String {
    let files: Vec<&str> = name_status
        .lines()
        .filter_map(|l| l.split('\t').nth(1))
        .filter(|p| !p.is_empty())
        .collect();
    if files.is_empty() {
        return "Update staged changes".to_string();
    }
    if files.len() == 1 {
        return format!("Update {}", files[0]);
    }
    let mut short = files[0].to_string();
    for f in files.iter().take(3).skip(1) {
        short.push_str(", ");
        short.push_str(f);
    }
    if files.len() > 3 {
        short.push_str(&format!(" +{} more", files.len() - 3));
    }
    format!("Update {} files: {}", files.len(), short)
}

/// Derive a commit message from the staged diff (LLM first, fallback on failure).
async fn build_auto_message(work_dir: &str, ctx: &ToolContext) -> Option<String> {
    let stat = git_capture(work_dir, &["diff", "--cached", "--stat"]).await?;
    if stat.is_empty() {
        return None;
    }
    let name_status = git_capture(work_dir, &["diff", "--cached", "--name-status"])
        .await
        .unwrap_or_default();
    let recent_logs = git_capture(work_dir, &["log", "-5", "--oneline"])
        .await
        .unwrap_or_default();

    let prompt = build_auto_summary_prompt(&stat, &name_status, &recent_logs);
    let request = crate::llm::ChatRequestParams {
        model: ctx.llm_model.clone(),
        messages: vec![crate::llm::ChatMessage {
            role: "user".into(),
            content: prompt,
            images: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        system: "You are a git commit message generator. Return only the commit message — \
                  one line, imperative mood, under 100 characters, no markdown."
            .into(),
        max_tokens: 120,
        temperature: 0.2,
        thinking_enabled: false,
        thinking_budget: 0,
    };
    let empty_tools: Vec<serde_json::Value> = vec![];
    match crate::llm::chat_with_tools(
        &ctx.llm_provider,
        &ctx.llm_api_key,
        ctx.llm_base_url.as_deref(),
        request,
        &empty_tools,
        None,
    )
    .await
    {
        Ok((crate::llm::LlmResponse::Text(text), _)) => {
            let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            let cleaned = line
                .trim()
                .trim_start_matches('`')
                .trim_end_matches('`')
                .trim();
            if cleaned.is_empty() {
                Some(fallback_commit_summary(&name_status))
            } else {
                Some(cleaned.to_string())
            }
        }
        _ => {
            log::warn!("[GitCommit] LLM summary failed — using fallback");
            Some(fallback_commit_summary(&name_status))
        }
    }
}

#[async_trait::async_trait]
impl Tool for GitCommit {
    fn name(&self) -> &str {
        "git_commit"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let auto_summary = args["auto_summary"].as_bool().unwrap_or(false);
        let mut message = args["message"].as_str().unwrap_or("").trim().to_string();

        let work_dir = ctx.project_path.as_deref().unwrap_or(".");
        let add_all = args["add_all"].as_bool().unwrap_or(true);

        // Step 1: Stage files if add_all is true
        if add_all {
            let add_output = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                tokio::process::Command::new("git")
                    .arg("add")
                    .arg("-A")
                    .current_dir(work_dir)
                    .output(),
            )
            .await;

            match add_output {
                Ok(Ok(out)) => {
                    if !out.status.success() {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        return format!("git add -A failed: {}", stderr.trim());
                    }
                }
                Ok(Err(e)) => return format!("Failed to execute git add: {}", e),
                Err(_) => return "git add timed out (15s)".to_string(),
            }
        }

        // Step 2: Derive the message from the staged diff (must run after staging)
        if message.is_empty() && auto_summary {
            match build_auto_message(work_dir, ctx).await {
                Some(m) => message = m,
                None => {
                    return "Error: nothing staged to summarize — run git add first or pass a message"
                        .to_string()
                }
            }
        }
        if message.is_empty() {
            return "Error: commit message is required (or pass auto_summary: true)".to_string();
        }

        // Step 2: Commit
        let commit_output = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            tokio::process::Command::new("git")
                .arg("commit")
                .arg("-m")
                .arg(&message)
                .current_dir(work_dir)
                .output(),
        )
        .await;

        match commit_output {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !out.status.success() {
                    let combined = format!("{}\n{}", stdout, stderr).trim().to_string();
                    return format!("git commit failed: {}", combined);
                }
                // Extract commit hash from output
                let hash = extract_commit_hash(&stdout);
                match hash {
                    Some(h) => format!("Committed successfully: {} ({})", message, h),
                    None => format!("Committed: {}", stdout.trim()),
                }
            }
            Ok(Err(e)) => format!("Failed to execute git commit: {}", e),
            Err(_) => "git commit timed out (15s)".to_string(),
        }
    }
}

/// Extract the short commit hash from git commit output.
/// Output typically contains: "[main abc1234] message"
fn extract_commit_hash(output: &str) -> Option<String> {
    for line in output.lines() {
        if line.starts_with('[') {
            // Format: [branch hash] message
            if let Some(close) = line.find(']') {
                let inner = &line[1..close];
                let parts: Vec<&str> = inner.split_whitespace().collect();
                if parts.len() >= 2 {
                    return Some(parts[1].to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_summary_single_file() {
        assert_eq!(
            fallback_commit_summary("M\tsrc/main.rs"),
            "Update src/main.rs"
        );
    }

    #[test]
    fn fallback_summary_multiple_files() {
        let ns = "M\tsrc/main.rs\nA\tsrc/lib.rs\nD\tREADME.md\nM\ttests/t.rs";
        let s = fallback_commit_summary(ns);
        assert!(s.starts_with("Update 4 files: src/main.rs, src/lib.rs, README.md"), "{}", s);
        assert!(s.ends_with("+1 more"), "{}", s);
    }

    #[test]
    fn fallback_summary_empty() {
        assert_eq!(fallback_commit_summary(""), "Update staged changes");
    }

    #[test]
    fn auto_summary_prompt_includes_all_sections() {
        let p = build_auto_summary_prompt(
            " 3 files changed",
            "M\tsrc/a.rs\nA\tsrc/b.rs",
            "509d7b9 Add feature X",
        );
        assert!(p.contains("509d7b9 Add feature X"), "{}", p);
        assert!(p.contains("3 files changed"), "{}", p);
        assert!(p.contains("src/b.rs"), "{}", p);
        assert!(p.contains("Commit message:"), "{}", p);
    }

    #[test]
    fn extracts_hash_from_commit_output() {
        assert_eq!(
            extract_commit_hash("[master 509d7b9] Add feature\n"),
            Some("509d7b9".to_string())
        );
        assert_eq!(extract_commit_hash("nothing here"), None);
    }
}
