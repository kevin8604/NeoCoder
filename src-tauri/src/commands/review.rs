use std::process::Command;
use std::sync::Arc;
use tauri::State;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::config::AppSettings;

/// Trigger an automatic code review on the current git changes.
/// Returns the session_id for tracking the review conversation.
#[tauri::command]
pub async fn trigger_auto_review(
    project_path: String,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
) -> Result<String, String> {
    let _settings = settings.read().await;

    // Get git diff (prefer staged changes, fallback to unstaged)
    let diff = get_git_diff(&project_path)?;

    if diff.trim().is_empty() {
        return Err("No changes to review".to_string());
    }

    // Truncate very large diffs
    let diff = if diff.len() > 100_000 {
        let truncated: String = diff.chars().take(100_000).collect();
        format!("{}... [truncated at 100KB]", truncated)
    } else {
        diff
    };

    // Build the review prompt
    let review_prompt = format!(
        r#"You are performing an automated code review on recent changes.

Analyze the following git diff and provide a structured review.

## Review Criteria
- **Bugs**: Logic errors, null pointer risks, race conditions
- **Security**: Injection vulnerabilities, data exposure, auth issues
- **Performance**: N+1 queries, unnecessary allocations, blocking calls
- **Style**: Naming conventions, code organization, readability
- **Best Practices**: Error handling, edge cases, documentation

## Changes to Review
```diff
{}
```

## Output Format
Provide your review in this format:

### Summary
Brief overview of the changes and overall assessment.

### Issues Found
List any issues with severity (🔴 Critical / 🟡 Warning / 🔵 Suggestion):
- [severity] file:line - description

### Recommendations
Actionable suggestions for improvement.

### Verdict
✅ Approve / ⚠️ Approve with comments / ❌ Request changes"#,
        diff
    );

    // Create a new session for the review
    let session_id = Uuid::new_v4().to_string();

    // Store the review prompt in the session (using the chat memory system)
    // For now, we'll emit an event to the frontend to start the review in chat
    log::info!(
        "[AutoReview] Triggered review for project: {}, diff size: {} bytes",
        project_path,
        diff.len()
    );

    // Return the session_id and prompt for the frontend to handle
    // The frontend will call send_message with this prompt
    Ok(format!("{}|||{}", session_id, review_prompt))
}

/// Get git diff from the project directory
fn get_git_diff(project_path: &str) -> Result<String, String> {
    // Try staged changes first
    let staged = Command::new("git")
        .args(["diff", "--cached"])
        .current_dir(project_path)
        .output()
        .map_err(|e| format!("Failed to run git: {}", e))?;

    if staged.status.success() {
        let diff = String::from_utf8_lossy(&staged.stdout).to_string();
        if !diff.trim().is_empty() {
            return Ok(diff);
        }
    }

    // Fallback to unstaged changes
    let unstaged = Command::new("git")
        .args(["diff"])
        .current_dir(project_path)
        .output()
        .map_err(|e| format!("Failed to run git: {}", e))?;

    if unstaged.status.success() {
        let diff = String::from_utf8_lossy(&unstaged.stdout).to_string();
        if !diff.trim().is_empty() {
            return Ok(diff);
        }
    }

    // Try diff against HEAD (includes both staged and unstaged)
    let head = Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(project_path)
        .output()
        .map_err(|e| format!("Failed to run git: {}", e))?;

    if head.status.success() {
        return Ok(String::from_utf8_lossy(&head.stdout).to_string());
    }

    Err("No git changes found".to_string())
}

/// Check if auto-review is enabled for the given trigger type
#[tauri::command]
pub async fn get_auto_review_settings(
    settings: State<'_, Arc<RwLock<AppSettings>>>,
) -> Result<AutoReviewSettings, String> {
    let settings = settings.read().await;
    Ok(AutoReviewSettings {
        on_save: settings.auto_review_on_save,
        on_commit: settings.auto_review_on_commit,
    })
}

#[derive(serde::Serialize)]
pub struct AutoReviewSettings {
    pub on_save: bool,
    pub on_commit: bool,
}
