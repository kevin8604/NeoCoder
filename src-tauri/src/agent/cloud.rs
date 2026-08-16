//! Cloud Agent — background asynchronous agent execution.
//!
//! Allows starting agent tasks that run independently of the frontend.
//! Tasks are tracked with status, and results can be retrieved later.
//! Optionally, completed tasks can auto-create GitHub PRs.

use std::collections::HashMap;
use std::path::PathBuf;
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
    /// Interrupted by app restart — was Running/Pending when the process died.
    /// Resumable via the `resume_cloud_task` command.
    Interrupted,
}

impl CloudTaskStatus {
    /// Stable lowercase label for the frontend / logs.
    pub fn label(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed(_) => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
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
    /// Agent definition id used for execution (resume re-runs with the same agent)
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// Shared state for cloud task management.
///
/// When constructed with [`CloudTaskManager::with_storage`], every state
/// change is persisted to a JSON file so tasks survive app restarts.
/// Tasks that were Running/Pending at load time are marked `Interrupted`
/// (the process died mid-flight) and can be resumed via `resume_cloud_task`.
pub struct CloudTaskManager {
    tasks: Mutex<HashMap<TaskId, CloudTask>>,
    /// Persistence file path (`None` disables persistence, used in tests)
    storage_path: Option<PathBuf>,
}

impl CloudTaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            storage_path: None,
        }
    }

    /// Construct with disk persistence: loads existing tasks from `path` and
    /// marks tasks that were Running/Pending (interrupted by a restart) as
    /// `Interrupted` so the user can resume them.
    pub fn with_storage(path: PathBuf) -> Self {
        let mut tasks = HashMap::new();
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(list) = serde_json::from_str::<Vec<CloudTask>>(&content) {
                for mut task in list {
                    if matches!(task.status, CloudTaskStatus::Pending | CloudTaskStatus::Running) {
                        task.status = CloudTaskStatus::Interrupted;
                    }
                    tasks.insert(task.id.clone(), task);
                }
                log::info!("[CloudTask] Loaded {} tasks from {}", tasks.len(), path.display());
            }
        }
        Self {
            tasks: Mutex::new(tasks),
            storage_path: Some(path),
        }
    }

    /// Write the current task snapshot back to disk (no-op without storage).
    async fn persist(&self) {
        let Some(path) = self.storage_path.clone() else { return; };
        let tasks = self.tasks.lock().await;
        let snapshot: Vec<CloudTask> = tasks.values().cloned().collect();
        drop(tasks);
        match serde_json::to_string_pretty(&snapshot) {
            Ok(json) => {
                if let Err(e) = tokio::fs::write(&path, json).await {
                    log::warn!("[CloudTask] Failed to persist tasks: {}", e);
                }
            }
            Err(e) => log::warn!("[CloudTask] Failed to serialize tasks: {}", e),
        }
    }

    /// Register a new pending task. Returns the task ID.
    pub async fn register(&self, task: CloudTask) -> TaskId {
        let id = task.id.clone();
        let mut tasks = self.tasks.lock().await;
        tasks.insert(id.clone(), task);
        drop(tasks);
        self.persist().await;
        id
    }

    /// Update task status.
    pub async fn update_status(&self, id: &str, status: CloudTaskStatus) {
        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.get_mut(id) {
            task.status = status;
        }
        drop(tasks);
        self.persist().await;
    }

    /// Mark task as completed with result.
    pub async fn complete(&self, id: &str, result: String) {
        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.get_mut(id) {
            task.status = CloudTaskStatus::Completed;
            task.result = Some(result);
            task.completed_at = Some(chrono::Utc::now().timestamp());
        }
        drop(tasks);
        self.persist().await;
    }

    /// Mark task as failed with error message.
    pub async fn fail(&self, id: &str, error: String) {
        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.get_mut(id) {
            task.status = CloudTaskStatus::Failed(error);
            task.completed_at = Some(chrono::Utc::now().timestamp());
        }
        drop(tasks);
        self.persist().await;
    }

    /// Set PR URL on a completed task.
    pub async fn set_pr_url(&self, id: &str, pr_url: String) {
        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.get_mut(id) {
            task.pr_url = Some(pr_url);
        }
        drop(tasks);
        self.persist().await;
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
            agent_id: Some(agent_id.clone()),
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
        "🤖 Auto-generated by NeoCoder Cloud Agent".to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_task(id: &str, status: CloudTaskStatus) -> CloudTask {
        CloudTask {
            id: id.to_string(),
            session_id: "s1".to_string(),
            status,
            message: format!("task {}", id),
            created_at: 1,
            completed_at: None,
            result: None,
            pr_config: None,
            pr_url: None,
            agent_id: None,
        }
    }

    #[tokio::test]
    async fn test_persist_and_reload_marks_interrupted() {
        let dir = std::env::temp_dir().join(format!("cloud-task-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cloud_tasks.json");

        let manager = CloudTaskManager::with_storage(path.clone());
        manager.register(sample_task("t1", CloudTaskStatus::Running)).await;
        manager.register(sample_task("t2", CloudTaskStatus::Completed)).await;

        // 状态变更后快照已写盘
        assert!(path.exists());

        // 模拟重启：从磁盘重新加载
        let reloaded = CloudTaskManager::with_storage(path.clone());
        assert_eq!(reloaded.list().await.len(), 2);
        // 运行中的任务被标记为 Interrupted，等待用户恢复
        assert_eq!(reloaded.get("t1").await.unwrap().status, CloudTaskStatus::Interrupted);
        // 已完成的任务原样保留
        assert_eq!(reloaded.get("t2").await.unwrap().status, CloudTaskStatus::Completed);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_status_updates_persist() {
        let dir = std::env::temp_dir().join(format!("cloud-task-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cloud_tasks.json");

        let manager = CloudTaskManager::with_storage(path.clone());
        manager.register(sample_task("t1", CloudTaskStatus::Pending)).await;
        manager.update_status("t1", CloudTaskStatus::Running).await;
        manager.complete("t1", "done".to_string()).await;

        let reloaded = CloudTaskManager::with_storage(path.clone());
        let task = reloaded.get("t1").await.unwrap();
        assert_eq!(task.status, CloudTaskStatus::Completed);
        assert_eq!(task.result.as_deref(), Some("done"));
        assert!(task.completed_at.is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_without_storage_skips_persistence() {
        let manager = CloudTaskManager::new();
        manager.register(sample_task("t1", CloudTaskStatus::Pending)).await;
        manager.update_status("t1", CloudTaskStatus::Running).await;
        assert_eq!(manager.list().await.len(), 1);
    }

    #[test]
    fn test_status_label() {
        assert_eq!(CloudTaskStatus::Pending.label(), "pending");
        assert_eq!(CloudTaskStatus::Running.label(), "running");
        assert_eq!(CloudTaskStatus::Completed.label(), "completed");
        assert_eq!(CloudTaskStatus::Failed("x".into()).label(), "failed");
        assert_eq!(CloudTaskStatus::Cancelled.label(), "cancelled");
        assert_eq!(CloudTaskStatus::Interrupted.label(), "interrupted");
    }
}
