//! Project-wide text search and replace.
//!
//! Unlike `search_codebase` (semantic RAG over indexed chunks), these commands
//! walk the raw filesystem with a regex-or-literal matcher and can apply
//! replacements across many files. The frontend exposes both modes; the LLM
//! agent's `grep` tool already covers the read side.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Files/directories we never descend into (mirrors agent::utils::SKIP_DIRS).
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    ".svn",
    ".hg",
    "dist",
    "build",
    "out",
    ".next",
    ".cache",
    "vendor",
    "__pycache__",
    ".venv",
    "venv",
    ".idea",
    ".vscode",
    ".neocoder",
];

/// One hit inside a file: 1-based line + trimmed line text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHit {
    pub file_path: String,
    pub line: u32,
    pub column: u32,
    pub preview: String,
}

/// Aggregate result for `search_in_files`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchInFilesResult {
    pub hits: Vec<SearchHit>,
    pub files_with_matches: usize,
    pub total_matches: usize,
    pub truncated: bool,
}

/// Aggregate result for `replace_in_files`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaceInFilesResult {
    pub files_changed: usize,
    pub total_replacements: usize,
    pub file_details: Vec<FileReplaceSummary>,
    pub applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileReplaceSummary {
    pub file_path: String,
    pub replacements: usize,
    /// Sample of the first N replaced lines (preview only, not full diff).
    pub samples: Vec<String>,
}

/// A matcher that maps a line to the byte ranges of its matches.
type Matcher = Box<dyn Fn(&str) -> Vec<(usize, usize)> + Send>;

/// Build a matcher for a query string; supports literal or regex mode.
fn build_matcher(query: &str, is_regex: bool, case_sensitive: bool) -> Result<Matcher, String> {
    if is_regex {
        let mut rb = regex::RegexBuilder::new(query);
        rb.case_insensitive(!case_sensitive);
        let re = rb
            .build()
            .map_err(|e| format!("Invalid regex '{}': {}", query, e))?;
        Ok(Box::new(move |line: &str| {
            re.find_iter(line).map(|m| (m.start(), m.end())).collect()
        }))
    } else {
        let needle = if case_sensitive {
            query.to_string()
        } else {
            query.to_lowercase()
        };
        let cs = case_sensitive;
        Ok(Box::new(move |line: &str| {
            let hay = if cs {
                line.to_string()
            } else {
                line.to_lowercase()
            };
            let mut out = Vec::new();
            let mut start = 0usize;
            while let Some(rel) = hay[start..].find(&needle) {
                let s = start + rel;
                let e = s + needle.len();
                out.push((s, e));
                start = e;
            }
            out
        }))
    }
}

/// Check if a filename matches a comma-separated glob-ish pattern
/// (e.g. `*.rs,*.toml`). Empty pattern means "any file".
fn file_matches_pattern(path: &Path, pattern: &str) -> bool {
    if pattern.trim().is_empty() {
        return true;
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    for raw in pattern.split(',') {
        let pat = raw.trim().to_lowercase();
        if pat.is_empty() {
            continue;
        }
        if let Some(suffix) = pat.strip_prefix('*') {
            if name.ends_with(suffix) {
                return true;
            }
        } else if name == pat {
            return true;
        }
    }
    false
}

/// Walk the project directory collecting candidate file paths.
fn collect_files(root: &Path, pattern: &str, max_files: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if files.len() >= max_files {
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push(p);
            } else if p.is_file() && file_matches_pattern(&p, pattern) {
                files.push(p);
                if files.len() >= max_files {
                    break;
                }
            }
        }
    }
    files
}

