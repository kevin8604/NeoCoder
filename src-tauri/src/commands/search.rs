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

#[tauri::command]
pub async fn reindex_project(
    app: tauri::AppHandle,
    indexer: State<'_, Arc<CodeIndexer>>,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
    project_path: String,
) -> Result<String, String> {
    log::info!("Reindexing project: {}", project_path);

    // Clear existing index
    indexer.clear().await;

    // Update embedding model from settings
    let settings = settings.read().await;
    let _ = &settings.embedding_model;

    // Index the project
    let (files, chunks) = indexer.index_project(&project_path).await?;

    // Persist to DB
    let db_path = app.path().app_config_dir()
        .map(|p| p.join("code_index.db").to_string_lossy().to_string())
        .unwrap_or_else(|_| "code_index.db".to_string());
    if let Err(e) = indexer.save_to_db(&db_path).await {
        log::warn!("Failed to persist index to DB: {}", e);
    }

    Ok(format!(
        "Indexing complete: {} files, {} chunks",
        files, chunks
    ))
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
