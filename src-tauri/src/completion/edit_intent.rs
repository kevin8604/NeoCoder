//! Edit-intent tracking for code completion.
//!
//! Records recently modified files so the FIM prompt can signal "what the
//! user is currently working on" — a lightweight, local analogue of Cursor's
//! edit-history prediction. Only paths + timestamps are stored (no content),
//! keeping it privacy-friendly and memory-cheap.

use std::collections::VecDeque;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum number of recorded edit entries.
const MAX_ENTRIES: usize = 20;

/// A recorded edit: which file changed and when (unix seconds).
#[derive(Debug, Clone)]
pub struct EditRecord {
    pub path: String,
    pub timestamp: u64,
}

/// Thread-safe tracker of recently edited files.
pub struct EditIntentTracker {
    entries: RwLock<VecDeque<EditRecord>>,
}

impl EditIntentTracker {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(VecDeque::new()),
        }
    }

    /// Record a file modification. Deduplicates by path — re-editing an
    /// existing entry refreshes its timestamp and moves it to the front.
    pub fn record_edit(&self, path: &str) {
        if path.trim().is_empty() {
            return;
        }
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Ok(mut entries) = self.entries.write() {
            entries.retain(|e| e.path != path);
            entries.push_front(EditRecord {
                path: path.to_string(),
                timestamp: ts,
            });
            while entries.len() > MAX_ENTRIES {
                entries.pop_back();
            }
        }
    }

    /// Recently edited file paths, newest first (up to `max`).
    pub fn recent(&self, max: usize) -> Vec<String> {
        if let Ok(entries) = self.entries.read() {
            entries.iter().take(max).map(|e| e.path.clone()).collect()
        } else {
            Vec::new()
        }
    }

    /// Number of tracked entries (for diagnostics).
    pub fn len(&self) -> usize {
        self.entries.read().map(|e| e.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for EditIntentTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_recent() {
        let tracker = EditIntentTracker::new();
        tracker.record_edit("src/main.rs");
        tracker.record_edit("src/lib.rs");

        let recent = tracker.recent(10);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0], "src/lib.rs"); // newest first
    }

    #[test]
    fn test_dedup_refreshes_position() {
        let tracker = EditIntentTracker::new();
        tracker.record_edit("a.rs");
        tracker.record_edit("b.rs");
        tracker.record_edit("a.rs"); // re-edit moves to front

        let recent = tracker.recent(10);
        assert_eq!(recent, vec!["a.rs", "b.rs"]);
    }

    #[test]
    fn test_capacity_limit() {
        let tracker = EditIntentTracker::new();
        for i in 0..30 {
            tracker.record_edit(&format!("file_{}.rs", i));
        }
        assert_eq!(tracker.len(), MAX_ENTRIES);
        let recent = tracker.recent(100);
        assert_eq!(recent.len(), MAX_ENTRIES);
        assert_eq!(recent[0], "file_29.rs"); // newest kept
        assert!(!recent.contains(&"file_0.rs".to_string())); // oldest evicted
    }

    #[test]
    fn test_empty_path_ignored() {
        let tracker = EditIntentTracker::new();
        tracker.record_edit("");
        tracker.record_edit("   ");
        assert!(tracker.is_empty());
    }
}