/// Search project files for `query`. Returns per-line matches with preview.
pub fn search_in_files_impl(
    root: &Path,
    query: &str,
    file_pattern: &str,
    is_regex: bool,
    case_sensitive: bool,
    max_hits: usize,
) -> Result<SearchInFilesResult, String> {
    if query.is_empty() {
        return Err("query must not be empty".to_string());
    }
    let matcher = build_matcher(query, is_regex, case_sensitive)?;
    let files = collect_files(root, file_pattern, 5000);
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut files_with_matches = 0usize;
    let mut total_matches = 0usize;
    let mut truncated = false;

    for file in files {
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => continue, // binary or unreadable
        };
        let mut file_had_hit = false;
        for (idx, line) in content.lines().enumerate() {
            let spans = matcher(line);
            if spans.is_empty() {
                continue;
            }
            file_had_hit = true;
            for (start, end) in spans {
                total_matches += 1;
                if hits.len() >= max_hits {
                    truncated = true;
                    break;
                }
                hits.push(SearchHit {
                    file_path: file.to_string_lossy().to_string(),
                    line: (idx + 1) as u32,
                    column: (start + 1) as u32,
                    preview: build_preview(line, start, end),
                });
            }
            if truncated {
                break;
            }
        }
        if file_had_hit {
            files_with_matches += 1;
        }
        if truncated {
            break;
        }
    }

    Ok(SearchInFilesResult {
        hits,
        files_with_matches,
        total_matches,
        truncated,
    })
}

fn build_preview(line: &str, start: usize, end: usize) -> String {
    // Trim to at most 160 chars around the match.
    let chars: Vec<char> = line.chars().collect();
    // Byte offsets -> char indices (best-effort).
    let mut char_start = 0usize;
    let mut byte = 0usize;
    for (i, c) in chars.iter().enumerate() {
        if byte >= start {
            char_start = i;
            break;
        }
        byte += c.len_utf8();
    }
    let mut char_end = chars.len();
    byte = 0;
    for (i, c) in chars.iter().enumerate() {
        if byte >= end {
            char_end = i;
            break;
        }
        byte += c.len_utf8();
    }
    let left = char_start.saturating_sub(40);
    let right = (char_end + 40).min(chars.len());
    let mut out: String = chars[left..right].iter().collect();
    if left > 0 {
        out = format!("…{}", out);
    }
    if right < chars.len() {
        out.push('…');
    }
    out.trim_end().to_string()
}

/// Replace matches inside files. If `dry_run` is true nothing is written; the
/// result describes what would change (used for the frontend's preview step).
pub fn replace_in_files_impl(
    root: &Path,
    query: &str,
    replacement: &str,
    file_pattern: &str,
    is_regex: bool,
    case_sensitive: bool,
    dry_run: bool,
) -> Result<ReplaceInFilesResult, String> {
    if query.is_empty() {
        return Err("query must not be empty".to_string());
    }
    let files = collect_files(root, file_pattern, 5000);
    let mut details: Vec<FileReplaceSummary> = Vec::new();
    let mut total_replacements = 0usize;

    for file in files {
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let (new_content, count, samples) =
            apply_replacements(&content, query, replacement, is_regex, case_sensitive)?;
        if count == 0 {
            continue;
        }
        total_replacements += count;
        if !dry_run {
            std::fs::write(&file, &new_content)
                .map_err(|e| format!("Failed to write {}: {}", file.display(), e))?;
        }
        details.push(FileReplaceSummary {
            file_path: file.to_string_lossy().to_string(),
            replacements: count,
            samples,
        });
    }

    Ok(ReplaceInFilesResult {
        files_changed: details.len(),
        total_replacements,
        file_details: details,
        applied: !dry_run,
    })
}

