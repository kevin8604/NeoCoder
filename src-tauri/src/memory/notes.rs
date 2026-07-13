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
}
