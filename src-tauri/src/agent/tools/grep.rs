use std::path::PathBuf;
use crate::agent::utils::SKIP_DIRS;
use super::{Tool, ToolContext};

pub struct Grep;

#[async_trait::async_trait]
impl Tool for Grep {
    fn name(&self) -> &str {
        "grep"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let pattern = args["pattern"].as_str().unwrap_or("");
        let search_path = args["path"]
            .as_str()
            .or(ctx.project_path.as_deref())
            .unwrap_or(".");
        // Normalize Cygwin/MSYS2 paths on Windows
        let search_path_owned = crate::agent::utils::normalize_cygwin_path(search_path);
        let search_path = search_path_owned.as_str();
        let use_regex = args["regex"].as_bool().unwrap_or(false);
        let context_lines = args["context"].as_u64().unwrap_or(0) as usize;
        let file_pattern = args["file_pattern"].as_str().unwrap_or("");

        if pattern.is_empty() {
            return "Error: grep pattern is required".to_string();
        }

        // Build the matcher: regex or case-insensitive substring
        let matcher: Box<dyn Fn(&str) -> bool + Send> = if use_regex {
            match regex::RegexBuilder::new(pattern).case_insensitive(true).build() {
                Ok(re) => Box::new(move |line: &str| re.is_match(line)),
                Err(e) => return format!("Error: Invalid regex pattern '{}': {}", pattern, e),
            }
        } else {
            let lower = pattern.to_lowercase();
            Box::new(move |line: &str| line.to_lowercase().contains(&lower))
        };

        // Sandbox: check read access on search path
        let search_path_buf = std::path::PathBuf::from(search_path);
        if let Err(e) = ctx.sandbox.check_path(&search_path_buf, ctx.project_path.as_deref(), false) {
            return format!("Error: Sandbox blocked: {}", e);
        }

        let mut results: Vec<FileMatch> = Vec::new();
        let mut file_count = 0usize;
        let mut total_matches = 0usize;
        let max_results = 100usize;

        let mut dirs: Vec<PathBuf> = vec![search_path_buf];
        let skip_set: std::collections::HashSet<&str> = SKIP_DIRS.iter().copied().collect();

        while let Some(dir) = dirs.pop() {
            if total_matches >= max_results {
                break;
            }
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                if total_matches >= max_results {
                    break;
                }
                let path = entry.path();
                if path.is_dir() {
                    if let Some(name) = path.file_name() {
                        if !skip_set.contains(name.to_string_lossy().as_ref()) {
                            dirs.push(path);
                        }
                    }
                } else if path.is_file() {
                    // File pattern filter
                    if !file_pattern.is_empty() && !matches_file_pattern(&path, file_pattern) {
                        continue;
                    }

                    if let Ok(content) = std::fs::read_to_string(&path) {
                        let lines: Vec<&str> = content.lines().collect();
                        let mut match_ranges: Vec<(usize, usize)> = Vec::new();

                        for (line_num, line) in lines.iter().enumerate() {
                            if matcher(line) {
                                if total_matches >= max_results {
                                    break;
                                }
                                let start = line_num.saturating_sub(context_lines);
                                let end = (line_num + context_lines).min(lines.len().saturating_sub(1));
                                match_ranges.push((start, end));
                                total_matches += 1;
                            }
                        }

                        if !match_ranges.is_empty() {
                            let merged = merge_ranges(&match_ranges);
                            results.push(FileMatch {
                                path: path.clone(),
                                ranges: merged,
                                lines: lines.iter().map(|l| l.to_string()).collect(),
                            });
                            file_count += 1;
                        }
                    }
                }
            }
        }

        if results.is_empty() {
            format!("No matches found for pattern '{}' in {}", pattern, search_path)
        } else {
            let mut output = format!(
                "Grep results for '{}' ({} matches in {} files):\n\n",
                pattern, total_matches, file_count
            );
            for fm in &results {
                output.push_str(&format!("── {} ──\n", fm.path.display()));
                for (start, end) in &fm.ranges {
                    for i in *start..=*end {
                        let line = fm.lines.get(i).map(|s| s.as_str()).unwrap_or("");
                        output.push_str(&format!("  {:>4} │ {}\n", i + 1, line));
                    }
                    output.push_str("  ⋮\n");
                }
                output.push('\n');
            }
            if total_matches >= max_results {
                output.push_str(&format!("\n... truncated at {} matches\n", max_results));
            }
            output
        }
    }
}

struct FileMatch {
    path: PathBuf,
    ranges: Vec<(usize, usize)>,
    lines: Vec<String>,
}

/// Merge overlapping or adjacent line ranges
fn merge_ranges(ranges: &[(usize, usize)]) -> Vec<(usize, usize)> {
    if ranges.is_empty() {
        return vec![];
    }
    let mut sorted: Vec<(usize, usize)> = ranges.to_vec();
    sorted.sort_by_key(|r| r.0);

    let mut merged: Vec<(usize, usize)> = vec![sorted[0]];
    for &(start, end) in &sorted[1..] {
        let last = merged.last_mut().unwrap();
        if start <= last.1 + 1 {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
}

/// Check if a file path matches a glob-like pattern (e.g., "*.rs", "*.tsx")
fn matches_file_pattern(path: &std::path::Path, pattern: &str) -> bool {
    let file_name = path.file_name().and_then(|f| f.to_str()).unwrap_or("");

    // Support comma-separated patterns: "*.rs,*.toml"
    for pat in pattern.split(',') {
        let pat = pat.trim();
        if pat.starts_with('*') {
            let ext = &pat[1..]; // includes the dot
            if file_name.ends_with(ext) {
                return true;
            }
        } else if file_name == pat {
            return true;
        }
    }
    false
}