/// Apply regex/literal replacements to a single document. Returns the new
/// content, count of replacements and a few preview samples of the first
/// affected lines.
pub fn apply_replacements(
    content: &str,
    query: &str,
    replacement: &str,
    is_regex: bool,
    case_sensitive: bool,
) -> Result<(String, usize, Vec<String>), String> {
    let mut samples: Vec<String> = Vec::new();
    if is_regex {
        let mut rb = regex::RegexBuilder::new(query);
        rb.case_insensitive(!case_sensitive);
        let re = rb
            .build()
            .map_err(|e| format!("Invalid regex '{}': {}", query, e))?;
        let count = re.find_iter(content).count();
        if count == 0 {
            return Ok((content.to_string(), 0, samples));
        }
        for m in re.find_iter(content).take(3) {
            let line_start = content[..m.start()].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let line_end = content[m.end()..]
                .find('\n')
                .map(|i| m.end() + i)
                .unwrap_or(content.len());
            samples.push(content[line_start..line_end].trim().to_string());
        }
        let new = re.replace_all(content, replacement).to_string();
        Ok((new, count, samples))
    } else {
        let mut out = String::with_capacity(content.len());
        let mut count = 0usize;
        let mut i = 0usize;
        // Literal replace; case-insensitive uses a byte-safe scan.
        if case_sensitive {
            while i < content.len() {
                if content[i..].starts_with(query) {
                    out.push_str(replacement);
                    i += query.len();
                    count += 1;
                    if samples.len() < 3 {
                        // Sample: the source line containing the match.
                        let line_start = content[..i - query.len()]
                            .rfind('\n')
                            .map(|x| x + 1)
                            .unwrap_or(0);
                        let line_end = content[i..]
                            .find('\n')
                            .map(|x| i + x)
                            .unwrap_or(content.len());
                        samples.push(content[line_start..line_end].trim().to_string());
                    }
                } else {
                    // Push one char at a time to preserve UTF-8 boundaries.
                    let ch = content[i..].chars().next().unwrap();
                    out.push(ch);
                    i += ch.len_utf8();
                }
            }
        } else {
            let lower_content = content.to_lowercase();
            let lower_query = query.to_lowercase();
            let chars: Vec<char> = content.chars().collect();
            let lower_chars: Vec<char> = lower_content.chars().collect();
            let qchars: Vec<char> = lower_query.chars().collect();
            let mut idx = 0usize;
            while idx < chars.len() {
                if idx + qchars.len() <= lower_chars.len()
                    && lower_chars[idx..idx + qchars.len()] == qchars[..]
                {
                    out.push_str(replacement);
                    idx += qchars.len();
                    count += 1;
                    if samples.len() < 3 {
                        let line_start = lower_chars[..idx - qchars.len()]
                            .iter()
                            .rposition(|c| *c == '\n')
                            .map(|x| x + 1)
                            .unwrap_or(0);
                        let line_end = lower_chars[idx..]
                            .iter()
                            .position(|c| *c == '\n')
                            .map(|x| idx + x)
                            .unwrap_or(chars.len());
                        samples.push(
                            chars[line_start..line_end]
                                .iter()
                                .collect::<String>()
                                .trim()
                                .to_string(),
                        );
                    }
                } else {
                    out.push(chars[idx]);
                    idx += 1;
                }
            }
        }
        Ok((out, count, samples))
    }
}

// ── Tauri command wrappers ────────────────────────────────────────────────

#[tauri::command]
pub async fn search_in_files(
    project_path: String,
    query: String,
    file_pattern: Option<String>,
    is_regex: Option<bool>,
    case_sensitive: Option<bool>,
    max_hits: Option<usize>,
) -> Result<SearchInFilesResult, String> {
    let root = PathBuf::from(&project_path);
    if !root.exists() {
        return Err(format!("Project path does not exist: {}", project_path));
    }
    let pattern = file_pattern.unwrap_or_default();
    let is_regex = is_regex.unwrap_or(false);
    let cs = case_sensitive.unwrap_or(true);
    let max = max_hits.unwrap_or(500);
    tokio::task::spawn_blocking(move || {
        search_in_files_impl(&root, &query, &pattern, is_regex, cs, max)
    })
    .await
    .map_err(|e| format!("Search task panicked: {}", e))?
}

