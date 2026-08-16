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

        for lesson in &mut self.inner.lessons {
            if lesson.project == project && lesson.signature == signature {
                lesson.count = lesson.count.saturating_add(1);
                lesson.last_seen = now.clone();
                if !snippet.is_empty() {
                    lesson.snippet = snippet.clone();
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
            out.push_str(&format!(
                "{}. [{}×] {} (surfaced by {}) — {}\n",
                i + 1,
                l.count,
                l.signature,
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
}
