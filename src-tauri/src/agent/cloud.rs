//! Cloud Agent — background asynchronous agent execution.
//!
//! Allows starting agent tasks that run independently of the frontend.
//! Tasks are tracked with status, and results can be retrieved later.
//! Optionally, completed tasks can auto-create GitHub PRs.

use std::collections::HashMap;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;

use crate::agent::AgentInstance;

/// Unique task identifier.
pub type TaskId = String;

/// Status of a cloud agent task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CloudTaskStatus {
    /// Queued, waiting to start
    Pending,
    /// Currently executing
    Running,
    /// Completed successfully
    Completed,
    /// Failed with an error message
    Failed(String),
    /// Cancelled by user
    Cancelled,
}

/// Configuration for auto-creating a GitHub PR on completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrConfig {
    /// Local path to the git repository
    pub repo_path: String,
    /// Branch name to create/use for the PR
    pub branch_name: String,
    /// Git commit message for agent changes
    pub commit_message: String,
    /// PR title
    pub pr_title: String,
    /// PR body/description
    #[serde(default)]
    pub pr_description: String,
    /// GitHub remote name (default: "origin")
    #[serde(default = "default_remote")]
    pub remote: String,
    /// Base branch to target (default: "main")
    #[serde(default = "default_base_branch")]
    pub base_branch: String,
}

fn default_remote() -> String { "origin".to_string() }
fn default_base_branch() -> String { "main".to_string() }

/// A single cloud agent task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudTask {
    pub id: TaskId,
    pub session_id: String,
    pub status: CloudTaskStatus,
    /// Original user message
    pub message: String,
    /// Unix timestamp when created
    pub created_at: i64,
    /// Unix timestamp when completed/failed
    pub completed_at: Option<i64>,
    /// Final output text (if completed)
    pub result: Option<String>,
    /// PR configuration (if PR should be created on completion)
    pub pr_config: Option<PrConfig>,
    /// PR URL (if PR was created)
    pub pr_url: Option<String>,
}

/// Shared state for cloud task management.
pub struct CloudTaskManager {
    tasks: Mutex<HashMap<TaskId, CloudTask>>,
}

impl CloudTaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
        }
    }

    /// Register a new pending task. Returns the task ID.
    pub async fn register(&self, task: CloudTask) -> TaskId {
        let id = task.id.clone();
        let mut tasks = self.tasks.lock().await;
        tasks.insert(id.clone(), task);
        id
    }

    /// Update task status.
    pub async fn update_status(&self, id: &str, status: CloudTaskStatus) {
        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.get_mut(id) {
            task.status = status;
        }
    }

    /// Mark task as completed with result.
    pub async fn complete(&self, id: &str, result: String) {
        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.get_mut(id) {
            task.status = CloudTaskStatus::Completed;
            task.result = Some(result);
            task.completed_at = Some(chrono::Utc::now().timestamp());
        }
    }

    /// Mark task as failed with error message.
    pub async fn fail(&self, id: &str, error: String) {
        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.get_mut(id) {
            task.status = CloudTaskStatus::Failed(error);
            task.completed_at = Some(chrono::Utc::now().timestamp());
        }
    }

    /// Set PR URL on a completed task.
    pub async fn set_pr_url(&self, id: &str, pr_url: String) {
        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.get_mut(id) {
            task.pr_url = Some(pr_url);
        }
    }

    /// Get a single task by ID.
    pub async fn get(&self, id: &str) -> Option<CloudTask> {
        let tasks = self.tasks.lock().await;
        tasks.get(id).cloned()
    }

    /// List all tasks, newest first.
    pub async fn list(&self) -> Vec<CloudTask> {
        let tasks = self.tasks.lock().await;
        let mut list: Vec<_> = tasks.values().cloned().collect();
        list.sort_by_key(|t| -t.created_at);
        list
    }
}

