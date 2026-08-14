use std::collections::HashMap;
use std::sync::Arc;
use tauri::{Manager, State};
use crate::commands::config::ConfigState;
use crate::commands::skill::SkillState;
use crate::config::Workspace;
use crate::fs_watcher::FileWatcher;
use crate::fs_service::FileService;
use crate::rag::CodeIndexer;

/// Global file snapshots for EditDiff accept/reject
pub type FileSnapshots = Arc<std::sync::Mutex<HashMap<String, String>>>;

#[derive(serde::Serialize)]
pub struct FileTreeItem {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub children: Option<Vec<FileTreeItem>>,
}

/// Default ignore patterns for file tree listing
const IGNORE_DIRS: &[&str] = &[
    ".git", "node_modules", "target", "dist", "build",
    ".next", ".cache", "__pycache__", ".venv", "env",
    ".idea", ".vscode", ".vs", ".DS_Store",
];

fn should_ignore(name: &str) -> bool {
    name.starts_with('.') || IGNORE_DIRS.contains(&name)
}

fn list_directory(path: &std::path::Path, max_depth: u32, current_depth: u32) -> Vec<FileTreeItem> {
    let mut items = Vec::new();

    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return items,
    };

    let mut entries: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| !should_ignore(&e.file_name().to_string_lossy()))
        .collect();

    entries.sort_by_key(|e| (!e.file_type().map(|t| t.is_dir()).unwrap_or(false), e.file_name()));

    for entry in entries {
        let entry_path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);

        let children = if is_dir && current_depth < max_depth {
            let sub = list_directory(&entry_path, max_depth, current_depth + 1);
            Some(sub)
        } else if is_dir {
            Some(vec![]) // Show as expandable but don't load children
        } else {
            None
        };

        items.push(FileTreeItem {
            name,
            path: entry_path.to_string_lossy().to_string(),
            is_dir,
            children,
        });
    }

    items
}

