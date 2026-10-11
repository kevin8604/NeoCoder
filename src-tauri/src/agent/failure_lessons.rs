//! Cross-session failure lessons.
//!
//! Persists error signatures (e.g. `E0308`, `TS2345`, `ModuleNotFoundError`)
//! surfaced by build/test/diagnostic tools, keyed by project path. Each new
//! agent session reads the lessons learned in past sessions for the current
//! project and injects them into the system prompt, so the model can apply a
//! known fix directly instead of debugging the same problem from scratch.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A single learned failure pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureLesson {
    /// Absolute project path this lesson was learned in.
    pub project: String,
    /// Normalized error signature, e.g. "E0308", "TS2345", "ModuleNotFoundError".
    pub signature: String,
    /// Tool that surfaced the error.
    pub tool: String,
    /// Short representative error line (≤120 chars).
    pub snippet: String,
    /// File the error pointed at, when the output carried a `path:line` /
    /// `path(line,col)` / `--> path:line:col` location. Makes the injected
    /// lesson code-aware (where it broke), not just what broke.
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    /// Enclosing symbol (function/class declaration line) resolved from the
    /// file's tree-sitter chunks, when the file is parseable and the location
    /// falls inside a symbol. Turns "where" into "which function".
    #[serde(default)]
    pub symbol: Option<String>,
    /// How many times this signature has been observed.
    pub count: u32,
    pub first_seen: String,
    pub last_seen: String,
}

/// On-disk shape of the lessons file.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FailureLessonsFile {
    pub lessons: Vec<FailureLesson>,
}

/// Maximum lessons kept across all projects.
const MAX_LESSONS: usize = 200;
/// Maximum lessons injected per project per session.
const INJECT_LIMIT: usize = 8;
/// Results larger than this are not analyzed (avoid scanning huge outputs).
const MAX_ANALYZE_CHARS: usize = 120_000;

/// Signature matchers, ordered by specificity. First match on a line wins.
/// Capture group 1 (when present) becomes the normalized signature.
fn extract_signature(result: &str) -> Option<String> {
    use std::sync::LazyLock;
    static MATCHERS: LazyLock<Vec<(regex::Regex, &'static str)>> = LazyLock::new(|| {
        vec![
            // Rust compiler: error[E0308]: mismatched types
            (regex::Regex::new(r"error\[(E\d{4})\]").expect("regex"), "rust"),
            // TypeScript / tsc: TS2345
            (regex::Regex::new(r"\b(TS\d{4,5})\b").expect("regex"), "ts"),
            // Python exceptions
            (regex::Regex::new(r"\b(ModuleNotFoundError|ImportError|AttributeError|TypeError|ValueError|KeyError|IndexError|SyntaxError|IndentationError|NameError|FileNotFoundError|ConnectionError|OSError)\b")
                .expect("regex"), "py"),
            // Go compiler
            (regex::Regex::new(r"\b(undefined:|cannot use|not enough arguments|too many arguments|missing return|implicit assignment)\b")
                .expect("regex"), "go"),
            // Shell / filesystem
            (regex::Regex::new(r"\b(no such file or directory|permission denied|command not found|is not recognized)\b")
                .expect("regex"), "fs"),
        ]
    });

    for line in result.lines().take(400) {
        for (re, _) in MATCHERS.iter() {
            if let Some(caps) = re.captures(line) {
                let sig = caps
                    .get(1)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_else(|| line.trim().to_string());
                let sig = sig.trim().chars().take(60).collect::<String>();
                if !sig.is_empty() {
                    return Some(sig);
                }
            }
        }
    }

    // Fallback: bare "error:" / "error[...]:" summary lines (short compiler output).
    for line in result.lines().take(200) {
        let t = line.trim();
        let lower = t.to_lowercase();
        if (lower.starts_with("error:") || lower.starts_with("error["))
            && t.chars().count() > 8
            && t.chars().count() <= 200
        {
            return Some(t.chars().take(60).collect());
        }
    }

    None
}

/// Pick a short representative line for the snippet.
fn pick_snippet(result: &str) -> String {
    result
        .lines()
        .find(|l| {
            let t = l.trim().to_lowercase();
            t.contains("error") || t.contains("failed") || t.contains("panicked")
        })
        .unwrap_or_else(|| result.lines().next().unwrap_or(""))
        .trim()
        .chars()
        .take(120)
        .collect()
}

