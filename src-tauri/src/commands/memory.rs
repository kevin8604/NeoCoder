//! Memory & local-model commands exposed to the frontend.
//!
//! Covers the "local model integration" and "memory enhancement" phases:
//! - check_local_model: Ollama health probe (status indicator)
//! - search_memory: BM25 / hybrid (BM25 + embedding) memory search
//! - preview_memory: MEMORY.md preview + stats (MemoryPanel)
//! - list_notes / read_note: Daily Notes calendar browsing (MemoryPanel)
//! - get_memory_stats: memory panel statistics
//! - cleanup_memory: memory GC (capacity + expiration) → memory-updated event
//! - run_deep_dreaming: consolidation & deduplication → dreaming-progress events
//! - export_training_data: MEMORY.md → JSONL fine-tune dataset → finetune-progress event
//! - get_memory_entries: entries + current Ebbinghaus retention (R) for visualization

use crate::commands::chat::ChatState;
use crate::config::AppSettings;
use crate::llm::health::LocalModelHealth;
use crate::memory::search::MemSearchResult;
use std::sync::Arc;
use tauri::{Emitter, State};
use tokio::sync::RwLock;

/// Probe the local Ollama service: running state + available models.
#[tauri::command]
pub async fn check_local_model(
    settings: State<'_, Arc<RwLock<AppSettings>>>,
) -> Result<LocalModelHealth, String> {
    let settings = settings.read().await;
    let base_url = settings.local_model.base_url.clone();
    drop(settings);
    Ok(crate::llm::health::check_ollama(&base_url).await)
}

/// Search the memory corpus. When `use_semantic` is enabled and the memory GC
/// config allows it, runs a hybrid BM25 + embedding search (local Ollama
/// embedding first, automatic fallback to BM25 on any embedding failure).
#[tauri::command]
pub async fn search_memory(
    chat_state: State<'_, ChatState>,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
    query: String,
    max_results: Option<usize>,
    use_semantic: Option<bool>,
) -> Result<Vec<MemSearchResult>, String> {
    let limit = max_results.unwrap_or(8).clamp(1, 30);
    let memory = chat_state.memory.read().await;
    let mgr = memory.memory_manager();
    let settings = settings.read().await;

    let want_semantic = use_semantic.unwrap_or(settings.memory_gc.semantic_search);
    if want_semantic {
        let results = mgr.hybrid_search_memory(&query, &settings, limit).await?;
        log::info!(
            "[MemorySearch] Hybrid search '{}' -> {} results",
            query,
            results.len()
        );
        Ok(results)
    } else {
        let results = mgr.search_memory(&query, limit)?;
        log::info!(
            "[MemorySearch] BM25 search '{}' -> {} results",
            query,
            results.len()
        );
        Ok(results)
    }
}

/// Full MEMORY.md preview + stats for the MemoryPanel.
#[tauri::command]
pub async fn preview_memory(
    app: tauri::AppHandle,
    chat_state: State<'_, ChatState>,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
) -> Result<serde_json::Value, String> {
    let memory = chat_state.memory.read().await;
    let mgr = memory.memory_manager();
    let settings = settings.read().await;
    let long_term = mgr.read_long_term().unwrap_or_default();
    let stats = mgr.get_memory_stats(&settings)?;
    let _ = app.emit("memory-view-opened", ());
    Ok(serde_json::json!({
        "long_term": long_term,
        "stats": stats,
    }))
}

/// List daily note dates, newest first: [{date, chars, preview}].
#[tauri::command]
pub async fn list_notes(
    chat_state: State<'_, ChatState>,
) -> Result<Vec<serde_json::Value>, String> {
    let memory = chat_state.memory.read().await;
    let mgr = memory.memory_manager();
    Ok(mgr
        .list_notes()?
        .into_iter()
        .map(|(date, chars, preview)| serde_json::json!({ "date": date, "chars": chars, "preview": preview }))
        .collect())
}

/// Read a daily note for a specific date (YYYY-MM-DD).
#[tauri::command]
pub async fn read_note(chat_state: State<'_, ChatState>, date: String) -> Result<String, String> {
    let memory = chat_state.memory.read().await;
    let mgr = memory.memory_manager();
    mgr.read_note(&date)
}

/// Aggregate memory statistics for the memory panel.
#[tauri::command]
pub async fn get_memory_stats(
    chat_state: State<'_, ChatState>,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
) -> Result<serde_json::Value, String> {
    let memory = chat_state.memory.read().await;
    let mgr = memory.memory_manager();
    let settings = settings.read().await;
    mgr.get_memory_stats(&settings)
}

