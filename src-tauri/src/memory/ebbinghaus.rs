use chrono::{NaiveDate, Utc};

// ── Memory Category ─────────────────────────────────────────────────────────

/// Classification of a memory entry, determining decay policy and injection priority.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MemoryCategory {
    /// Evergreen — exempt from time decay, always injected at full weight.
    Core,
    /// Reusable coding pattern or idiom.
    Pattern,
    /// Architecture or technology decision.
    Decision,
    /// Error resolution or pitfall learned.
    Lesson,
    /// Bug fix or error resolution with specific file/line reference.
    BugFix,
    /// API protocol or integration detail (e.g., message format, pairing rules).
    ApiProtocol,
    /// Performance optimization or bottleneck finding.
    Performance,
    /// General coding knowledge (backward-compatible with old "coding").
    Coding,
    /// Non-coding content — aggressive decay.
    General,
    /// User-defined category.
    Custom(String),
}

impl MemoryCategory {
    /// Serialize to a compact string for metadata embedding.
    pub fn to_tag(&self) -> String {
        match self {
            MemoryCategory::Core => "core".into(),
            MemoryCategory::Pattern => "pattern".into(),
            MemoryCategory::Decision => "decision".into(),
            MemoryCategory::Lesson => "lesson".into(),
            MemoryCategory::BugFix => "bugfix".into(),
            MemoryCategory::ApiProtocol => "api".into(),
            MemoryCategory::Performance => "perf".into(),
            MemoryCategory::Coding => "coding".into(),
            MemoryCategory::General => "general".into(),
            MemoryCategory::Custom(s) => format!("custom:{}", s),
        }
    }

    /// Deserialize from a metadata tag string.
    pub fn from_tag(s: &str) -> Self {
        match s {
            "core" => MemoryCategory::Core,
            "pattern" => MemoryCategory::Pattern,
            "decision" => MemoryCategory::Decision,
            "lesson" => MemoryCategory::Lesson,
            "bugfix" => MemoryCategory::BugFix,
            "api" => MemoryCategory::ApiProtocol,
            "perf" => MemoryCategory::Performance,
            "coding" => MemoryCategory::Coding,
            "general" => MemoryCategory::General,
            other if other.starts_with("custom:") => {
                MemoryCategory::Custom(other.strip_prefix("custom:").unwrap_or("").to_string())
            }
            _ => MemoryCategory::General,
        }
    }

    /// Detect category from entry text tags like [Lesson], [Decision], [Pattern].
    /// Falls back to MemoryCategory::Coding if no tag matches (backward compatible).
    pub fn detect_from_text(text: &str) -> Self {
        let lower = text.to_lowercase();
        if lower.contains("[bugfix]") || lower.contains("[bug]") || lower.contains("[fix]") {
            MemoryCategory::BugFix
        } else if lower.contains("[api]") || lower.contains("[protocol]") {
            MemoryCategory::ApiProtocol
        } else if lower.contains("[perf]") || lower.contains("[performance]") {
            MemoryCategory::Performance
        } else if lower.contains("[lesson]") {
            MemoryCategory::Lesson
        } else if lower.contains("[decision]") {
            MemoryCategory::Decision
        } else if lower.contains("[pattern]") {
            MemoryCategory::Pattern
        } else if lower.contains("[core]") {
            MemoryCategory::Core
        } else {
            MemoryCategory::Coding
        }
    }

    /// Whether this category represents coding-domain knowledge.
    pub fn is_coding(&self) -> bool {
        matches!(self, MemoryCategory::Core | MemoryCategory::Pattern | MemoryCategory::Decision | MemoryCategory::Lesson | MemoryCategory::BugFix | MemoryCategory::ApiProtocol | MemoryCategory::Performance | MemoryCategory::Coding)
    }

    /// Whether this category is exempt from Ebbinghaus decay.
    pub fn is_evergreen(&self) -> bool {
        matches!(self, MemoryCategory::Core)
    }