/// Extract a source location (`file`, `line`) from tool output, so an injected
/// lesson can name *where* the failure happened, not just its signature.
/// Recognizes the shapes compilers emit:
///   - Rust:        `--> src/main.rs:10:5`  /  `src/main.rs:10:5: error ...`
///   - TS/JS:       `src/app.tsx(12,5): error TS2345 ...`
///   - Generic:     `path/to/file.py:42: ...`
///
/// Returns the first match; `(None, None)` when no location is present.
fn extract_location(result: &str) -> (Option<String>, Option<u32>) {
    use std::sync::LazyLock;
    // Capture group 1 = path, group 2 = line number. `path:line` and `path(line,`
    // both reduce to a shared extension-anchored pattern.
    static RX: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"([\w./\\-]+\.(?:rs|tsx|jsx|ts|js|py|go|c|cc|cpp|h|hpp|java|kt|kts|rb|php|cs|swift))(?::|\()(\d+)",
        )
        .expect("location regex")
    });

    for line in result.lines().take(400) {
        // Prefer the canonical `--> path:line:col` marker when present.
        if let Some(cap) = RX.captures(line) {
            let path = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let clean = path.trim_start_matches("./").trim_start_matches('/');
            if !clean.is_empty() {
                let file = clean.chars().take(200).collect::<String>();
                let num = cap.get(2).and_then(|m| m.as_str().parse::<u32>().ok());
                return (Some(file), num);
            }
        }
    }
    (None, None)
}

/// Pick the innermost chunk that spans `line` and return its declaration
/// summary (≈ the enclosing function/class). Pure: works on any chunk list, so
/// the selection logic is testable without disk or a parser.
#[cfg_attr(not(feature = "treesitter"), allow(dead_code))]
fn symbol_from_chunks(chunks: &[crate::rag::CodeChunk], line: usize) -> Option<String> {
    chunks
        .iter()
        .filter(|c| c.start_line <= line && line <= c.end_line)
        .min_by_key(|c| c.end_line - c.start_line)
        .map(|c| c.summary.trim().chars().take(80).collect::<String>())
        .filter(|s| !s.is_empty())
}

/// Resolve the enclosing symbol for `file:line` by reading the file and running
/// tree-sitter chunking. Best-effort: returns `None` on any miss (file gone,
/// unsupported language, line outside every symbol) so lesson recording never
/// depends on it. Gated behind the optional `treesitter` feature — with it off,
/// symbol resolution is a no-op (no disk read) and lessons still carry file:line.
fn resolve_symbol(project: &str, file: &str, line: u32) -> Option<String> {
    #[cfg(not(feature = "treesitter"))]
    {
        let _ = (project, file, line);
        None
    }
    #[cfg(feature = "treesitter")]
    {
        let path = if std::path::Path::new(file).is_absolute() {
            std::path::PathBuf::from(file)
        } else {
            std::path::Path::new(project).join(file)
        };
        let content = std::fs::read_to_string(&path).ok()?;
        let chunks = crate::rag::treesitter::chunk_with_tree_sitter(file, &content)?;
        symbol_from_chunks(&chunks, line as usize)
    }
}

/// JSON-backed store of failure lessons.
pub struct FailureLessonsStore {
    path: PathBuf,
    inner: FailureLessonsFile,
}

impl FailureLessonsStore {
    /// Load the store from disk; missing/corrupt files start empty.
    pub fn load(path: PathBuf) -> Self {
        let inner = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { path, inner }
    }

