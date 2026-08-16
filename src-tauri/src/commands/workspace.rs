//! Multi-workspace runtime commands.
//!
//! Each registered workspace owns an independent code index database, a file
//! watcher session and a project-level skills directory. Switching workspaces
//! atomically swaps all three without restarting the app.

use std::sync::Arc;
use tauri::{Manager, State};

use crate::commands::config::ConfigState;
use crate::commands::skill::SkillState;
use crate::config::{AppSettings, Workspace};
use crate::fs_watcher::FileWatcher;
use crate::rag::CodeIndexer;

/// Activate a workspace by swapping watcher / index / project skills.
/// The workspace entry must already exist in `settings.workspaces`; it is
/// updated in place (last_opened_at + resolved index db path) and returned.
pub(crate) async fn activate_ws(
    settings: &mut AppSettings,
    workspace_id: &str,
    watcher: &Arc<std::sync::Mutex<FileWatcher>>,
    indexer: &Arc<CodeIndexer>,
    skill_manager: &SkillState,
    config_dir: &std::path::Path,
) -> Result<Workspace, String> {
    let ws = settings
        .workspaces
        .iter_mut()
        .find(|w| w.id == workspace_id)
        .ok_or_else(|| format!("Workspace '{}' not found", workspace_id))?;

    ws.last_opened_at = chrono::Utc::now().timestamp();
    if ws.index_db_path.is_empty() {
        ws.index_db_path = ws.index_db_path_for(config_dir);
    }
    let snapshot = ws.clone();
    let db_path = snapshot.index_db_path.clone();
    let ws_path = snapshot.path.clone();

    // 1. File watcher: stop all old watches, start watching the new workspace
    {
        let mut w = watcher.lock().unwrap_or_else(|e| e.into_inner());
        w.stop_all();
        if let Err(e) = w.start_watch(std::path::Path::new(&ws_path), true) {
            log::warn!("[Workspace] Failed to watch '{}': {}", ws_path, e);
        } else {
            log::info!("[Workspace] Watching project: {}", ws_path);
        }
    }

    // 2. Code index: load the per-workspace index DB (empty index on first open)
    indexer.clear().await;
    match indexer.load_from_db(&db_path).await {
        Ok(n) if n > 0 => log::info!("[Workspace] Loaded {} chunks from {}", n, db_path),
        Ok(_) => log::info!("[Workspace] Index DB empty or missing: {}", db_path),
        Err(e) => log::warn!("[Workspace] Failed to load index DB {}: {}", db_path, e),
    }

    // 3. Project skills: point at <workspace>/.neocoder/skills and reload.
    //    Fall back to the legacy `.neecoder` dir so pre-rename projects keep
    //    their existing skills.
    let project_dir = std::path::Path::new(&ws_path);
    let skills_dir = if project_dir.join(".neocoder").exists() {
        project_dir.join(".neocoder").join("skills")
    } else {
        project_dir.join(".neecoder").join("skills")
    };
    skill_manager.update_project_dir(Some(skills_dir));

    settings.active_workspace_id = Some(workspace_id.to_string());
    log::info!(
        "[Workspace] Activated '{}' ({}), index db: {}",
        snapshot.name,
        ws_path,
        db_path
    );

    Ok(snapshot)
}

/// List all registered workspaces, most recently opened first.
#[tauri::command]
pub async fn list_workspaces(state: State<'_, ConfigState>) -> Result<Vec<Workspace>, String> {
    let mut settings = state.manager.read().await.get_settings().await;
    settings.workspaces.sort_by(|a, b| b.last_opened_at.cmp(&a.last_opened_at));
    Ok(settings.workspaces)
}

/// Activate a registered workspace (switches watcher, index and project skills).
#[tauri::command]
pub async fn activate_workspace(
    app: tauri::AppHandle,
    workspace_id: String,
    state: State<'_, ConfigState>,
    watcher: State<'_, Arc<std::sync::Mutex<FileWatcher>>>,
    indexer: State<'_, Arc<CodeIndexer>>,
    skill_state: State<'_, SkillState>,
) -> Result<Workspace, String> {
    let mut settings = state.manager.write().await.get_settings().await;
    let config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let result = activate_ws(
        &mut settings,
        &workspace_id,
        watcher.inner(),
        indexer.inner(),
        skill_state.inner(),
        &config_dir,
    )
    .await?;
    state.manager.write().await.update_settings(settings).await?;
    Ok(result)
}

/// Remove a workspace entry (and its per-workspace index DB directory).
#[tauri::command]
pub async fn remove_workspace(
    app: tauri::AppHandle,
    workspace_id: String,
    state: State<'_, ConfigState>,
) -> Result<(), String> {
    let mut settings = state.manager.write().await.get_settings().await;
    let removed = settings
        .workspaces
        .iter()
        .find(|w| w.id == workspace_id)
        .cloned();

    let Some(ws) = removed else {
        return Err(format!("Workspace '{}' not found", workspace_id));
    };

    settings.workspaces.retain(|w| w.id != workspace_id);
    if settings.active_workspace_id.as_deref() == Some(workspace_id.as_str()) {
        settings.active_workspace_id = None;
    }

    // Best-effort cleanup of the per-workspace index directory
    let config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let ws_dir = config_dir.join("workspaces").join(&ws.id);
    if ws_dir.exists() {
        let _ = std::fs::remove_dir_all(&ws_dir);
        log::info!("[Workspace] Removed index directory: {}", ws_dir.display());
    }

    state.manager.write().await.update_settings(settings).await?;
    log::info!("[Workspace] Removed workspace '{}' ({})", ws.name, ws.path);
    Ok(())
}

/// Rename a workspace entry (display name only; path is unchanged).
#[tauri::command]
pub async fn rename_workspace(
    workspace_id: String,
    new_name: String,
    state: State<'_, ConfigState>,
) -> Result<(), String> {
    let new_name = new_name.trim().to_string();
    if new_name.is_empty() {
        return Err("Workspace name cannot be empty".to_string());
    }

    let mut settings = state.manager.write().await.get_settings().await;
    let ws = settings
        .workspaces
        .iter_mut()
        .find(|w| w.id == workspace_id)
        .ok_or_else(|| format!("Workspace '{}' not found", workspace_id))?;
    ws.name = new_name;

    state.manager.write().await.update_settings(settings).await?;
    Ok(())
}
