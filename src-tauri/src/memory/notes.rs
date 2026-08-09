use std::path::Path;
use std::fs;

/// Daily notes management: reads/writes `notes/YYYY-MM-DD.md` files.
pub struct DailyNotes {
    notes_dir: std::path::PathBuf,
}

impl DailyNotes {
    pub fn new(base_dir: &Path) -> Self {
        Self {
            notes_dir: base_dir.join("notes"),
        }
    }

    /// Read today's note file.
    pub fn read_today(&self) -> Result<String, String> {
        let path = self.date_path(&chrono::Utc::now().format("%Y-%m-%d").to_string());
        Self::read_file(&path)
    }

    /// Read yesterday's note file.
    pub fn read_yesterday(&self) -> Result<String, String> {
        let yesterday = chrono::Utc::now() - chrono::Duration::days(1);
        let path = self.date_path(&yesterday.format("%Y-%m-%d").to_string());
        Self::read_file(&path)
    }

    /// List all note dates, newest first: (date, char_len, first_line).
    pub fn list_notes(&self) -> Result<Vec<(String, usize, String)>, String> {
        if !self.notes_dir.exists() {
            return Ok(Vec::new());
        }
        let mut notes = Vec::new();
        for entry in fs::read_dir(&self.notes_dir)
            .map_err(|e| format!("Failed to read notes dir: {}", e))?
            .flatten()
        {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // Only date-named files (YYYY-MM-DD) are shown in the calendar
            if chrono::NaiveDate::parse_from_str(stem, "%Y-%m-%d").is_err() {
                continue;
            }
            let content = Self::read_file(&path).unwrap_or_default();
            let first_line = content
                .lines()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.chars().take(80).collect())
                .unwrap_or_else(|| "(empty)".to_string());
            notes.push((stem.to_string(), content.chars().count(), first_line));
        }
        notes.sort_by(|a, b| b.0.cmp(&a.0));
        Ok(notes)
    }

    /// Read the note for a specific date (YYYY-MM-DD). Empty string if absent.
    pub fn read_note(&self, date: &str) -> Result<String, String> {
        // Validate the date format to avoid path traversal
        if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
            return Err(format!("Invalid date format: {} (expected YYYY-MM-DD)", date));
        }
        let path = self.date_path(date);
        Self::read_file(&path)
    }

    /// Read note for a specific date.
    fn read_file(path: &std::path::PathBuf) -> Result<String, String> {
        if !path.exists() {
            return Ok(String::new());
        }
        fs::read_to_string(path)
            .map_err(|e| format!("Failed to read note file: {}", e))
    }

    /// Append an entry to today's note file (creates if not exists).
    pub fn append(&self, entry: &str) -> Result<(), String> {
        let date_str = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let path = self.date_path(&date_str);

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create notes dir: {}", e))?;
        }

        let current = Self::read_file(&path)?;
        let mut new_content = current;
        if !new_content.is_empty() && !new_content.ends_with('\n') {
            new_content.push('\n');
        }
        let timestamp = chrono::Utc::now().format("%H:%M:%S").to_string();
        new_content.push_str(&format!("- [{}] {}\n", timestamp, entry));

        fs::write(&path, &new_content)
            .map_err(|e| format!("Failed to write note file: {}", e))
    }

    fn date_path(&self, date: &str) -> std::path::PathBuf {
        self.notes_dir.join(format!("{}.md", date))
    }

    /// Delete note files older than `max_age_days` days. Returns the number deleted.
    /// A note is considered expired when its filename date (YYYY-MM-DD) is older
    /// than the retention window. Today's note is never deleted.
    pub fn cleanup_expired(&self, max_age_days: u32) -> Result<usize, String> {
        if !self.notes_dir.exists() {
            return Ok(0);
        }
        let cutoff = chrono::Utc::now().date_naive() - chrono::Days::new(max_age_days as u64);
        let mut deleted = 0usize;

        for entry in fs::read_dir(&self.notes_dir)
            .map_err(|e| format!("Failed to read notes dir: {}", e))?
            .flatten()
        {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // Filename is YYYY-MM-DD; non-date files (e.g. custom notes) are kept
            let Ok(date) = chrono::NaiveDate::parse_from_str(stem, "%Y-%m-%d") else {
                continue;
            };
            if date < cutoff {
                match fs::remove_file(&path) {
                    Ok(_) => deleted += 1,
                    Err(e) => log::warn!("[NotesGC] Failed to delete {}: {}", path.display(), e),
                }
            }
        }
        Ok(deleted)
    }
}