/// Read memory entries with their current Ebbinghaus retention (R) values
/// for the visualization panel (R-value distribution + decay curves).
///
/// Returns entries sorted by R ascending (most forgotten first), each with
/// id, truncated text, section, category tag, stability, recall count, dates
/// and the computed retention in [0, 1].
#[tauri::command]
pub async fn get_memory_entries(
    chat_state: State<'_, ChatState>,
    limit: Option<usize>,
) -> Result<Vec<serde_json::Value>, String> {
    let memory = chat_state.memory.read().await;
    let mgr = memory.memory_manager();
    let now = chrono::Utc::now().date_naive();

    let mut entries: Vec<serde_json::Value> = mgr
        .long_term
        .read_entries()?
        .into_iter()
        .map(|e| {
            let retention = crate::memory::ebbinghaus::compute_retention(&e, now);
            serde_json::json!({
                "id": e.id,
                "text": e.text.chars().take(120).collect::<String>(),
                "section": e.section,
                "category": e.category.to_tag(),
                "stability": (e.stability * 100.0).round() / 100.0,
                "recall_count": e.recall_count,
                "created": e.created.to_string(),
                "last_recalled": e.last_recalled.to_string(),
                "retention": (retention * 100.0).round() / 100.0,
            })
        })
        .collect();

    // Most forgotten first
    entries.sort_by(|a, b| {
        a["retention"]
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&b["retention"].as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if let Some(limit) = limit
        && entries.len() > limit
    {
        entries.truncate(limit);
    }
    Ok(entries)
}

/// Run memory GC: capacity enforcement, expired entries, old notes & sessions.
/// Emits `memory-updated` when done so the UI can refresh.
#[tauri::command]
pub async fn cleanup_memory(
    app: tauri::AppHandle,
    chat_state: State<'_, ChatState>,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
) -> Result<serde_json::Value, String> {
    let memory = chat_state.memory.read().await;
    let mgr = memory.memory_manager();
    let settings = settings.read().await;
    let report = mgr.run_gc(&settings)?;
    let _ = app.emit(
        "memory-updated",
        serde_json::json!({ "source": "gc", "report": report }),
    );
    Ok(report)
}

/// Deep Dreaming: global consolidation — merge duplicates, drop stale entries.
/// Emits `dreaming-progress` (start/done) and `memory-updated` events.
#[tauri::command]
pub async fn run_deep_dreaming(
    app: tauri::AppHandle,
    chat_state: State<'_, ChatState>,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
) -> Result<String, String> {
    let _ = app.emit(
        "dreaming-progress",
        serde_json::json!({ "phase": "start", "progress": 0.05, "message": "Consolidating long-term memory…" }),
    );
    let memory = chat_state.memory.read().await;
    let mgr = memory.memory_manager();
    let settings = settings.read().await;
    let report = mgr.deep_dreaming(&settings).await;
    match &report {
        Ok(text) => {
            let _ = app.emit(
                "dreaming-progress",
                serde_json::json!({ "phase": "done", "progress": 1.0, "message": "Deep dreaming finished" }),
            );
            let _ = app.emit(
                "memory-updated",
                serde_json::json!({ "source": "dreaming", "report": text }),
            );
        }
        Err(e) => {
            let _ = app.emit(
                "dreaming-progress",
                serde_json::json!({ "phase": "error", "progress": 1.0, "message": e }),
            );
        }
    }
    report
}

/// Export MEMORY.md as a JSONL dataset for LoRA fine-tuning.
/// Emits `finetune-progress` + `memory-updated` events.
#[tauri::command]
pub async fn export_training_data(
    app: tauri::AppHandle,
    chat_state: State<'_, ChatState>,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
) -> Result<String, String> {
    let _ = app.emit(
        "finetune-progress",
        serde_json::json!({ "phase": "start", "progress": 0.1, "message": "Exporting training dataset…" }),
    );
    let memory = chat_state.memory.read().await;
    let mgr = memory.memory_manager();
    let settings = settings.read().await;
    let result = mgr.export_training_data(&settings);
    match &result {
        Ok(path) => {
            let _ = app.emit(
                "finetune-progress",
                serde_json::json!({ "phase": "exported", "progress": 1.0, "message": path }),
            );
            let _ = app.emit(
                "memory-updated",
                serde_json::json!({ "source": "finetune", "path": path }),
            );
        }
        Err(e) => {
            let _ = app.emit(
                "finetune-progress",
                serde_json::json!({ "phase": "error", "progress": 1.0, "message": e }),
            );
        }
    }
    result
}
