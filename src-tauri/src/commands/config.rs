use tauri::{State, Manager};
use crate::config::{AppSettings, ConfigManager};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ConfigState {
    pub manager: Arc<RwLock<ConfigManager>>,
}

#[tauri::command]
pub async fn get_settings(
    state: State<'_, ConfigState>,
) -> Result<AppSettings, String> {
    let manager = state.manager.read().await;
    Ok(manager.get_settings().await)
}

#[tauri::command]
pub async fn update_settings(
    state: State<'_, ConfigState>,
    settings: AppSettings,
) -> Result<(), String> {
    let manager = state.manager.write().await;
    manager.update_settings(settings).await
}

/// 读取最近 N 行日志（默认 200 行）
#[tauri::command]
pub async fn get_app_logs(
    app: tauri::AppHandle,
    lines: Option<usize>,
) -> Result<String, String> {
    let app_data = app.path().app_config_dir()
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(".neocoder"));
    let content = crate::logging::read_recent_logs(&app_data, lines.unwrap_or(200));
    Ok(content)
}

/// 获取日志文件路径
#[tauri::command]
pub async fn get_log_path(
    app: tauri::AppHandle,
) -> Result<String, String> {
    let app_data = app.path().app_config_dir()
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(".neocoder"));
    Ok(crate::logging::log_file_path(&app_data).to_string_lossy().to_string())
}
