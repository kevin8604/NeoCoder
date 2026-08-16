use super::ebbinghaus::{self, MemoryCategory, MemoryEntry};
use std::fs;
use std::path::Path;

/// Long-term memory store: reads/writes `MEMORY.md` in the memory base directory.
/// Supports Ebbinghaus forgetting curve metadata for individual entries.
pub struct LongTermMemory {
    file_path: std::path::PathBuf,
}

impl LongTermMemory {
    pub fn new(base_dir: &Path) -> Self {
        Self {
            file_path: base_dir.join("MEMORY.md"),
        }
    }

    /// Read the full MEMORY.md content. Returns empty string if file doesn't exist.
    pub fn read(&self) -> Result<String, String> {
        if !self.file_path.exists() {
            return Ok(String::new());
        }
        fs::read_to_string(&self.file_path).map_err(|e| format!("Failed to read MEMORY.md: {}", e))
    }

    /// Overwrite MEMORY.md with new content.
    pub fn write(&self, content: &str) -> Result<(), String> {
        if let Some(parent) = self.file_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create memory dir: {}", e))?;
        }
        fs::write(&self.file_path, content).map_err(|e| format!("Failed to write MEMORY.md: {}", e))
    }

    /// Append a section heading + entry to MEMORY.md.
    /// New entries automatically get Ebbinghaus metadata.
    /// Dual-channel dedup: Jaccard > 0.6 OR topic_similarity > 0.5 triggers merge.
    pub fn append(&self, section: &str, entry: &str) -> Result<(), String> {
        // Check for similar existing entries (dual-channel dedup)
        if let Ok(existing) = self.read_entries() {
            let mut similar_idx: Option<usize> = None;
            let mut best_sim = 0.0;
            for (i, e) in existing.iter().enumerate() {
                // Channel 1: Jaccard word overlap (lowered threshold from 0.7 to 0.6)
                let jaccard = ebbinghaus::compute_similarity(&e.text, entry);
                // Channel 2: Topic-level keyword overlap
                let topic = ebbinghaus::compute_topic_similarity(&e.text, entry);
                // Merge if EITHER channel triggers
                let combined = jaccard.max(topic * 0.9); // topic gets slight discount
                if (jaccard > 0.6 || topic > 0.5) && combined > best_sim {
                    best_sim = combined;
                    similar_idx = Some(i);
                }
            }
            if let Some(idx) = similar_idx {
                // Merge: keep higher stability, sum recall counts, use newer text
                let mut entries = existing;
                let old = &mut entries[idx];
                old.stability = old.stability.max(1.0);
                old.recall_count += 1;
                // Prefer newer (longer/more detailed) text
                if entry.len() > old.text.len() {
                    old.text = entry.to_string();
                }
                old.last_recalled = chrono::Utc::now().date_naive();
                log::debug!(
                    "[Memory] Merged similar entry (sim={:.2}): '{}'",
                    best_sim,
                    &old.text[..old.text.len().min(60)]
                );
                return self.write_entries(&entries);
            }
        }

        let current = self.read()?;
        let mut new_content = current;
        if !new_content.is_empty() && !new_content.ends_with('\n') {
            new_content.push('\n');
        }
        // Add entry with Ebbinghaus metadata — detect category from text tags
        let category = MemoryCategory::detect_from_text(entry);
        // 条目行必须带 "- " 前缀，否则 parse_memory_entries 无法识别（与
        // serialize_memory_entries 的输出格式保持一致）
        let bullet = if entry.starts_with("- ") {
            entry.to_string()
        } else {
            format!("- {}", entry)
        };
        let mem_entry = MemoryEntry::with_category(bullet.clone(), section.to_string(), category);
        new_content.push_str(&format!(
            "\n## {}\n\n{}\n{}\n",
            section,
            bullet,
            ebbinghaus::format_metadata(&mem_entry)
        ));
        self.write(&new_content)
    }

    // ── Ebbinghaus-aware entry operations ──

    /// Parse all entries from MEMORY.md with Ebbinghaus metadata.
    /// Entries without metadata get default values (backward compatible).
    pub fn read_entries(&self) -> Result<Vec<MemoryEntry>, String> {
        let content = self.read()?;
        Ok(ebbinghaus::parse_memory_entries(&content))
    }

    /// Write entries back to MEMORY.md, preserving Ebbinghaus metadata.
    pub fn write_entries(&self, entries: &[MemoryEntry]) -> Result<(), String> {
        let content = ebbinghaus::serialize_memory_entries(entries);
        self.write(&content)
    }

    /// Recall specific entries by their indices (0-based).
    /// Updates their recall_count, last_recalled, and stability.
    pub fn recall_entries(&self, indices: &[usize]) -> Result<(), String> {
        let mut entries = self.read_entries()?;
        for &idx in indices {
            if idx < entries.len() {
                ebbinghaus::update_recall(&mut entries[idx]);
            }
        }
        self.write_entries(&entries)
    }

    /// Archive (remove) entries with low retention scores.
    /// Returns the number of archived entries.
    pub fn cleanup_expired(&self) -> Result<usize, String> {
        let entries = self.read_entries()?;
        let now = chrono::Utc::now().date_naive();
        let mut retained = Vec::new();
        let mut archived_count = 0;

        for entry in &entries {
            if ebbinghaus::should_archive(entry, now) {
                archived_count += 1;
                log::debug!(
                    "[Memory] Archiving expired entry: {}",
                    &entry.text[..entry.text.len().min(60)]
                );
            } else {
                retained.push(entry.clone());
            }
        }

        if archived_count > 0 {
            self.write_entries(&retained)?;
            log::info!("[Memory] Archived {} expired entries", archived_count);
        }

        Ok(archived_count)
    }

    /// Enforce maximum entry count by evicting lowest-retention entries.
    /// Core entries are never evicted. Returns number of evicted entries.
    pub fn enforce_capacity(&self, max_entries: usize) -> Result<usize, String> {
        let mut entries = self.read_entries()?;
        if entries.len() <= max_entries {
            return Ok(0);
        }

        let now = chrono::Utc::now().date_naive();

        // Compute retention for each entry, sort ascending (lowest first)
        let mut indexed: Vec<(usize, f64)> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let retention = if e.category.is_evergreen() {
                    f64::MAX // Core entries never evicted
                } else {
                    ebbinghaus::compute_retention(e, now)
                };
                (i, retention)
            })
            .collect();

        // Sort by retention ascending (lowest retention first = evict candidates)
        indexed.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let to_evict = entries.len() - max_entries;
        let mut evict_indices: Vec<usize> =
            indexed.iter().take(to_evict).map(|(i, _)| *i).collect();
        evict_indices.sort_unstable(); // sort for stable removal
        evict_indices.reverse(); // reverse for safe removal by index

        for idx in evict_indices {
            log::debug!(
                "[Memory] Evicting low-retention entry: {}",
                &entries[idx].text[..entries[idx].text.len().min(60)]
            );
            entries.remove(idx);
        }

        self.write_entries(&entries)?;
        log::info!(
            "[Memory] Evicted {} entries to enforce capacity limit of {}",
            to_evict,
            max_entries
        );
        Ok(to_evict)
    }
}