    /// Stability growth per recall event.
    /// - Core: no decay, growth is irrelevant
    /// - Pattern/Decision/Coding: normal ln(N+1) growth
    /// - Lesson: slightly faster decay (0.8x ln growth — errors should fade quicker)
    /// - General/Custom: capped slow growth to accelerate forgetting
    pub fn stability_growth(&self, recall_count: u32) -> f64 {
        match self {
            MemoryCategory::Core => 0.0,
            MemoryCategory::Pattern | MemoryCategory::Decision | MemoryCategory::Coding => {
                (recall_count as f64 + 1.0).ln()
            }
            MemoryCategory::BugFix | MemoryCategory::ApiProtocol => {
                (recall_count as f64 + 1.0).ln() * 1.1 // slightly stronger retention — precise facts
            }
            MemoryCategory::Performance => {
                (recall_count as f64 + 1.0).ln() * 0.9
            }
            MemoryCategory::Lesson => {
                (recall_count as f64 + 1.0).ln() * 0.8
            }
            MemoryCategory::General | MemoryCategory::Custom(_) => 0.2,
        }
    }

    /// Score bonus applied during context injection.
    pub fn injection_bonus(&self) -> f64 {
        match self {
            MemoryCategory::Core => 2.0,
            MemoryCategory::BugFix | MemoryCategory::ApiProtocol => 1.5, // high value — precise facts
            MemoryCategory::Pattern => 1.0,
            MemoryCategory::Decision => 1.0,
            MemoryCategory::Performance => 1.0,
            MemoryCategory::Lesson => 0.8,
            MemoryCategory::Coding => 1.0,
            MemoryCategory::General => 0.0,
            MemoryCategory::Custom(_) => 0.0,
        }
    }

    /// Archive threshold parameters: (R_threshold, min_days_since_recall)
    pub fn archive_params(&self) -> (f64, i64) {
        match self {
            MemoryCategory::Core => (0.0, 365 * 100),
            MemoryCategory::BugFix | MemoryCategory::ApiProtocol => (0.02, 90), // long-lived — precise facts
            MemoryCategory::Pattern | MemoryCategory::Decision => (0.02, 60),
            MemoryCategory::Performance => (0.03, 45),
            MemoryCategory::Lesson => (0.05, 45),
            MemoryCategory::Coding => (0.05, 30),
            MemoryCategory::General => (0.5, 7),
            MemoryCategory::Custom(_) => (0.5, 7),
        }
    }
}

// ── Memory Entry ────────────────────────────────────────────────────────────

/// A single memory entry with Ebbinghaus forgetting curve metadata.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    /// Unique identifier (UUIDv4).
    pub id: String,
    /// Optional named key for idempotent store/retrieve (e.g., "user_pref:lang").
    pub key: Option<String>,
    /// The text content of the memory (e.g., "- [Lesson] Use LoRA with small LR")
    pub text: String,
    /// Date this entry was first created
    pub created: NaiveDate,
    /// Date this entry was last recalled/injected into context
    pub last_recalled: NaiveDate,
    /// Total number of times this entry has been recalled
    pub recall_count: u32,
    /// Memory stability (days). Higher = slower decay.
    /// Grows each time the entry is recalled.
    pub stability: f64,
    /// Section heading this entry belongs to (e.g., "Learned Patterns")
    pub section: String,
    /// Domain category for noise filtering and decay policy.
    pub category: MemoryCategory,
    /// Optional session scope — ties this entry to a specific conversation.
    pub session_id: Option<String>,
}

/// Coding-related keywords for relevance scoring.
static CODING_KEYWORDS: &[&str] = &[
    "rust", "tauri", "react", "typescript", "javascript", "python", "go", "java", "c++",
    "api", "async", "await", "tokio", "function", "struct", "trait", "impl", "enum",
    "compile", "cargo", "npm", "vite", "webpack", "docker", "git", "commit", "branch",
    "error", "bug", "fix", "debug", "refactor", "test", "code", "module", "import",
    "component", "hook", "state", "props", "render", "build", "deploy", "server",
    "database", "sql", "query", "schema", "migration", "config", "cli", "sdk",
    "pattern", "architecture", "design", "framework", "library", "dependency",
    "memory", "cache", "thread", "lock", "mutex", "channel", "future", "stream",
    "parsing", "serialization", "json", "yaml", "toml", "http", "rest", "rpc",
];