    /// Record an error result from a tool. Deduplicates by (project, signature)
    /// and bumps the occurrence count. No-op when no error signature is found.
    pub fn record(&mut self, project: &str, tool: &str, result: &str) {
        let analyzed: &str = if result.len() > MAX_ANALYZE_CHARS {
            // Errors usually appear at the tail of large outputs.
            let skip = result.len() - MAX_ANALYZE_CHARS;
            &result[skip..]
        } else {
            result
        };

        let Some(signature) = extract_signature(analyzed) else {
            return;
        };

        let now = chrono::Utc::now().to_rfc3339();
        let snippet = pick_snippet(analyzed);
        let (file, line) = extract_location(analyzed);
        let symbol = match (&file, line) {
            (Some(f), Some(n)) => resolve_symbol(project, f, n),
            _ => None,
        };

        for lesson in &mut self.inner.lessons {
            if lesson.project == project && lesson.signature == signature {
                lesson.count = lesson.count.saturating_add(1);
                lesson.last_seen = now.clone();
                if !snippet.is_empty() {
                    lesson.snippet = snippet.clone();
                }
                // Fill in a location on later observations if the first one
                // didn't carry it (e.g. a bare signature line came first).
                if lesson.file.is_none() {
                    lesson.file = file.clone();
                    lesson.line = line;
                }
                if lesson.symbol.is_none() {
                    lesson.symbol = symbol.clone();
                }
                self.save();
                return;
            }
        }

        self.inner.lessons.push(FailureLesson {
            project: project.to_string(),
            signature,
            tool: tool.to_string(),
            snippet,
            file,
            line,
            symbol,
            count: 1,
            first_seen: now.clone(),
            last_seen: now,
        });
        self.prune();
        self.save();
    }

    /// Drop oldest lessons beyond the cap (most recent first).
    fn prune(&mut self) {
        if self.inner.lessons.len() <= MAX_LESSONS {
            return;
        }
        self.inner
            .lessons
            .sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        self.inner.lessons.truncate(MAX_LESSONS);
    }

    /// Merge an incoming set of lessons (e.g. from an imported knowledge pack)
    /// into the store. Union by (project, signature); returns how many were
    /// newly added. Delegates to the pack module's pure merge, then prunes and
    /// persists so the next session's hook picks the lessons up.
    pub fn merge_lessons(&mut self, incoming: &[FailureLesson]) -> usize {
        let added = crate::agent::eval::pack::merge_lessons(&mut self.inner.lessons, incoming);
        self.prune();
        self.save();
        added
    }

