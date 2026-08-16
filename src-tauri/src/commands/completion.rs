use crate::completion::{
    COMPLETION_SYSTEM_PROMPT, CompletionContext, CompletionEvent, build_fim_prompt,
    edit_intent::EditIntentTracker, multi_file,
};
use crate::config::AppSettings;
use crate::llm::{self, FimRequest};
use crate::rag::CodeIndexer;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, State};
use tokio::sync::RwLock;
use uuid::Uuid;

/// Store active cancellation flags for completions
pub type CancelMap = Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>;

/// Store completion candidates: id -> (candidates list, current index)
pub type CompletionCandidates =
    Arc<std::sync::Mutex<std::collections::HashMap<String, (Vec<String>, usize)>>>;

/// Drop guard to auto-remove cancel flag on completion
struct CancelGuard {
    map: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>,
    id: String,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = self.map.lock() {
            map.remove(&self.id);
        }
    }
}

/// Simple LRU cache for completion results.
/// Key: hash of (file_extension, prefix_tail, suffix_head)
/// Value: completion text
pub struct CompletionCache {
    entries: std::collections::HashMap<u64, String>,
    order: Vec<u64>,
    max_size: usize,
}

impl CompletionCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            order: Vec::with_capacity(max_size),
            max_size,
        }
    }

    pub fn get(&self, key: u64) -> Option<&String> {
        self.entries.get(&key)
    }

    pub fn insert(&mut self, key: u64, value: String) {
        if self.entries.contains_key(&key) {
            return; // already present
        }
        while self.order.len() >= self.max_size {
            if let Some(oldest) = self.order.first().cloned() {
                self.entries.remove(&oldest);
                self.order.remove(0);
            } else {
                break;
            }
        }
        self.order.push(key);
        self.entries.insert(key, value);
    }
}

/// Build a cache key from the completion context
fn build_cache_key(ctx: &CompletionContext) -> u64 {
    let mut hasher = DefaultHasher::new();
    ctx.file_path.hash(&mut hasher);
    ctx.language.hash(&mut hasher);
    // Use last 80 chars of prefix as context fingerprint
    let prefix_tail: String = ctx
        .prefix
        .chars()
        .rev()
        .take(80)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    prefix_tail.hash(&mut hasher);
    // Use first 40 chars of suffix
    let suffix_head: String = ctx.suffix.chars().take(40).collect();
    suffix_head.hash(&mut hasher);
    hasher.finish()
}

#[derive(serde::Serialize)]
pub struct CompletionResponse {
    pub id: String,
    pub text: String,
    pub candidates_count: usize,
}