/// Compute keyword-based relevance of a memory entry to coding context.
/// Returns a score in [0.0, 1.0] based on coding keyword density.
pub fn compute_coding_relevance(text: &str) -> f64 {
    let lower = text.to_lowercase();
    let word_count = lower.split_whitespace().count().max(1) as f64;
    let matched: usize = CODING_KEYWORDS.iter()
        .filter(|kw| lower.contains(*kw))
        .count();
    // Score = matched_keywords / sqrt(word_count) — penalizes long text, rewards density
    let score = matched as f64 / word_count.sqrt();
    score.min(1.0)
}

/// Compute Jaccard similarity between two texts based on word overlap.
/// Lightweight — no embedding model needed. Returns [0.0, 1.0].
pub fn compute_similarity(a: &str, b: &str) -> f64 {
    let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();
    if words_a.is_empty() && words_b.is_empty() {
        return 1.0;
    }
    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f64 / union as f64
}

/// Extract significant keywords/phrases from text for topic-level dedup.
/// Returns lowercase tokens filtered to technical terms (file paths, tech names, APIs).
fn extract_topic_keywords(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut keywords = Vec::new();

    // Extract file paths (e.g., "pty.rs", "mod.rs", "agent/mod.rs")
    for word in lower.split_whitespace() {
        let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '/' && c != '-' && c != '_');
        if w.contains('.') && !w.starts_with('.') && w.len() > 3 {
            keywords.push(w.to_string());
        }
    }

    // Extract technical terms (camelCase, snake_case, known tech names)
    let tech_terms = [
        "xterm", "pty", "portable-pty", "tauri", "rust", "tokio", "react",
        "typescript", "python", "cargo", "npm", "api", "json", "jsonl",
        "websocket", "stdio", "stdin", "stdout", "resize", "loop",
        "iteration", "agent", "tool", "memory", "config", "cli",
        "stream", "async", "await", "mutex", "lock", "thread",
        "compile", "error", "panic", "debug", "test", "build",
        "deepseek", "openai", "claude", "anthropic", "ollama",
        "gpt-4", "gpt-4o", "qwen", "llama",
    ];
    for term in &tech_terms {
        if lower.contains(term) {
            keywords.push(term.to_string());
        }
    }

    keywords.sort();
    keywords.dedup();
    keywords
}

/// Topic-level similarity: compares extracted keywords rather than raw word overlap.
/// More robust against paraphrasing — "Use xterm.js for terminal" and
/// "Embed xterm.js in the panel" both extract ["xterm", "terminal"].
/// Returns [0.0, 1.0].
pub fn compute_topic_similarity(a: &str, b: &str) -> f64 {
    let kw_a = extract_topic_keywords(a);
    let kw_b = extract_topic_keywords(b);

    if kw_a.is_empty() && kw_b.is_empty() {
        return compute_similarity(a, b); // fallback to Jaccard
    }
    if kw_a.is_empty() || kw_b.is_empty() {
        return 0.0;
    }

    let set_a: std::collections::HashSet<&str> = kw_a.iter().map(|s| s.as_str()).collect();
    let set_b: std::collections::HashSet<&str> = kw_b.iter().map(|s| s.as_str()).collect();
    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f64 / union as f64
}

impl MemoryEntry {
    /// Create a new entry with default stability (S=1.0), auto-generated UUID.
    pub fn new(text: String, section: String) -> Self {
        let today = Utc::now().date_naive();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            key: None,
            text,
            created: today,
            last_recalled: today,
            recall_count: 0,
            stability: 1.0,
            section,
            category: MemoryCategory::Coding,
            session_id: None,
        }
    }

    /// Create a new entry with explicit category.
    pub fn with_category(text: String, section: String, category: MemoryCategory) -> Self {
        let mut entry = Self::new(text, section);
        entry.category = category;
        entry
    }

    /// Create a new entry tied to a specific session.
    pub fn with_session(text: String, section: String, category: MemoryCategory, session_id: String) -> Self {
        let mut entry = Self::with_category(text, section, category);
        entry.session_id = Some(session_id);
        entry
    }
}

