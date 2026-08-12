//! Cloud Agent commands — background agent execution with status tracking.

use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use tokio::sync::RwLock;

use crate::agent::cloud::{CloudTask, CloudTaskManager, CloudTaskStatus, PrConfig, TaskId, create_github_pr};
use crate::agent::AgentInstance;
use crate::chat::ConversationMemory;
use crate::config::{AppSettings, LlmProvider};
use crate::commands::chat::ChatState;

/// Parameters for starting a cloud agent task.
#[derive(serde::Deserialize)]
pub struct StartCloudAgentParams {
    pub message: String,
    pub session_id: String,
    pub agent_id: Option<String>,
    /// Optional PR configuration for auto-PR on completion.
    pub pr_config: Option<PrConfig>,
}

/// Cloud task manager state type.
pub type CloudTaskState = Arc<CloudTaskManager>;

/// Shared execution pipeline for cloud tasks (used by `start_cloud_agent` and
/// `resume_cloud_task`): mark Running → run AgentInstance → complete/fail →
/// emit event → persist result to conversation memory.
///
/// Deliberately a *synchronous* function: it only resolves state and calls
/// `tokio::spawn`, so callers don't carry the spawned future's `Send` bounds.
#[allow(clippy::too_many_arguments)]
fn spawn_task_execution(
    app: tauri::AppHandle,
    task_manager: Arc<CloudTaskManager>,
    memory_arc: Arc<RwLock<ConversationMemory>>,
    task_id: String,
    session_id: String,
    message: String,
    agent_id: Option<String>,
    pr_config: Option<PrConfig>,
    provider: LlmProvider,
    api_key: String,
    chat_model: String,
    project_path: Option<String>,
    memory_context: String,
) {
    tokio::spawn(async move {
        task_manager.update_status(&task_id, CloudTaskStatus::Running).await;

        // Build AgentInstance
        let agent_def = agent_id.as_deref().and_then(|id| {
            app.try_state::<crate::agent::definition::AgentRegistry>()
                .and_then(|registry| crate::agent::definition::find_agent(registry.inner(), id))
        });

        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let agent_instance = AgentInstance::new(
            app.clone(),
            session_id.clone(),
            vec![crate::llm::ChatMessage::text("user", &message)],
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

        // Run agent in background (we don't await the stream)
        let agent = Arc::new(tokio::sync::Mutex::new(agent_instance));
        let result = {
            let mut agent = agent.lock().await;
            agent.run().await
        };

        match result {
            Ok(final_text) => {
                task_manager.complete(&task_id, final_text.clone()).await;

                // Auto-create PR if configured
                if let Some(ref pr_cfg) = pr_config {
                    log::info!("[CloudAgent] Creating PR for task {}", task_id);
                    match create_github_pr(pr_cfg).await {
                        Ok(pr_url) => {
                            task_manager.set_pr_url(&task_id, pr_url.clone()).await;
                            log::info!("[CloudAgent] PR created: {}", pr_url);
                        }
                        Err(e) => {
                            log::error!("[CloudAgent] Failed to create PR: {}", e);
                        }
                    }
                }

                let _ = app.emit("cloud-agent-event", serde_json::json!({
                    "type": "completed",
                    "task_id": task_id,
                    "result": final_text,
                }));
            }
            Err(e) => {
                task_manager.fail(&task_id, e.clone()).await;
                let _ = app.emit("cloud-agent-event", serde_json::json!({
                    "type": "failed",
                    "task_id": task_id,
                    "error": e,
                }));
            }
        }

        // Persist result to conversation memory
        {
            let mem = memory_arc.write().await;
            let status = task_manager.get(&task_id).await;
            if let Some(task) = status {
                if let Some(ref result_text) = task.result {
                    mem.add_message(&session_id, crate::chat::ChatMessage {
                        role: crate::chat::Role::Assistant,
                        content: format!("[Cloud Agent Result]\n\n{}", result_text),
                        images: None,
                        tool_calls: None,
                    });
                }
            }
        }
    });
}

/// Start a background agent task. Returns the task ID immediately.
#[tauri::command]
pub async fn start_cloud_agent(
    app: tauri::AppHandle,
    params: StartCloudAgentParams,
    task_manager: State<'_, CloudTaskState>,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
    chat_state: State<'_, ChatState>,
) -> Result<TaskId, String> {
    let task_id = format!("cloud-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("task"));

    let settings = settings.read().await;
    let provider = settings.llm_provider.clone();
    let api_key = settings.api_key.clone();
    let chat_model = settings.chat_model.clone();
    let project_path = settings.project_paths.first().cloned();
    drop(settings);

    let task = CloudTask {
        id: task_id.clone(),
        session_id: params.session_id.clone(),
        status: CloudTaskStatus::Pending,
        message: params.message.clone(),
        created_at: chrono::Utc::now().timestamp(),
        completed_at: None,
        result: None,
        pr_config: params.pr_config.clone(),
        pr_url: None,
        agent_id: params.agent_id.clone(),
    };

    task_manager.register(task).await;

    // Clone memory Arc before spawning (State has non-static lifetime)
    let memory_arc = chat_state.memory.clone();

    // Extract memory context before spawn (State lifetime issue)
    let memory_context = {
        let mem = memory_arc.read().await;
        mem.memory_manager().inject_memory_context()
    };

    spawn_task_execution(
        app,
        task_manager.inner().clone(),
        memory_arc,
        task_id.clone(),
        params.session_id,
        params.message,
        params.agent_id,
        params.pr_config,
        provider,
        api_key,
        chat_model,
        project_path,
        memory_context,
    );

    Ok(task_id)
}

/// Resume an interrupted (or failed) cloud agent task.
///
/// Re-runs the original message with a fresh agent instance; the task keeps
/// its id so the frontend can track the same card through completion.
#[tauri::command]
pub async fn resume_cloud_task(
    app: tauri::AppHandle,
    task_id: String,
    task_manager: State<'_, CloudTaskState>,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
    chat_state: State<'_, ChatState>,
) -> Result<(), String> {
    let task = task_manager
        .get(&task_id)
        .await
        .ok_or_else(|| format!("Task '{}' not found", task_id))?;
    if !matches!(task.status, CloudTaskStatus::Interrupted | CloudTaskStatus::Failed(_)) {
        return Err(format!(
            "Task '{}' is not resumable (status: {})",
            task_id,
            task.status.label()
        ));
    }

    // Re-read run configuration (fresh settings may differ from submission time)
    let settings = settings.read().await;
    let provider = settings.llm_provider.clone();
    let api_key = settings.api_key.clone();
    let chat_model = settings.chat_model.clone();
    let project_path = settings.project_paths.first().cloned();
    drop(settings);

    let memory_arc = chat_state.memory.clone();
    let memory_context = {
        let mem = memory_arc.read().await;
        mem.memory_manager().inject_memory_context()
    };

    spawn_task_execution(
        app,
        task_manager.inner().clone(),
        memory_arc,
        task_id,
        task.session_id,
        task.message,
        task.agent_id,
        task.pr_config,
        provider,
        api_key,
        chat_model,
        project_path,
        memory_context,
    );

    Ok(())
}

/// Get the status of a cloud agent task.
#[tauri::command]
pub async fn get_cloud_task(
    task_id: String,
    task_manager: State<'_, CloudTaskState>,
) -> Result<Option<CloudTask>, String> {
    Ok(task_manager.get(&task_id).await)
}

/// List all cloud agent tasks.
#[tauri::command]
pub async fn list_cloud_tasks(
    task_manager: State<'_, CloudTaskState>,
) -> Result<Vec<CloudTask>, String> {
    Ok(task_manager.list().await)
}

/// Cancel a running cloud agent task.
#[tauri::command]
pub async fn cancel_cloud_task(
    task_id: String,
    task_manager: State<'_, CloudTaskState>,
) -> Result<(), String> {
    task_manager.update_status(&task_id, CloudTaskStatus::Cancelled).await;
    Ok(())
}