#[tauri::command]
pub async fn request_completion(
    app: tauri::AppHandle,
    context: CompletionContext,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
    cache: State<'_, Arc<std::sync::Mutex<CompletionCache>>>,
    cancel_map: State<'_, CancelMap>,
    candidates_state: State<'_, CompletionCandidates>,
    indexer: State<'_, Arc<CodeIndexer>>,
    edit_intent: State<'_, Arc<EditIntentTracker>>,
) -> Result<CompletionResponse, String> {
    let settings = settings.read().await;
    let id = Uuid::new_v4().to_string();

    // Register cancellation flag with auto-cleanup guard
    let cancel_flag = Arc::new(AtomicBool::new(false));
    {
        if let Ok(mut map) = cancel_map.lock() {
            map.insert(id.clone(), cancel_flag.clone());
        }
    }
    let _guard = CancelGuard {
        map: cancel_map.inner().clone(),
        id: id.clone(),
    };

    // Collect related file context (non-blocking, with timeout)
    // 1. Same-directory symbol scan (fast, no index needed)
    // 2. RAG cross-directory search driven by cursor identifiers (150ms budget)
    let indexer_clone = indexer.inner().clone();
    let prefix_for_rag = context.prefix.clone();
    let dir_context = multi_file::collect_related_context(&context.file_path, None, 3, 5);
    let rag_context = tokio::time::timeout(
        std::time::Duration::from_millis(150),
        multi_file::collect_rag_context(&prefix_for_rag, &indexer_clone, None, 3),
    )
    .await
    .ok();
    let related_context = tokio::time::timeout(std::time::Duration::from_millis(200), dir_context)
        .await
        .ok();

    // Merge: RAG results first (higher signal), then same-directory symbols
    let mut merged_files = Vec::new();
    if let Some(rag) = rag_context {
        for f in rag.files {
            if !merged_files
                .iter()
                .any(|x: &crate::completion::multi_file::RelatedFile| x.path == f.path)
            {
                merged_files.push(f);
            }
        }
    }
    if let Some(dir) = related_context {
        for f in dir.files {
            if !merged_files
                .iter()
                .any(|x: &crate::completion::multi_file::RelatedFile| x.path == f.path)
            {
                merged_files.push(f);
            }
        }
    }

    // Recent edits (edit-intent signal)
    let recent_edits = edit_intent.inner().recent(8);

    // Enrich context with related files + edit intent
    let enriched_context = CompletionContext {
        related_context: if merged_files.is_empty() {
            None
        } else {
            Some(crate::completion::multi_file::RelatedContext {
                files: merged_files,
            })
        },
        recent_edits,
        ..context
    };

    // Check cache first
    let cache_key = build_cache_key(&enriched_context);
    {
        let cache_lock = cache
            .lock()
            .map_err(|e| format!("Cache lock error: {}", e))?;
        if let Some(cached) = cache_lock.get(cache_key) {
            log::info!("Completion cache hit for key {}", cache_key);
            let processed = cached.clone();
            drop(cache_lock);
            let _ = app.emit(
                "completion-event",
                CompletionEvent::Finished {
                    id: id.clone(),
                    full_text: processed.clone(),
                },
            );
            return Ok(CompletionResponse {
                id,
                text: processed,
                candidates_count: 1,
            });
        }
    }

    // Build FIM prompt
    let provider = settings.llm_provider.clone();
    let fim_prompt = build_fim_prompt(&enriched_context, "openai");

    // Emit started event
    let _ = app.emit(
        "completion-event",
        CompletionEvent::Started { id: id.clone() },
    );

    let api_key = settings.api_key.clone();
    let model = settings.completion_model.clone();

    // Generate 3 candidates with different temperatures
    let temperatures: [f32; 3] = [0.0, 0.3, 0.6];
    let mut candidates: Vec<String> = Vec::new();

    for &temp in &temperatures {
        let app_clone = app.clone();
        let id_clone = id.clone();
        let cancel = cancel_flag.clone();

        let request = FimRequest {
            model: model.clone(),
            prompt: fim_prompt.clone(),
            system_prompt: COMPLETION_SYSTEM_PROMPT.to_string(),
            max_tokens: 256,
            temperature: temp,
        };

        let full_text = Arc::new(std::sync::Mutex::new(String::new()));
        let full_text_clone = full_text.clone();

        let result = llm::stream_fim(
            &provider,
            &api_key,
            None,
            request,
            |token| {
                if let Ok(mut buf) = full_text_clone.lock() {
                    buf.push_str(&token);
                }
                // Only stream tokens for the first candidate (temp=0.0)
                if temp == 0.0 {
                    let _ = app_clone.emit(
                        "completion-event",
                        CompletionEvent::Delta {
                            id: id_clone.clone(),
                            token,
                        },
                    );
                }
                Ok(())
            },
            Some(cancel),
        )
        .await;

        match result {
            Ok(()) => {
                let accumulated = full_text.lock().map(|s| s.clone()).unwrap_or_default();
                let processed =
                    crate::completion::post_process_completion(&accumulated, &enriched_context);
                if processed.len() > 1 && !candidates.contains(&processed) {
                    candidates.push(processed);
                }
            }
            Err(e) => {
                // If first candidate fails, abort entirely
                if candidates.is_empty() {
                    let _ = app.emit(
                        "completion-event",
                        CompletionEvent::Error {
                            id: id.clone(),
                            message: e.clone(),
                        },
                    );
                    return Err(e);
                }
                // Subsequent candidate failures are acceptable
                log::debug!("Candidate generation failed (temp={}): {}", temp, e);
            }
        }
    }

    // Fallback: if no candidates generated
    if candidates.is_empty() {
        let _ = app.emit(
            "completion-event",
            CompletionEvent::Error {
                id: id.clone(),
                message: "No completion candidates generated".to_string(),
            },
        );
        return Err("No completion candidates generated".to_string());
    }

    let primary = candidates[0].clone();
    let count = candidates.len();

    // Store candidates for cycling
    if let Ok(mut map) = candidates_state.lock() {
        map.insert(id.clone(), (candidates, 0));
    }

    // Store primary in cache
    if primary.len() > 1
        && let Ok(mut cache_lock) = cache.lock()
    {
        cache_lock.insert(cache_key, primary.clone());
    }

    let _ = app.emit(
        "completion-event",
        CompletionEvent::Finished {
            id: id.clone(),
            full_text: primary.clone(),
        },
    );

    Ok(CompletionResponse {
        id,
        text: primary,
        candidates_count: count,
    })
}

/// Cycle to the next/previous completion candidate.
#[tauri::command]
pub async fn cycle_completion(
    app: tauri::AppHandle,
    id: String,
    direction: i32,
    candidates_state: State<'_, CompletionCandidates>,
) -> Result<String, String> {
    let mut map = candidates_state
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;
    let entry = map
        .get_mut(&id)
        .ok_or_else(|| "No candidates for this completion id".to_string())?;
    let (ref candidates, ref mut index) = *entry;
    if candidates.is_empty() {
        return Err("No candidates available".to_string());
    }
    let len = candidates.len();
    let new_index = if direction >= 0 {
        (*index + 1) % len
    } else {
        if *index == 0 { len - 1 } else { *index - 1 }
    };
    *index = new_index;
    let text = candidates[new_index].clone();
    drop(map);

    let _ = app.emit(
        "completion-event",
        CompletionEvent::Finished {
            id: id.clone(),
            full_text: text.clone(),
        },
    );
    Ok(text)
}

#[tauri::command]
pub async fn cancel_completion(
    app: tauri::AppHandle,
    cancel_map: State<'_, CancelMap>,
) -> Result<(), String> {
    if let Ok(map) = cancel_map.lock() {
        let ids: Vec<String> = map.keys().cloned().collect();
        for id in &ids {
            if let Some(flag) = map.get(id) {
                flag.store(true, Ordering::SeqCst);
            }
        }
        let _ = app.emit(
            "completion-event",
            CompletionEvent::Cancelled {
                id: ids.first().cloned().unwrap_or_default(),
            },
        );
    }
    Ok(())
}