/// Compute retention value using Ebbinghaus formula: R = e^(-t/S)
///
/// - `entry`: The memory entry to compute retention for
/// - `now`: Reference date (typically today)
///
/// Returns a value in [0, 1] where 1 = fully retained, 0 = completely forgotten.
pub fn compute_retention(entry: &MemoryEntry, now: NaiveDate) -> f64 {
    let days_since = (now - entry.last_recalled).num_days().max(0) as f64;
    if entry.stability <= 0.0 {
        return 0.0;
    }
    (-days_since / entry.stability).exp()
}

/// Update entry metadata after a recall event.
///
/// Each recall boosts the stability S according to the entry's category policy.
///
/// Category-specific growth rates (see MemoryCategory::stability_growth):
/// - Core: no growth (evergreen)
/// - Pattern/Decision/Coding: ln(recall_count + 1)
/// - Lesson: 0.8 * ln(recall_count + 1)
/// - General/Custom: +0.2 (capped slow growth)
pub fn update_recall(entry: &mut MemoryEntry) {
    entry.recall_count += 1;
    entry.last_recalled = Utc::now().date_naive();
    let growth = entry.category.stability_growth(entry.recall_count);
    entry.stability += growth;
}

/// Archive threshold: entries with R below this value for too long are candidates for archiving.
pub const ARCHIVE_THRESHOLD: f64 = 0.05;

/// Minimum days since last recall before an entry can be considered for archiving.
pub const ARCHIVE_MIN_DAYS: i64 = 30;

/// Check if an entry should be archived (low retention + old).
/// Uses category-specific thresholds from MemoryCategory::archive_params().
/// Core entries are never archived.
pub fn should_archive(entry: &MemoryEntry, now: NaiveDate) -> bool {
    let days_since = (now - entry.last_recalled).num_days();
    let retention = compute_retention(entry, now);
    let (threshold, min_days) = entry.category.archive_params();
    retention < threshold && days_since > min_days
}

/// Format metadata as an HTML comment line for embedding in Markdown.
///
/// Compact format: `<!-- mem: recalled=YYYY-MM-DD count=N S=X.X cat=tag [key=...] [sid=...] -->`
/// Removed `id` and `created` to reduce file size (~40% savings).
/// Backward compatible: parser still accepts old format with id/created.
pub fn format_metadata(entry: &MemoryEntry) -> String {
    let mut meta = format!(
        "<!-- mem: recalled={} count={} S={:.2} cat={}",
        entry.last_recalled.format("%Y-%m-%d"),
        entry.recall_count,
        entry.stability,
        entry.category.to_tag(),
    );
    if let Some(ref key) = entry.key {
        meta.push_str(&format!(" key={}", key));
    }
    if let Some(ref sid) = entry.session_id {
        meta.push_str(&format!(" sid={}", sid));
    }
    meta.push_str(" -->");
    meta
}

/// Parsed metadata from a memory comment line.
pub struct ParsedMeta {
    pub id: Option<String>,
    pub created: NaiveDate,
    pub recalled: NaiveDate,
    pub count: u32,
    pub stability: f64,
    pub category: MemoryCategory,
    pub key: Option<String>,
    pub session_id: Option<String>,
}

/// Parse metadata from an HTML comment line.
///
/// Expects format: `<!-- mem: id=UUID created=YYYY-MM-DD recalled=YYYY-MM-DD count=N S=X.X cat=category [key=...] [sid=...] -->`
pub fn parse_metadata(line: &str) -> Option<ParsedMeta> {
    let trimmed = line.trim();
    if !trimmed.starts_with("<!-- mem:") || !trimmed.ends_with("-->") {
        return None;
    }

    let inner = trimmed
        .strip_prefix("<!-- mem:")?
        .strip_suffix("-->")?
        .trim();

    let mut id = None;
    let mut created = None;
    let mut recalled = None;
    let mut count = None;
    let mut stability = None;
    let mut category = None;
    let mut key = None;
    let mut session_id = None;

    for part in inner.split_whitespace() {
        if let Some(val) = part.strip_prefix("id=") {
            id = Some(val.to_string());
        } else if let Some(val) = part.strip_prefix("created=") {
            created = NaiveDate::parse_from_str(val, "%Y-%m-%d").ok();
        } else if let Some(val) = part.strip_prefix("recalled=") {
            recalled = NaiveDate::parse_from_str(val, "%Y-%m-%d").ok();
        } else if let Some(val) = part.strip_prefix("count=") {
            count = val.parse::<u32>().ok();
        } else if let Some(val) = part.strip_prefix("S=") {
            stability = val.parse::<f64>().ok();
        } else if let Some(val) = part.strip_prefix("cat=") {
            category = Some(MemoryCategory::from_tag(val));
        } else if let Some(val) = part.strip_prefix("key=") {
            key = Some(val.to_string());
        } else if let Some(val) = part.strip_prefix("sid=") {
            session_id = Some(val.to_string());
        }
    }

    // Accept both old format (with id+created) and new compact format
    match (recalled, count, stability) {
        (Some(r), Some(cnt), Some(s)) => Some(ParsedMeta {
            id,
            created: created.unwrap_or_else(|| Utc::now().date_naive()),
            recalled: r,
            count: cnt,
            stability: s,
            category: category.unwrap_or(MemoryCategory::Coding),
            key,
            session_id,
        }),
        _ => None,
    }
}

