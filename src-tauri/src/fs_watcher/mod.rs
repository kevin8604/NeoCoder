use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ── Public Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FileChangeEvent {
    pub path: PathBuf,
    pub kind: FileChangeKind,
}

#[derive(Debug, Clone)]
pub enum FileChangeKind {
    Created,
    Modified,
    Deleted,
}

/// Callback type for file change events
type ChangeCallback = Arc<dyn Fn(FileChangeEvent) + Send + Sync + 'static>;

// ── Debounce Helper ─────────────────────────────────────────────────────────

struct DebouncedEvent {
    path: PathBuf,
    kind: FileChangeKind,
    timestamp: Instant,
}

impl DebouncedEvent {
    fn should_send(&self, debounce_ms: u64) -> bool {
        self.timestamp.elapsed() >= Duration::from_millis(debounce_ms)
    }
}

// ── File Watcher ────────────────────────────────────────────────────────────

pub struct FileWatcher {
    watcher: Option<RecommendedWatcher>,
    watched_paths: Arc<Mutex<HashMap<PathBuf, bool>>>, // path -> is_recursive
    /// Internal channel to receive notify events
    rx: Option<mpsc::Receiver<Result<Event, notify::Error>>>,
    /// Registered external callbacks
    callbacks: Arc<Mutex<Vec<ChangeCallback>>>,
    /// Debounce tracking
    pending_events: Arc<Mutex<Vec<DebouncedEvent>>>,
}

impl Default for FileWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl FileWatcher {
    pub fn new() -> Self {
        FileWatcher {
            watcher: None,
            watched_paths: Arc::new(Mutex::new(HashMap::new())),
            rx: None,
            callbacks: Arc::new(Mutex::new(Vec::new())),
            pending_events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start watching a path. Returns a receiver for file change events.
    pub fn start_watch(&mut self, path: &Path, recursive: bool) -> Result<(), String> {
        // Initialize watcher if needed
        if self.watcher.is_none() {
            let (tx, rx) = mpsc::channel::<Result<Event, notify::Error>>();
            let watcher = RecommendedWatcher::new(tx, Config::default())
                .map_err(|e| format!("Failed to create file watcher: {}", e))?;
            self.watcher = Some(watcher);
            self.rx = Some(rx);
        }

        let watcher = match self.watcher.as_mut() {
            Some(w) => w,
            None => return Err("File watcher not initialized".to_string()),
        };
        let mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };

        watcher
            .watch(path, mode)
            .map_err(|e| format!("Failed to watch path '{}': {}", path.display(), e))?;

        {
            let mut paths = self.watched_paths.lock().unwrap_or_else(|e| e.into_inner());
            paths.insert(path.to_path_buf(), recursive);
        }

        log::info!(
            "Watching path: {} (recursive: {})",
            path.display(),
            recursive
        );
        Ok(())
    }

    /// Stop watching a path.
    pub fn stop_watch(&mut self, path: &Path) {
        if let Some(watcher) = self.watcher.as_mut() {
            let _ = watcher.unwatch(path);
        }
        let mut paths = self.watched_paths.lock().unwrap_or_else(|e| e.into_inner());
        paths.remove(path);
        log::info!("Stopped watching: {}", path.display());
    }

    /// Register a callback for file change events.
    pub fn on_change<F>(&self, callback: F)
    where
        F: Fn(FileChangeEvent) + Send + Sync + 'static,
    {
        let mut cbs = self.callbacks.lock().unwrap_or_else(|e| e.into_inner());
        cbs.push(Arc::new(callback));
    }

    /// Poll for file change events. Should be called periodically (e.g., in a loop).
    pub fn poll_events(&self, debounce_ms: u64) -> Vec<FileChangeEvent> {
        let mut events = Vec::new();

        // Collect new events from notify
        if let Some(rx) = &self.rx {
            while let Ok(Ok(event)) = rx.try_recv() {
                for path in &event.paths {
                    let kind = match &event.kind {
                        EventKind::Create(_) => FileChangeKind::Created,
                        EventKind::Modify(_) => FileChangeKind::Modified,
                        EventKind::Remove(_) => FileChangeKind::Deleted,
                        _ => continue,
                    };

                    let mut pending = self
                        .pending_events
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());

                    // Check if we already have a pending event for this path
                    if let Some(existing) = pending.iter_mut().find(|e| e.path == *path) {
                        // Update: keep the latest kind and reset timer
                        existing.kind = kind;
                        existing.timestamp = Instant::now();
                    } else {
                        pending.push(DebouncedEvent {
                            path: path.clone(),
                            kind,
                            timestamp: Instant::now(),
                        });
                    }
                }
            }
        }

        // Process debounced events
        let mut ready_indices = Vec::new();
        {
            let pending = self
                .pending_events
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for (i, evt) in pending.iter().enumerate() {
                if evt.should_send(debounce_ms) {
                    ready_indices.push(i);
                }
            }
        }

        if !ready_indices.is_empty() {
            let mut pending = self
                .pending_events
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for &i in ready_indices.iter().rev() {
                if i < pending.len() {
                    let evt = pending.remove(i);
                    let change = FileChangeEvent {
                        path: evt.path,
                        kind: evt.kind,
                    };
                    // Fire callbacks
                    let cbs = self.callbacks.lock().unwrap_or_else(|e| e.into_inner());
                    for cb in cbs.iter() {
                        cb(change.clone());
                    }
                    events.push(change);
                }
            }
        }

        events
    }

    /// Get the list of currently watched paths.
    pub fn get_watched_paths(&self) -> Vec<PathBuf> {
        let paths = self.watched_paths.lock().unwrap_or_else(|e| e.into_inner());
        paths.keys().cloned().collect()
    }

    /// Stop all watches.
    pub fn stop_all(&mut self) {
        self.watcher = None; // Drops the watcher, which stops all watches
        let mut paths = self.watched_paths.lock().unwrap_or_else(|e| e.into_inner());
        paths.clear();
        log::info!("All file watches stopped");
    }
}

impl Drop for FileWatcher {
    fn drop(&mut self) {
        self.stop_all();
    }
}
