use std::collections::HashMap;
use std::sync::Arc;
use tauri::State;
use crate::commands::config::ConfigState;

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

#[tauri::command]
pub async fn open_project(
    path: String,
    state: State<'_, ConfigState>,
) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("Path does not exist: {}", path));
    }
    if !p.is_dir() {
        return Err(format!("Path is not a directory: {}", path));
    }

    let canonical = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let canonical_str = canonical.to_string_lossy().to_string();

    let manager = state.manager.write().await;
    let mut settings = manager.get_settings().await;

    let canonical_lower = canonical_str.to_lowercase();
    settings.project_paths.retain(|pp| pp.to_lowercase() != canonical_lower);
    settings.project_paths.insert(0, canonical_str);
    settings.project_paths.truncate(10);

    manager.update_settings(settings).await?;
    Ok(())
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
    if !std::path::Path::new(&path).exists() {
        return Err(format!("File not found: {}", path));
    }
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn write_file(
    path: String,
    content: String,
) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, &content).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_file(
    path: String,
    content: Option<String>,
) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, content.unwrap_or_default()).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_directory(
    path: String,
) -> Result<(), String> {
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_file(
    path: String,
) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("Path not found: {}", path));
    }
    if p.is_dir() {
        std::fs::remove_dir_all(&path).map_err(|e| e.to_string())
    } else {
        std::fs::remove_file(&path).map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub async fn rename_file(
    source: String,
    destination: String,
) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(&destination).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&source, &destination).map_err(|e| e.to_string())
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
