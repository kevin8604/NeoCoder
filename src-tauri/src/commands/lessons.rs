//! Tauri commands for the cross-session failure-lessons store.
//!
//! The capture (`FailureMemoryHook::post_tool`) and injection
//! (`build_system_prompt` → `format_for_prompt`) sides of the loop already exist;
//! these commands supply the management side the store never had: let the user
//! see what the agent has learned, dismiss a false-positive, clear a project, and
//! prune stale patterns. Reads/writes the same `failure_lessons.json` the agent
//! uses, resolved via the app config dir.

use crate::agent::failure_lessons::{FailureLesson, FailureLessonsStore};
use tauri::Manager;

/// Lessons not seen in this many days are considered stale by `prune`/`get`.
const DEFAULT_STALE_DAYS: i64 = 90;

fn lessons_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|d| d.join("failure_lessons.json"))
        .map_err(|e| e.to_string())
}

/// List every learned lesson, most recently seen first.
#[tauri::command]
pub async fn get_failure_lessons(app: tauri::AppHandle) -> Result<Vec<FailureLesson>, String> {
    let store = FailureLessonsStore::load(lessons_path(&app)?);
    let mut lessons = store.all().to_vec();
    lessons.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
    Ok(lessons)
}

/// Dismiss a single lesson by (project, signature).
#[tauri::command]
pub async fn delete_failure_lesson(
    app: tauri::AppHandle,
    project: String,
    signature: String,
) -> Result<bool, String> {
    let mut store = FailureLessonsStore::load(lessons_path(&app)?);
    Ok(store.remove(&project, &signature))
}

/// Clear lessons: a single project when `project` is given, otherwise all.
/// Returns how many lessons were removed.
#[tauri::command]
pub async fn clear_failure_lessons(
    app: tauri::AppHandle,
    project: Option<String>,
) -> Result<usize, String> {
    let mut store = FailureLessonsStore::load(lessons_path(&app)?);
    Ok(match project {
        Some(p) => store.remove_project(&p),
        None => store.clear(),
    })
}

/// Drop lessons whose `last_seen` is older than `days` (default 90).
/// Returns how many were removed.
#[tauri::command]
pub async fn prune_failure_lessons(
    app: tauri::AppHandle,
    days: Option<i64>,
) -> Result<usize, String> {
    let mut store = FailureLessonsStore::load(lessons_path(&app)?);
    Ok(store.prune_older_than(days.unwrap_or(DEFAULT_STALE_DAYS)))
}
