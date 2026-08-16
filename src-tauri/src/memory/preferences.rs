use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Tracks user editing patterns and preferences for context injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPreferences {
    /// Tool usage statistics: tool_name -> (total_calls, success_count)
    pub tool_stats: HashMap<String, ToolStats>,
    /// File type distribution: extension -> count
    pub file_type_counts: HashMap<String, u32>,
    /// Preferred programming languages (inferred from file extensions)
    pub preferred_languages: Vec<String>,
    /// Common task patterns extracted from agent interactions
    pub task_patterns: Vec<TaskPattern>,
    /// Last updated timestamp
    pub last_updated: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStats {
    pub total_calls: u32,
    pub success_count: u32,
    pub avg_duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPattern {
    pub description: String,
    pub frequency: u32,
    pub last_seen: String,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            tool_stats: HashMap::new(),
            file_type_counts: HashMap::new(),
            preferred_languages: Vec::new(),
            task_patterns: Vec::new(),
            last_updated: chrono::Utc::now().to_rfc3339(),
        }
    }
}

impl UserPreferences {
    /// Load preferences from disk, or create default if not found.
    pub fn load(base_dir: &Path) -> Self {
        let path = Self::prefs_path(base_dir);
        if !path.exists() {
            return Self::default();
        }
        let content = fs::read_to_string(&path).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    }

    /// Save preferences to disk.
    pub fn save(&self, base_dir: &Path) -> Result<(), String> {
        let path = Self::prefs_path(base_dir);
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize preferences: {}", e))?;
        fs::write(&path, json).map_err(|e| format!("Failed to write preferences: {}", e))
    }

    fn prefs_path(base_dir: &Path) -> PathBuf {
        base_dir.join("user_preferences.json")
    }

    /// Record a tool usage event.
    pub fn record_tool_usage(&mut self, tool_name: &str, success: bool, duration_ms: u64) {
        let stats = self
            .tool_stats
            .entry(tool_name.to_string())
            .or_insert(ToolStats {
                total_calls: 0,
                success_count: 0,
                avg_duration_ms: 0,
            });
        stats.total_calls += 1;
        if success {
            stats.success_count += 1;
        }
        // Rolling average
        stats.avg_duration_ms = (stats.avg_duration_ms * (stats.total_calls as u64 - 1)
            + duration_ms)
            / stats.total_calls as u64;
        self.last_updated = chrono::Utc::now().to_rfc3339();
    }

    /// Record a file edit event (tracks file type distribution).
    pub fn record_file_edit(&mut self, file_path: &str) {
        let ext = Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("no_ext")
            .to_string();

        *self.file_type_counts.entry(ext.clone()).or_insert(0) += 1;

        // Update preferred languages based on extension
        let language = ext_to_language(&ext);
        if !language.is_empty() && !self.preferred_languages.contains(&language.to_string()) {
            self.preferred_languages.push(language.to_string());
        }
        self.last_updated = chrono::Utc::now().to_rfc3339();
    }

    /// Record a task pattern (e.g., "refactored module", "added test", "fixed bug").
    pub fn record_task_pattern(&mut self, pattern: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        if let Some(existing) = self
            .task_patterns
            .iter_mut()
            .find(|p| p.description == pattern)
        {
            existing.frequency += 1;
            existing.last_seen = now.clone();
        } else {
            self.task_patterns.push(TaskPattern {
                description: pattern.to_string(),
                frequency: 1,
                last_seen: now.clone(),
            });
        }
        self.last_updated = now;
    }

    /// Generate a context summary for injection into system prompts.
    pub fn to_context_summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        // Top file types
        let mut file_types: Vec<_> = self.file_type_counts.iter().collect();
        file_types.sort_by(|a, b| b.1.cmp(a.1));
        let top_types: Vec<String> = file_types
            .iter()
            .take(5)
            .map(|(ext, count)| format!(".{} ({} files)", ext, count))
            .collect();
        if !top_types.is_empty() {
            parts.push(format!("Most edited file types: {}", top_types.join(", ")));
        }

        // Preferred languages
        if !self.preferred_languages.is_empty() {
            parts.push(format!(
                "Preferred languages: {}",
                self.preferred_languages.join(", ")
            ));
        }

        // Most used tools (by success rate)
        let mut tools: Vec<_> = self
            .tool_stats
            .iter()
            .filter(|(_, s)| s.total_calls >= 3)
            .collect();
        tools.sort_by(|a, b| {
            let rate_a = a.1.success_count as f64 / a.1.total_calls as f64;
            let rate_b = b.1.success_count as f64 / b.1.total_calls as f64;
            rate_b
                .partial_cmp(&rate_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let top_tools: Vec<String> = tools
            .iter()
            .take(5)
            .map(|(name, stats)| {
                let rate = (stats.success_count as f64 / stats.total_calls as f64 * 100.0) as u32;
                format!("{} ({}% success, {} calls)", name, rate, stats.total_calls)
            })
            .collect();
        if !top_tools.is_empty() {
            parts.push(format!("Effective tools: {}", top_tools.join(", ")));
        }

        // Task patterns
        let mut patterns = self.task_patterns.clone();
        patterns.sort_by_key(|p| std::cmp::Reverse(p.frequency));
        let top_patterns: Vec<String> = patterns
            .iter()
            .take(3)
            .map(|p| format!("{} ({}x)", p.description, p.frequency))
            .collect();
        if !top_patterns.is_empty() {
            parts.push(format!("Common tasks: {}", top_patterns.join(", ")));
        }

        if parts.is_empty() {
            String::new()
        } else {
            format!("[USER_PREFERENCES] {}", parts.join(". "))
        }
    }
}

/// Map file extension to programming language name.
fn ext_to_language(ext: &str) -> &str {
    match ext {
        "rs" => "Rust",
        "ts" | "tsx" => "TypeScript",
        "js" | "jsx" => "JavaScript",
        "py" => "Python",
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "swift" => "Swift",
        "rb" => "Ruby",
        "php" => "PHP",
        "cs" => "C#",
        "cpp" | "cc" | "cxx" => "C++",
        "c" => "C",
        "h" | "hpp" => "C/C++ Header",
        "html" | "htm" => "HTML",
        "css" | "scss" | "sass" => "CSS/SCSS",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "md" => "Markdown",
        "sh" | "bash" => "Shell",
        "sql" => "SQL",
        _ => "",
    }
}