/// Open (create-or-activate) a workspace directory.
///
/// If the canonical path is not yet registered as a workspace it is created;
/// either way the workspace becomes active: watcher + code index + project
/// skills are switched atomically (see `commands::workspace::activate_ws`).
#[tauri::command]
pub async fn open_project(
    path: String,
    app: tauri::AppHandle,
    state: State<'_, ConfigState>,
    watcher: State<'_, Arc<std::sync::Mutex<FileWatcher>>>,
    indexer: State<'_, Arc<CodeIndexer>>,
    skill_state: State<'_, SkillState>,
) -> Result<Workspace, String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("Path does not exist: {}", path));
    }
    if !p.is_dir() {
        return Err(format!("Path is not a directory: {}", path));
    }

    let canonical = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let canonical_str = canonical.to_string_lossy().to_string();
    let canonical_lower = canonical_str.to_lowercase();

    let mut settings = state.manager.write().await.get_settings().await;

    // Create the workspace entry if this path is new
    let workspace_id = {
        let existing = settings
            .workspaces
            .iter()
            .find(|w| w.path.to_lowercase() == canonical_lower)
            .map(|w| w.id.clone());
        match existing {
            Some(id) => id,
            None => {
                let ws = Workspace::new(canonical_str.clone());
                let id = ws.id.clone();
                settings.workspaces.push(ws);
                log::info!("[Workspace] Registered new workspace: {}", canonical_str);
                id
            }
        }
    };

    // Keep legacy project_paths list in sync (MRU, max 10) for older consumers
    settings.project_paths.retain(|pp| pp.to_lowercase() != canonical_lower);
    settings.project_paths.insert(0, canonical_str.clone());
    settings.project_paths.truncate(10);

    let config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let result = crate::commands::workspace::activate_ws(
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

#[tauri::command]
pub async fn get_file_tree(
    path: String,
    max_depth: Option<u32>,
) -> Result<Vec<FileTreeItem>, String> {
    let root = std::path::Path::new(&path);
    if !root.is_dir() {
        return Err("Path is not a directory".to_string());
    }

    let depth = max_depth.unwrap_or(2); // Default: 2 levels deep
    Ok(list_directory(root, depth, 0))
}

#[tauri::command]
pub async fn read_file(
    path: String,
) -> Result<String, String> {
    FileService::read_text(std::path::Path::new(&path), None, None)
}

#[tauri::command]
pub async fn write_file(
    path: String,
    content: String,
) -> Result<(), String> {
    FileService::write_text(std::path::Path::new(&path), &content, None, None, false)
}

#[tauri::command]
pub async fn create_file(
    path: String,
    content: Option<String>,
) -> Result<(), String> {
    FileService::write_text(std::path::Path::new(&path), content.as_deref().unwrap_or(""), None, None, false)
}

#[tauri::command]
pub async fn create_directory(
    path: String,
) -> Result<(), String> {
    FileService::create_dir_all(std::path::Path::new(&path), None, None)
}

#[tauri::command]
pub async fn delete_file(
    path: String,
) -> Result<(), String> {
    FileService::remove(std::path::Path::new(&path), None, None)
}

#[tauri::command]
pub async fn rename_file(
    source: String,
    destination: String,
) -> Result<(), String> {
    FileService::rename(std::path::Path::new(&source), std::path::Path::new(&destination), None, None)
}

/// Save file snapshots into global state for accept/reject
/// Called by AgentLoop after computing diffs
pub fn save_snapshots(snapshots: HashMap<String, String>, global: &FileSnapshots) {
    if let Ok(mut s) = global.lock() {
        *s = snapshots;
    }
}

#[tauri::command]
pub async fn accept_change(
    _snapshots: State<'_, FileSnapshots>,
    _file_path: String,
) -> Result<(), String> {
    // Changes are already applied by the Agent; just acknowledge
    // (snapshot will be overwritten on next Agent run)
    Ok(())
}

#[tauri::command]
pub async fn reject_change(
    snapshots: State<'_, FileSnapshots>,
    file_path: String,
) -> Result<(), String> {
    let original = {
        let s = snapshots.lock().map_err(|e| format!("Lock error: {}", e))?;
        s.get(&file_path).cloned()
    };
    if let Some(content) = original {
        // Restore original file content
        std::fs::write(&file_path, &content).map_err(|e| format!("Failed to restore file: {}", e))?;
        // Clear snapshot for this file
        if let Ok(mut s) = snapshots.lock() {
            s.remove(&file_path);
        }
        Ok(())
    } else {
        Err(format!("No snapshot found for file: {}", file_path))
    }
}

#[tauri::command]
pub async fn accept_all_changes(
    snapshots: State<'_, FileSnapshots>,
) -> Result<usize, String> {
    // Changes are already applied by the Agent; just acknowledge all
    let count = {
        let s = snapshots.lock().map_err(|e| format!("Lock error: {}", e))?;
        s.len()
    };
    // Clear all snapshots
    if let Ok(mut s) = snapshots.lock() {
        s.clear();
    }
    Ok(count)
}

#[tauri::command]
pub async fn reject_all_changes(
    snapshots: State<'_, FileSnapshots>,
) -> Result<usize, String> {
    let entries: Vec<(String, String)> = {
        let s = snapshots.lock().map_err(|e| format!("Lock error: {}", e))?;
        s.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    };

    let count = entries.len();
    let mut errors: Vec<String> = Vec::new();

    for (path, original) in entries {
        if let Err(e) = std::fs::write(&path, &original) {
            errors.push(format!("{}: {}", path, e));
        }
    }

    // Clear all snapshots
    if let Ok(mut s) = snapshots.lock() {
        s.clear();
    }

    if !errors.is_empty() {
        return Err(format!("Some files failed to restore: {}", errors.join("; ")));
    }
    Ok(count)
}