/// Parse MEMORY.md content into structured entries with metadata.
///
/// MEMORY.md format:
/// ```markdown
/// ## Section Name
///
/// - Entry text
/// <!-- mem: id=... created=... recalled=... count=... S=... cat=... -->
/// - Another entry
/// <!-- mem: ... -->
/// ```
///
/// Entries without metadata get default values (backward compatible).
pub fn parse_memory_entries(content: &str) -> Vec<MemoryEntry> {
    let mut entries = Vec::new();
    let mut current_section = String::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        // Detect section headings
        if line.starts_with("## ") {
            current_section = line.strip_prefix("## ").unwrap_or("").to_string();
            i += 1;
            continue;
        }

        // Detect entry lines (start with "- ")
        if line.starts_with("- ") {
            let text = line.to_string();

            // Check if next line is metadata
            let entry = if i + 1 < lines.len() {
                if let Some(meta) = parse_metadata(lines[i + 1]) {
                    i += 1; // skip metadata line
                    MemoryEntry {
                        id: meta.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                        key: meta.key,
                        text,
                        created: meta.created,
                        last_recalled: meta.recalled,
                        recall_count: meta.count,
                        stability: meta.stability,
                        section: current_section.clone(),
                        category: meta.category,
                        session_id: meta.session_id,
                    }
                } else {
                    // No metadata — use defaults (backward compatible)
                    MemoryEntry::new(text, current_section.clone())
                }
            } else {
                MemoryEntry::new(text, current_section.clone())
            };

            entries.push(entry);
        }

        i += 1;
    }

    entries
}

/// Serialize a list of entries back to MEMORY.md format.
pub fn serialize_memory_entries(entries: &[MemoryEntry]) -> String {
    let mut output = String::new();
    let mut current_section = String::new();

    for entry in entries {
        if entry.section != current_section {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&format!("\n## {}\n\n", entry.section));
            current_section = entry.section.clone();
        }
        output.push_str(&entry.text);
        output.push('\n');
        output.push_str(&format_metadata(entry));
        output.push('\n');
    }

    output
}

// ── Memory Backend Trait ───────────────────────────────────────────────────

/// Pluggable storage backend for memory entries.
/// Allows swapping file-based, database, or remote storage without changing
/// the memory manager logic.
pub trait MemoryBackend {
    /// Store a new memory entry. Returns the entry's id.
    fn store(&self, entry: &MemoryEntry) -> Result<String, String>;
    /// Recall an entry by its unique id.
    fn recall_by_id(&self, id: &str) -> Result<Option<MemoryEntry>, String>;
    /// Recall an entry by its named key (idempotent access pattern).
    fn recall_by_key(&self, key: &str) -> Result<Option<MemoryEntry>, String>;
    /// List all entries, optionally filtered by category.
    fn list_all(&self, category_filter: Option<&MemoryCategory>) -> Result<Vec<MemoryEntry>, String>;
    /// Remove an entry by its id.
    fn forget(&self, id: &str) -> Result<(), String>;
    /// Total number of stored entries.
    fn count(&self) -> Result<usize, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_retention_same_day() {
        let entry = MemoryEntry::new("- test".to_string(), "Test".to_string());
        let now = Utc::now().date_naive();
        let r = compute_retention(&entry, now);
        assert!((r - 1.0).abs() < 0.001, "Same day should be R≈1.0");
    }

