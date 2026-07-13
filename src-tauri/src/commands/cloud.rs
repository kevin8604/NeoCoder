//! Cloud Agent commands — background agent execution with status tracking.

use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use tokio::sync::RwLock;

use crate::agent::cloud::{CloudTask, CloudTaskManager, CloudTaskStatus, PrConfig, TaskId, create_github_pr};
use crate::agent::AgentInstance;
use crate::config::AppSettings;
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
    };

    task_manager.register(task).await;

    // Clone memory Arc before spawning (State has non-static lifetime)
    let memory_arc = chat_state.memory.clone();

    // Extract memory context before spawn (State lifetime issue)
    let memory_context = {
        let mem = memory_arc.read().await;
        mem.memory_manager().inject_memory_context()
    };

    // Spawn background task
    let app_clone = app.clone();
    let task_manager_clone: Arc<CloudTaskManager> = task_manager.inner().clone();
    let task_id_clone = task_id.clone();
    let agent_id = params.agent_id;
    let message = params.message;
    let session_id = params.session_id;
    let pr_config = params.pr_config;

    tokio::spawn(async move {
        task_manager_clone.update_status(&task_id_clone, CloudTaskStatus::Running).await;

        // Build AgentInstance
        let agent_def = agent_id.as_deref().and_then(|id| {
            app_clone.try_state::<crate::agent::definition::AgentRegistry>()
                .and_then(|registry| crate::agent::definition::find_agent(registry.inner(), id))
        });

        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let agent_instance = AgentInstance::new(
            app_clone.clone(),
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
                task_manager_clone.complete(&task_id_clone, final_text.clone()).await;

                // Auto-create PR if configured
                if let Some(ref pr_cfg) = pr_config {
                    log::info!("[CloudAgent] Creating PR for task {}", task_id_clone);
                    match create_github_pr(pr_cfg).await {
                        Ok(pr_url) => {
                            task_manager_clone.set_pr_url(&task_id_clone, pr_url.clone()).await;
                            log::info!("[CloudAgent] PR created: {}", pr_url);
                        }
                        Err(e) => {
                            log::error!("[CloudAgent] Failed to create PR: {}", e);
                        }
                    }
                }

                let _ = app_clone.emit("cloud-agent-event", serde_json::json!({
                    "type": "completed",
                    "task_id": task_id_clone,
                    "result": final_text,
                }));
            }
            Err(e) => {
                task_manager_clone.fail(&task_id_clone, e.clone()).await;
                let _ = app_clone.emit("cloud-agent-event", serde_json::json!({
                    "type": "failed",
                    "task_id": task_id_clone,
                    "error": e,
                }));
            }
        }

        // Persist result to conversation memory
        {
            let mem = memory_arc.write().await;
            let status = task_manager_clone.get(&task_id_clone).await;
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

    Ok(task_id)
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
