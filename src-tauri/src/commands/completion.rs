use tauri::{Emitter, State};
use crate::completion::{CompletionContext, CompletionEvent, build_fim_prompt, COMPLETION_SYSTEM_PROMPT, multi_file};
use crate::config::AppSettings;
use crate::llm::{self, FimRequest};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Store active cancellation flags for completions
pub type CancelMap = Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>;

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
        Self { entries: std::collections::HashMap::new(), order: Vec::with_capacity(max_size), max_size }
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
    let prefix_tail: String = ctx.prefix.chars().rev().take(80).collect::<Vec<_>>().into_iter().rev().collect();
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
}

#[tauri::command]
pub async fn request_completion(
    app: tauri::AppHandle,
    context: CompletionContext,
    settings: State<'_, Arc<RwLock<AppSettings>>>,
    cache: State<'_, Arc<std::sync::Mutex<CompletionCache>>>,
    cancel_map: State<'_, CancelMap>,
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
    let _guard = CancelGuard { map: cancel_map.inner().clone(), id: id.clone() };

    // Collect related file context (non-blocking, with timeout)
    let related_context = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        multi_file::collect_related_context(&context.file_path, None, 3, 5)
    )
    .await
    .ok();

    // Enrich context with related files
    let enriched_context = if let Some(related) = related_context {
        CompletionContext {
            related_context: Some(related),
            ..context
        }
    } else {
        context
    };

    // Check cache first
    let cache_key = build_cache_key(&enriched_context);
    {
        let cache_lock = cache.lock().map_err(|e| format!("Cache lock error: {}", e))?;
        if let Some(cached) = cache_lock.get(cache_key) {
            log::info!("Completion cache hit for key {}", cache_key);
            let processed = cached.clone();
            drop(cache_lock);
            let _ = app.emit("completion-event", CompletionEvent::Finished {
                id: id.clone(),
                full_text: processed.clone(),
            });
            return Ok(CompletionResponse { id, text: processed });
        }
    }

    // Build FIM prompt
    let provider = settings.llm_provider.clone();
    let fim_prompt = build_fim_prompt(&enriched_context, "openai");

    // Emit started event
    let _ = app.emit("completion-event", CompletionEvent::Started {
        id: id.clone(),
    });

    let app_clone = app.clone();
    let id_clone = id.clone();

    let request = FimRequest {
        model: settings.completion_model.clone(),
        prompt: fim_prompt,
        system_prompt: COMPLETION_SYSTEM_PROMPT.to_string(),
        max_tokens: 256,
    };

    let api_key = settings.api_key.clone();
    
    // Accumulate streamed tokens into full_text
    let full_text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let full_text_clone = full_text.clone();
    
    let result = llm::stream_fim(
        &provider,
        &api_key,
        None,
        request,
        |token| {
            // Accumulate into shared buffer
            if let Ok(mut buf) = full_text_clone.lock() {
                buf.push_str(&token);
            }
            let _ = app_clone.emit("completion-event", CompletionEvent::Delta {
                id: id_clone.clone(),
                token,
            });
            Ok(())
        },
        Some(cancel_flag),
    ).await;

    match result {
        Ok(()) => {
            let accumulated = full_text.lock().map(|s| s.clone()).unwrap_or_default();
            // Apply post-processing to the accumulated text
            let processed = crate::completion::post_process_completion(&accumulated, &enriched_context);

            // Store in cache if meaningful result
            if processed.len() > 1 {
                if let Ok(mut cache_lock) = cache.lock() {
                    cache_lock.insert(cache_key, processed.clone());
                }
            }

            let _ = app.emit("completion-event", CompletionEvent::Finished {
                id: id.clone(),
                full_text: processed.clone(),
            });
            Ok(CompletionResponse {
                id,
                text: processed,
            })
        }
        Err(e) => {
            let _ = app.emit("completion-event", CompletionEvent::Error {
                id: id.clone(),
                message: e.clone(),
            });
            Err(e)
        }
    }
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
        let _ = app.emit("completion-event", CompletionEvent::Cancelled {
            id: ids.first().cloned().unwrap_or_default(),
        });
    }
    Ok(())
}
