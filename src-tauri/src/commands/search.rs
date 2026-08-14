use std::sync::Arc;
use tauri::{State, Manager};
use crate::config::AppSettings;
use crate::rag::{CodeIndexer, SearchResult};
use tokio::sync::RwLock;

#[tauri::command]
pub async fn search_codebase(
    indexer: State<'_, Arc<CodeIndexer>>,
    query: String,
    max_results: Option<usize>,
) -> Result<Vec<SearchResult>, String> {
    log::info!("Searching codebase for: {}", query);
    let max = max_results.unwrap_or(10);
    indexer.search(&query, max).await
}

/// Rebuild the code index.
///
/// Without an explicit `project_path` the active workspace (or legacy
/// `project_paths[0]`) is indexed; the index DB is persisted to the
/// per-workspace path so each workspace keeps an independent index.
#[tauri::command]
pub async fn reindex_project(
    app: tauri::AppHandle,
    indexer: State<'_, Arc<CodeIndexer>>,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
    project_path: Option<String>,
) -> Result<String, String> {
    // Resolve the target directory: explicit path > active workspace > legacy MRU
    let (target, db_path) = {
        let guard = settings.read().await;
        match project_path {
            Some(p) if !p.is_empty() => {
                let db = active_workspace_db(&guard, &app);
                (p, db)
            }
            _ => {
                let ws = guard
                    .active_workspace_id
                    .as_ref()
                    .and_then(|id| guard.workspaces.iter().find(|w| &w.id == id));
                match ws {
                    Some(ws) => (ws.path.clone(), ws.index_db_path.clone()),
                    None => (
                        guard.project_paths.first().cloned().ok_or(
                            "No project path: pass project_path or activate a workspace".to_string(),
                        )?,
                        legacy_db_path(&app),
                    ),
                }
            }
        }
    };

    log::info!("Reindexing project: {}", target);
    indexer.clear().await;
    let (files, chunks) = indexer.index_project(&target).await?;

    if let Err(e) = indexer.save_to_db(&db_path).await {
        log::warn!("Failed to persist index to DB {}: {}", db_path, e);
    }

    Ok(format!("Indexing complete: {} files, {} chunks", files, chunks))
}

/// Index DB path of the currently active workspace (empty string if none).
fn active_workspace_db(guard: &AppSettings, app: &tauri::AppHandle) -> String {
    guard
        .active_workspace_id
        .as_ref()
        .and_then(|id| guard.workspaces.iter().find(|w| &w.id == id))
        .map(|ws| {
            if ws.index_db_path.is_empty() {
                ws.index_db_path_for(&app.path().app_config_dir().unwrap_or_default())
            } else {
                ws.index_db_path.clone()
            }
        })
        .unwrap_or_else(|| legacy_db_path(app))
}

/// Legacy single global index DB path (kept for workspaces created before v2).
fn legacy_db_path(app: &tauri::AppHandle) -> String {
    app.path()
        .app_config_dir()
        .map(|p| p.join("code_index.db").to_string_lossy().to_string())
        .unwrap_or_else(|_| "code_index.db".to_string())
}

#[tauri::command]
pub async fn index_file(
    indexer: State<'_, Arc<CodeIndexer>>,
    file_path: String,
    content: String,
) -> Result<usize, String> {
    log::info!("Indexing file: {}", file_path);
    indexer.index_file(&file_path, &content).await
}

#[tauri::command]
pub async fn remove_from_index(
    indexer: State<'_, Arc<CodeIndexer>>,
    file_path: String,
) -> Result<(), String> {
    log::info!("Removing from index: {}", file_path);
    indexer.remove_file(&file_path).await;
    Ok(())
}

#[tauri::command]
pub async fn get_index_stats(
    indexer: State<'_, Arc<CodeIndexer>>,
) -> Result<usize, String> {
    Ok(indexer.chunk_count().await)
}