    #[test]
    fn test_compute_retention_decay() {
        let mut entry = MemoryEntry::new("- test".to_string(), "Test".to_string());
        entry.last_recalled = NaiveDate::from_ymd_opt(2026, 6, 20).unwrap();
        let now = NaiveDate::from_ymd_opt(2026, 6, 26).unwrap(); // 6 days later
        // S=1.0, t=6 → R = e^(-6) ≈ 0.0025
        let r = compute_retention(&entry, now);
        assert!(r < 0.01, "Should decay significantly with S=1.0 and t=6");
    }

    #[test]
    fn test_update_recall_increases_stability() {
        let mut entry = MemoryEntry::new("- test".to_string(), "Test".to_string());
        let s0 = entry.stability;
        update_recall(&mut entry);
        assert!(entry.stability > s0, "Stability should increase after recall");
        assert_eq!(entry.recall_count, 1);
    }

    #[test]
    fn test_metadata_roundtrip() {
        let entry = MemoryEntry {
            id: "test-id".to_string(),
            key: None,
            text: "- [Lesson] Test entry".to_string(),
            created: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
            last_recalled: NaiveDate::from_ymd_opt(2026, 6, 20).unwrap(),
            recall_count: 5,
            stability: 3.5,
            section: "Learned Patterns".to_string(),
            category: MemoryCategory::Lesson,
            session_id: None,
        };
        let meta = format_metadata(&entry);
        let parsed = parse_metadata(&meta).expect("Should parse");
        // Compact 格式有意移除了 created/id 字段（节省体积），解析器回退为当前日期
        assert_eq!(parsed.created, Utc::now().date_naive());
        assert_eq!(parsed.recalled, entry.last_recalled);
        assert_eq!(parsed.count, 5);
        assert!((parsed.stability - 3.5).abs() < 0.01);
        assert_eq!(parsed.category, MemoryCategory::Lesson);
    }

    #[test]
    fn test_parse_memory_entries_with_metadata() {
        let content = r#"
## Learned Patterns

- [Lesson] Use small learning rate
<!-- mem: created=2026-06-01 recalled=2026-06-20 count=5 S=3.50 -->
- [Decision] Use Tauri 2.0
<!-- mem: created=2026-05-15 recalled=2026-06-25 count=10 S=8.20 -->
"#;
        let entries = parse_memory_entries(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].section, "Learned Patterns");
        assert_eq!(entries[0].recall_count, 5);
        assert_eq!(entries[1].recall_count, 10);
    }

    #[test]
    fn test_parse_memory_entries_backward_compatible() {
        let content = r#"
## Learned Patterns

- [Lesson] Old entry without metadata
- [Decision] Another old entry
"#;
        let entries = parse_memory_entries(content);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].recall_count, 0);
        assert_eq!(entries[0].stability, 1.0);
    }

    #[test]
    fn test_serialize_roundtrip() {
        let entries = vec![
            MemoryEntry {
                id: "test-id".to_string(),
                key: None,
                text: "- [Lesson] Test".to_string(),
                created: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
                last_recalled: NaiveDate::from_ymd_opt(2026, 6, 20).unwrap(),
                recall_count: 3,
                stability: 2.5,
                section: "Patterns".to_string(),
                category: MemoryCategory::Lesson,
                session_id: None,
            },
        ];
        let serialized = serialize_memory_entries(&entries);
        let parsed = parse_memory_entries(&serialized);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].recall_count, 3);
    }

    #[test]
    fn test_should_archive() {
        let mut entry = MemoryEntry::new("- old".to_string(), "Test".to_string());
        // Set last recalled 60 days ago with S=1.0
        entry.last_recalled = NaiveDate::from_ymd_opt(2026, 4, 27).unwrap();
        let now = NaiveDate::from_ymd_opt(2026, 6, 26).unwrap();
        assert!(should_archive(&entry, now), "Should archive: low R + old");

        // Well-recalled entry
        entry.stability = 50.0;
        assert!(!should_archive(&entry, now), "Should not archive: high S");
    }
}