#[tauri::command]
pub async fn replace_in_files(
    project_path: String,
    query: String,
    replacement: String,
    file_pattern: Option<String>,
    is_regex: Option<bool>,
    case_sensitive: Option<bool>,
    dry_run: Option<bool>,
) -> Result<ReplaceInFilesResult, String> {
    let root = PathBuf::from(&project_path);
    if !root.exists() {
        return Err(format!("Project path does not exist: {}", project_path));
    }
    let pattern = file_pattern.unwrap_or_default();
    let is_regex = is_regex.unwrap_or(false);
    let cs = case_sensitive.unwrap_or(true);
    let dry = dry_run.unwrap_or(true);
    tokio::task::spawn_blocking(move || {
        replace_in_files_impl(&root, &query, &replacement, &pattern, is_regex, cs, dry)
    })
    .await
    .map_err(|e| format!("Replace task panicked: {}", e))?
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("neocoder_sr_{}", name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (rel, content) in files {
            let path = dir.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, content).unwrap();
        }
        dir
    }

    #[test]
    fn literal_search_finds_all_lines() {
        let dir = temp_project(
            "lit",
            &[
                ("a.rs", "foo bar\nbaz foo\nnothing here\nfoo again"),
                ("b.rs", "foo"),
            ],
        );
        let r = search_in_files_impl(&dir, "foo", "*.rs", false, true, 100).unwrap();
        assert_eq!(r.total_matches, 4);
        assert_eq!(r.files_with_matches, 2);
        assert!(r.hits[0].preview.contains("foo"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn regex_search_case_insensitive() {
        let dir = temp_project("re", &[("a.rs", "Foo\nFOO\nbar\nfOO")]);
        let r = search_in_files_impl(&dir, "f[o]+", "*.rs", true, false, 100).unwrap();
        assert_eq!(r.total_matches, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn literal_replace_applies_dry_then_applied() {
        let dir = temp_project("rp", &[("x.rs", "aaa\nbbb\nccc\naaa")]);
        let preview = replace_in_files_impl(&dir, "aaa", "zzz", "*.rs", false, true, true).unwrap();
        assert_eq!(preview.total_replacements, 2);
        assert!(!preview.applied);
        // file unchanged
        let still = std::fs::read_to_string(dir.join("x.rs")).unwrap();
        assert!(still.contains("aaa"));
        let applied =
            replace_in_files_impl(&dir, "aaa", "zzz", "*.rs", false, true, false).unwrap();
        assert_eq!(applied.total_replacements, 2);
        assert!(applied.applied);
        let updated = std::fs::read_to_string(dir.join("x.rs")).unwrap();
        assert!(!updated.contains("aaa"));
        assert!(updated.contains("zzz"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn regex_replace_roundtrip() {
        let dir = temp_project("rx", &[("x.rs", "id=1\nid=2\nid=3")]);
        let r = replace_in_files_impl(&dir, r"id=\d+", "id=0", "*.rs", true, true, false).unwrap();
        assert_eq!(r.total_replacements, 3);
        let content = std::fs::read_to_string(dir.join("x.rs")).unwrap();
        assert_eq!(content.matches("id=0").count(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn case_insensitive_literal_replace_preserves_text_length() {
        let (out, count, _) =
            apply_replacements("Foo foo FOO bar", "foo", "X", false, false).unwrap();
        assert_eq!(count, 3);
        assert_eq!(out, "X X X bar");
    }

    #[test]
    fn skip_dirs_are_not_walked() {
        let dir = temp_project(
            "skip",
            &[
                ("a.rs", "secret_here"),
                ("target/b.rs", "secret_here"),
                ("node_modules/c.js", "secret_here"),
            ],
        );
        let r = search_in_files_impl(&dir, "secret", "", false, true, 100).unwrap();
        assert_eq!(
            r.files_with_matches, 1,
            "hits should be limited to top-level file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_query_errors() {
        let dir = temp_project("empty", &[("a.rs", "x")]);
        assert!(search_in_files_impl(&dir, "", "*.rs", false, true, 100).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncates_at_max_hits() {
        let mut many = String::new();
        for i in 0..200 {
            many.push_str(&format!("match {}\n", i));
        }
        let dir = temp_project("trunc", &[("a.rs", &many)]);
        let r = search_in_files_impl(&dir, "match", "*.rs", false, true, 50).unwrap();
        assert_eq!(r.hits.len(), 50);
        assert!(r.truncated);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_regex_reports_error() {
        let dir = temp_project("bad", &[("a.rs", "x")]);
        let err = search_in_files_impl(&dir, "(", "*.rs", true, true, 100).unwrap_err();
        assert!(err.contains("regex"), "{}", err);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