/// Spawn a sub-agent in the background (fire-and-forget with completion
/// notification). Reuses the same execution template as `start_cloud_agent`:
/// register a CloudTask → spawn → run AgentInstance → complete/fail → emit
/// "cloud-agent-event" → persist result to the session.
///
/// Deliberately a *synchronous* function: it only resolves state and calls
/// `tokio::spawn`, so it can be called from the agent main loop (which runs
/// inside another `tokio::spawn` with `Send` requirements) without the
/// `Send` propagation problem an `async fn` would cause.
///
/// Returns the task ID on success (tool result), or an error message.
pub fn spawn_background_sub_agent(
    app: tauri::AppHandle,
    session_id: String,
    task: String,
    agent_id: String,
    project_path: Option<String>,
) -> Result<String, String> {
    // Resolve task manager (registered in lib.rs setup)
    let task_manager = app
        .try_state::<crate::commands::cloud::CloudTaskState>()
        .map(|s| s.inner().clone())
        .ok_or_else(|| "Error: CloudTaskManager not available".to_string())?;

    // Resolve chat memory for context injection + result persistence
    let memory_arc = app
        .try_state::<crate::commands::chat::ChatState>()
        .map(|s| s.memory.clone())
        .ok_or_else(|| "Error: ChatState not available".to_string())?;

    // Read LLM settings (non-blocking pattern)
    let (provider, api_key, chat_model) = app
        .try_state::<Arc<tokio::sync::RwLock<crate::config::AppSettings>>>()
        .map(|s| {
            let guard = tokio::task::block_in_place(|| s.blocking_read());
            (
                guard.llm_provider.clone(),
                guard.api_key.clone(),
                guard.chat_model.clone(),
            )
        })
        .unwrap_or_else(|| {
            (crate::config::LlmProvider::OpenAI, String::new(), String::new())
        });

    let task_id = format!("bg-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("task"));
    let app_clone = app.clone();
    let task_manager_clone = task_manager.clone();
    let task_id_clone = task_id.clone();
    let session_id_clone = session_id.clone();

    // NOTE: the function body itself does no `.await` — everything (register,
    // memory context extraction, agent run, persistence) happens inside the
    // spawned task, so this function's future stays `Send` and can be awaited
    // from the agent main loop (which runs inside another `tokio::spawn`).
    tokio::spawn(async move {
        let task_record = CloudTask {
            id: task_id_clone.clone(),
            session_id: session_id_clone.clone(),
            status: CloudTaskStatus::Pending,
            message: format!("[background sub-agent: {}] {}", agent_id, task),
            created_at: chrono::Utc::now().timestamp(),
            completed_at: None,
            result: None,
            pr_config: None,
            pr_url: None,
        };
        task_manager_clone.register(task_record).await;
        task_manager_clone
            .update_status(&task_id_clone, CloudTaskStatus::Running)
            .await;

        // Extract memory context inside the task (State lifetime issue)
        let memory_context = {
            let mem = memory_arc.read().await;
            mem.memory_manager().inject_memory_context()
        };

        // Resolve agent definition (optional)
        let agent_def = app_clone
            .try_state::<crate::agent::definition::AgentRegistry>()
            .and_then(|registry| crate::agent::definition::find_agent(registry.inner(), &agent_id));

        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let agent_instance = AgentInstance::new(
            app_clone.clone(),
            session_id_clone.clone(),
            vec![crate::llm::ChatMessage::text("user", &task)],
            provider,
            api_key,
            None, // base_url
            chat_model,
            project_path,
            None, // custom_instructions
            cancelled.clone(),
            agent_def.as_ref(),
            Some(memory_context),
        );

        let agent = Arc::new(tokio::sync::Mutex::new(agent_instance));
        let result = {
            let mut agent = agent.lock().await;
            agent.run().await
        };

        match result {
            Ok(final_text) => {
                task_manager_clone.complete(&task_id_clone, final_text.clone()).await;
                let _ = app_clone.emit("cloud-agent-event", serde_json::json!({
                    "type": "completed",
                    "task_id": task_id_clone,
                    "source": "background_sub_agent",
                    "result": final_text,
                }));
            }
            Err(e) => {
                task_manager_clone.fail(&task_id_clone, e.clone()).await;
                let _ = app_clone.emit("cloud-agent-event", serde_json::json!({
                    "type": "failed",
                    "task_id": task_id_clone,
                    "source": "background_sub_agent",
                    "error": e,
                }));
            }
        }

        // Persist result to conversation memory
        {
            let mem = memory_arc.write().await;
            let status = task_manager_clone.get(&task_id_clone).await;
            if let Some(task_record) = status {
                if let Some(ref result_text) = task_record.result {
                    mem.add_message(&session_id_clone, crate::chat::ChatMessage {
                        role: crate::chat::Role::Assistant,
                        content: format!("[Background Agent Result: {}]\n\n{}", agent_id, result_text),
                        images: None,
                        tool_calls: None,
                    });
                }
            }
        }
    });

    Ok(task_id)
}

/// Attempt to create a GitHub PR from agent changes.
///
/// Steps:
/// 1. cd to repo_path
/// 2. Create and switch to branch_name
/// 3. git add -A && git commit -m "message"
/// 4. git push to remote/branch
/// 5. Use `gh pr create` to create the PR
///
/// Returns the PR URL on success.
pub async fn create_github_pr(config: &PrConfig) -> Result<String, String> {
    use tokio::process::Command;

    let repo = &config.repo_path;
    let branch = &config.branch_name;
    let remote = &config.remote;
    let base = &config.base_branch;

    // 1. Create and switch to branch
    let output = Command::new("git")
        .args(["-C", repo, "checkout", "-b", branch])
        .output()
        .await
        .map_err(|e| format!("git checkout failed: {}", e))?;

    if !output.status.success() {
        // Try switching if branch exists
        let _ = Command::new("git")
            .args(["-C", repo, "checkout", branch])
            .output()
            .await;
    }

    // 2. Stage and commit
    let _ = Command::new("git")
        .args(["-C", repo, "add", "-A"])
        .output()
        .await
        .map_err(|e| format!("git add failed: {}", e))?;

    let _output = Command::new("git")
        .args(["-C", repo, "commit", "-m", &config.commit_message])
        .output()
        .await
        .map_err(|e| format!("git commit failed: {}", e))?;

    // If nothing to commit, that's fine
    let _ = _output;

    // 3. Push
    let output = Command::new("git")
        .args(["-C", repo, "push", "-u", remote, branch])
        .output()
        .await
        .map_err(|e| format!("git push failed: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git push failed: {}", stderr));
    }

    // 4. Create PR via GitHub CLI
    let pr_title = &config.pr_title;
    let pr_body = if config.pr_description.is_empty() {
        "🤖 Auto-generated by NeeCoder Cloud Agent".to_string()
    } else {
        config.pr_description.clone()
    };

    let output = Command::new("gh")
        .args([
            "pr", "create",
            "--repo", repo,
            "--head", branch,
            "--base", base,
            "--title", pr_title,
            "--body", &pr_body,
        ])
        .output()
        .await
        .map_err(|e| format!("gh pr create failed: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh pr create failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let pr_url = stdout.trim().to_string();

    Ok(pr_url)
}
