use std::sync::Arc;
use tauri::State;
use crate::lsp::{LSPHoverInfo, LSPSymbol, LspManager};

#[tauri::command]
pub async fn start_lsp(
    lsp_manager: State<'_, Arc<LspManager>>,
    language: String,
    root_uri: String,
) -> Result<(), String> {
    log::info!("Starting LSP for {} at {}", language, root_uri);
    lsp_manager.get_or_start(&language, &root_uri).await
}

#[tauri::command]
pub async fn get_symbols(
    lsp_manager: State<'_, Arc<LspManager>>,
    language: String,
    file_path: String,
) -> Result<Vec<LSPSymbol>, String> {
    log::info!("Getting symbols for {} ({})", file_path, language);
    lsp_manager.get_symbols(&language, &file_path).await
}

#[tauri::command]
pub async fn get_hover_info(
    lsp_manager: State<'_, Arc<LspManager>>,
    language: String,
    file_path: String,
    line: u32,
    column: u32,
) -> Result<Option<LSPHoverInfo>, String> {
    log::info!("Getting hover info for {}:{}:{}", file_path, line, column);
    lsp_manager.get_hover(&language, &file_path, line, column).await
}

#[tauri::command]
pub async fn lsp_did_open(
    lsp_manager: State<'_, Arc<LspManager>>,
    language: String,
    file_path: String,
    file_text: String,
) -> Result<(), String> {
    lsp_manager.did_open(&language, &file_path, &file_text).await
}

#[tauri::command]
pub async fn lsp_did_change(
    lsp_manager: State<'_, Arc<LspManager>>,
    language: String,
    file_path: String,
    text: String,
    version: i32,
) -> Result<(), String> {
    lsp_manager.did_change(&language, &file_path, &text, version).await
}

#[tauri::command]
pub async fn lsp_did_close(
    lsp_manager: State<'_, Arc<LspManager>>,
    language: String,
    file_path: String,
) -> Result<(), String> {
    lsp_manager.did_close(&language, &file_path).await
}

#[tauri::command]
pub async fn shutdown_lsp(
    lsp_manager: State<'_, Arc<LspManager>>,
) -> Result<(), String> {
    log::info!("Shutting down all LSP clients");
    lsp_manager.shutdown_all().await;
    Ok(())
}