    /// Persist to disk (best-effort).
    pub fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.inner) {
            let _ = std::fs::write(&self.path, json);
        }
    }

    /// Lessons for a project, most recent first, capped at `limit`.
    pub fn lessons_for(&self, project: &str, limit: usize) -> Vec<&FailureLesson> {
        let mut items: Vec<&FailureLesson> = self
            .inner
            .lessons
            .iter()
            .filter(|l| l.project == project)
            .collect();
        items.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        items.truncate(limit);
        items
    }

    /// Format lessons as a system-prompt block. Empty when nothing relevant.
    pub fn format_for_prompt(&self, project: &str) -> String {
        let lessons = self.lessons_for(project, INJECT_LIMIT);
        if lessons.is_empty() {
            return String::new();
        }

        let mut out = String::from(
            "Known failure patterns in this project (learned from past sessions — \
             apply the known fix directly instead of debugging from scratch):\n",
        );
        for (i, l) in lessons.iter().enumerate() {
            let at = match (&l.file, l.line) {
                (Some(f), Some(n)) => format!(" at {f}:{n}"),
                (Some(f), None) => format!(" in {f}"),
                _ => String::new(),
            };
            let sym = l
                .symbol
                .as_ref()
                .map(|s| format!(" [{s}]"))
                .unwrap_or_default();
            out.push_str(&format!(
                "{}. [{}×] {}{}{} (surfaced by {}) — {}\n",
                i + 1,
                l.count,
                l.signature,
                at,
                sym,
                l.tool,
                if l.snippet.is_empty() {
                    "(no snippet)"
                } else {
                    &l.snippet
                }
            ));
        }
        out
    }

    /// All lessons across every project (for the management dashboard).
    pub fn all(&self) -> &[FailureLesson] {
        &self.inner.lessons
    }

    /// Remove the lesson matching (project, signature). Returns whether one was
    /// removed. Used to dismiss a false-positive or now-obsolete lesson.
    pub fn remove(&mut self, project: &str, signature: &str) -> bool {
        let before = self.inner.lessons.len();
        self.inner
            .lessons
            .retain(|l| !(l.project == project && l.signature == signature));
        let removed = self.inner.lessons.len() < before;
        if removed {
            self.save();
        }
        removed
    }

    /// Remove every lesson for a project. Returns how many were removed.
    pub fn remove_project(&mut self, project: &str) -> usize {
        let before = self.inner.lessons.len();
        self.inner.lessons.retain(|l| l.project != project);
        let removed = before - self.inner.lessons.len();
        if removed > 0 {
            self.save();
        }
        removed
    }

    /// Wipe all lessons (all projects). Returns how many were removed.
    pub fn clear(&mut self) -> usize {
        let n = self.inner.lessons.len();
        self.inner.lessons.clear();
        if n > 0 {
            self.save();
        }
        n
    }

    /// Drop lessons whose `last_seen` is older than `days`. A lesson that has not
    /// resurfaced recently is likely fixed or irrelevant; removing it keeps the
    /// injected prompt focused on live failure patterns. Returns removed count.
    pub fn prune_older_than(&mut self, days: i64) -> usize {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days);
        let before = self.inner.lessons.len();
        self.inner.lessons.retain(|l| {
            chrono::DateTime::parse_from_rfc3339(&l.last_seen)
                .map(|ts| ts.with_timezone(&chrono::Utc) >= cutoff)
                // Keep lessons with unparseable timestamps (fail open, don't lose data).
                .unwrap_or(true)
        });
        let removed = before - self.inner.lessons.len();
        if removed > 0 {
            self.save();
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rust_signature() {
        let out = "error[E0308]: mismatched types\n --> src/main.rs:10:5";
        assert_eq!(extract_signature(out).as_deref(), Some("E0308"));
    }

    #[test]
    fn extracts_ts_signature() {
        let out = "src/app.tsx(12,5): error TS2345: Argument of type 'string' is not assignable";
        assert_eq!(extract_signature(out).as_deref(), Some("TS2345"));
    }

    #[test]
    fn extracts_python_signature() {
        let out = "ModuleNotFoundError: No module named 'requests'";
        assert_eq!(
            extract_signature(out).as_deref(),
            Some("ModuleNotFoundError")
        );
    }

    #[test]
    fn falls_back_to_error_line() {
        let out = "error: could not compile `demo` (bin \"demo\")";
        assert_eq!(
            extract_signature(out).as_deref(),
            Some("error: could not compile `demo` (bin \"demo\")")
        );
    }

    #[test]
    fn ignores_clean_output() {
        assert_eq!(
            extract_signature("All tests passed. 42 passed, 0 failed."),
            None
        );
        assert_eq!(extract_signature("Finished `dev` profile"), None);
    }

    #[test]
    fn dedups_and_bumps_count() {
        let dir = std::env::temp_dir().join(format!("nee-lessons-test-{}", uuid::Uuid::new_v4()));
        let mut store = FailureLessonsStore::load(dir.join("lessons.json"));
        store.record("/proj", "run_tests", "error[E0308]: mismatched types");
        store.record("/proj", "run_tests", "error[E0308]: mismatched types");
        store.record("/other", "run_build", "error[E0433]: failed to resolve");
        assert_eq!(store.lessons_for("/proj", 10).len(), 1);
        assert_eq!(store.lessons_for("/proj", 10)[0].count, 2);
        assert_eq!(store.lessons_for("/other", 10)[0].signature, "E0433");
        let prompt = store.format_for_prompt("/proj");
        assert!(prompt.contains("E0308"));
        assert!(store.format_for_prompt("/nonexistent").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_and_remove_project_and_clear() {
        let dir = std::env::temp_dir().join(format!("nee-lessons-rm-{}", uuid::Uuid::new_v4()));
        let mut store = FailureLessonsStore::load(dir.join("lessons.json"));
        store.record("/p", "run_build", "error[E0308]: mismatched types");
        store.record("/p", "run_build", "error[E0433]: failed to resolve");
        store.record("/q", "run_tests", "error[E0308]: mismatched types");
        assert_eq!(store.all().len(), 3);

        assert!(store.remove("/p", "E0308"));
        assert!(!store.remove("/p", "E0308"), "already gone → false");
        assert_eq!(store.all().len(), 2);

        assert_eq!(store.remove_project("/p"), 1, "only /p's E0433 remains");
        assert_eq!(store.all().len(), 1);

        assert_eq!(store.clear(), 1);
        assert!(store.all().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_older_than_drops_stale() {
        let dir = std::env::temp_dir().join(format!("nee-lessons-prune-{}", uuid::Uuid::new_v4()));
        let path = dir.join("lessons.json");
        // Seed one recent and one very old lesson directly on disk.
        let old_ts = (chrono::Utc::now() - chrono::Duration::days(120)).to_rfc3339();
        let recent_ts = chrono::Utc::now().to_rfc3339();
        let file = FailureLessonsFile {
            lessons: vec![
                FailureLesson {
                    project: "/p".into(),
                    signature: "STALE".into(),
                    tool: "run_build".into(),
                    snippet: String::new(),
                    file: None,
                    line: None,
                    symbol: None,
                    count: 1,
                    first_seen: old_ts.clone(),
                    last_seen: old_ts,
                },
                FailureLesson {
                    project: "/p".into(),
                    signature: "FRESH".into(),
                    tool: "run_build".into(),
                    snippet: String::new(),
                    file: None,
                    line: None,
                    symbol: None,
                    count: 1,
                    first_seen: recent_ts.clone(),
                    last_seen: recent_ts,
                },
            ],
        };
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();

        let mut store = FailureLessonsStore::load(path);
        let removed = store.prune_older_than(90);
        assert_eq!(removed, 1);
        assert_eq!(store.all().len(), 1);
        assert_eq!(store.all()[0].signature, "FRESH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extracts_rust_location() {
        let (f, l) = extract_location("error[E0308]: mismatched types\n --> src/main.rs:10:5\n  |");
        assert_eq!(f.as_deref(), Some("src/main.rs"));
        assert_eq!(l, Some(10));
    }

    #[test]
    fn extracts_ts_location() {
        let (f, l) = extract_location("src/app.tsx(12,5): error TS2345: not assignable");
        assert_eq!(f.as_deref(), Some("src/app.tsx"));
        assert_eq!(l, Some(12));
    }

    #[test]
    fn no_location_when_absent() {
        let (f, l) = extract_location("error: something failed with no file path at all");
        assert_eq!(f, None);
        assert_eq!(l, None);
    }

    #[test]
    fn record_captures_location_and_prompt_shows_it() {
        let dir = std::env::temp_dir().join(format!("nee-lessons-loc-{}", uuid::Uuid::new_v4()));
        let mut store = FailureLessonsStore::load(dir.join("lessons.json"));
        store.record(
            "/proj",
            "run_build",
            "error[E0308]: mismatched types\n --> src/parser.rs:42:9",
        );
        let lessons = store.lessons_for("/proj", 10);
        assert_eq!(lessons[0].file.as_deref(), Some("src/parser.rs"));
        assert_eq!(lessons[0].line, Some(42));
        let prompt = store.format_for_prompt("/proj");
        assert!(
            prompt.contains("src/parser.rs:42"),
            "injected lesson should name the file:line, got: {prompt}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn symbol_from_chunks_picks_innermost_containing() {
        use crate::rag::{ChunkType, CodeChunk};
        let mk = |s: usize, e: usize, sum: &str| CodeChunk {
            id: format!("{s}-{e}"),
            file_path: "f.rs".into(),
            start_line: s,
            end_line: e,
            language: "rust".into(),
            chunk_type: ChunkType::Function,
            content: String::new(),
            summary: sum.into(),
        };
        let chunks = vec![
            mk(1, 100, "mod x"),
            mk(10, 30, "fn outer"),
            mk(18, 24, "fn inner"),
        ];
        // 20 sits inside both outer(10-30) and inner(18-24) → innermost wins.
        assert_eq!(symbol_from_chunks(&chunks, 20).as_deref(), Some("fn inner"));
        assert_eq!(symbol_from_chunks(&chunks, 12).as_deref(), Some("fn outer"));
        assert_eq!(symbol_from_chunks(&chunks, 5).as_deref(), Some("mod x"));
        // Outside every chunk → no symbol.
        assert_eq!(symbol_from_chunks(&chunks, 200), None);
    }

    #[test]
    fn resolve_symbol_missing_file_is_none() {
        // Best-effort: a gone/unparseable file must yield None, never panic.
        assert_eq!(resolve_symbol("/nonexistent-proj-xyz", "nope.rs", 5), None);
    }
}
