//! Multi-file context collection for code completion.
//!
//! Scans related files in the same directory and extracts public symbols
//! (function signatures, class/trait/struct declarations) to provide
//! richer context for FIM completion.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

// ── Data Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedContext {
    pub files: Vec<RelatedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedFile {
    pub path: String,
    pub symbols: Vec<SymbolSignature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolSignature {
    pub name: String,
    pub kind: String,       // "function", "class", "trait", "struct", "interface", "type"
    pub signature: String,  // The actual signature line(s)
}

// ── Collection ────────────────────────────────────────────────────────────

/// Collect related file context for completion enhancement.
///
/// Strategy:
/// 1. Find files with the same extension in the same directory (up to `max_files`)
/// 2. Extract public symbols from each file (up to `max_symbols_per_file`)
/// 3. Return structured context for prompt injection
pub async fn collect_related_context(
    file_path: &str,
    project_path: Option<&str>,
    max_files: usize,
    max_symbols_per_file: usize,
) -> RelatedContext {
    let path = Path::new(file_path);
    let current_dir = match path.parent() {
        Some(d) => d.to_path_buf(),
        None => return RelatedContext { files: Vec::new() },
    };

    let current_ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let current_file_name = path.file_name().and_then(|f| f.to_str()).unwrap_or("");

    // Skip if no extension (e.g., README, Makefile)
    if current_ext.is_empty() {
        return RelatedContext { files: Vec::new() };
    }

    // Find related files
    let related_files = find_related_files(&current_dir, current_ext, current_file_name, max_files);

    // Extract symbols from each file
    let mut files = Vec::new();
    for file_path in related_files {
        let symbols = extract_symbols(&file_path, max_symbols_per_file).await;
        if !symbols.is_empty() {
            // Use relative path if project_path is provided
            let display_path = if let Some(proj) = project_path {
                file_path
                    .strip_prefix(proj)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| file_path.to_string_lossy().to_string())
            } else {
                file_path.to_string_lossy().to_string()
            };

            files.push(RelatedFile {
                path: display_path,
                symbols,
            });
        }
    }

    RelatedContext { files }
}

/// Find files with the same extension in the same directory.
fn find_related_files(dir: &Path, ext: &str, exclude_name: &str, max_files: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return files,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip directories and non-matching extensions
        if !path.is_file() {
            continue;
        }

        let entry_ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if entry_ext != ext {
            continue;
        }

        let file_name = path.file_name().and_then(|f| f.to_str()).unwrap_or("");

        // Skip the current file
        if file_name == exclude_name {
            continue;
        }

        // Skip hidden files and test files
        if file_name.starts_with('.') || file_name.contains("_test") || file_name.contains(".test.") {
            continue;
        }

        // Skip very large files (> 100KB)
        if let Ok(meta) = path.metadata() {
            if meta.len() > 100 * 1024 {
                continue;
            }
        }

        if seen.insert(path.clone()) {
            files.push(path);
            if files.len() >= max_files {
                break;
            }
        }
    }

    files
}

/// Extract public symbols from a file using regex-based parsing.
async fn extract_symbols(file_path: &Path, max_symbols: usize) -> Vec<SymbolSignature> {
    let content = match tokio::fs::read_to_string(file_path).await {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let mut symbols = Vec::new();

    // Language-specific regex patterns for public symbols
    let patterns: &[(&str, &str)] = match ext {
        "rs" => &[
            ("function", r"(?m)^pub\s+(?:async\s+)?fn\s+(\w+)"),
            ("struct", r"(?m)^pub\s+struct\s+(\w+)"),
            ("enum", r"(?m)^pub\s+enum\s+(\w+)"),
            ("trait", r"(?m)^pub\s+trait\s+(\w+)"),
            ("type", r"(?m)^pub\s+type\s+(\w+)"),
        ],
        "ts" | "tsx" => &[
            ("function", r"(?m)^export\s+(?:async\s+)?function\s+(\w+)"),
            ("function", r"(?m)^export\s+const\s+(\w+)\s*="),
            ("class", r"(?m)^export\s+class\s+(\w+)"),
            ("interface", r"(?m)^export\s+interface\s+(\w+)"),
            ("type", r"(?m)^export\s+type\s+(\w+)"),
            ("enum", r"(?m)^export\s+enum\s+(\w+)"),
        ],
        "js" | "jsx" => &[
            ("function", r"(?m)^export\s+(?:async\s+)?function\s+(\w+)"),
            ("function", r"(?m)^export\s+const\s+(\w+)\s*="),
            ("class", r"(?m)^export\s+class\s+(\w+)"),
        ],
        "py" => &[
            ("function", r"(?m)^def\s+(\w+)"),
            ("function", r"(?m)^async\s+def\s+(\w+)"),
            ("class", r"(?m)^class\s+(\w+)"),
        ],
        "go" => &[
            ("function", r"(?m)^func\s+(\w+)"),
            ("type", r"(?m)^type\s+(\w+)\s+struct"),
            ("type", r"(?m)^type\s+(\w+)\s+interface"),
        ],
        "java" | "kt" => &[
            ("function", r"(?m)^\s*public\s+(?:static\s+)?(?:\w+\s+)?(?:<\w+>\s+)?(\w+)\s*\("),
            ("class", r"(?m)^(?:public\s+)?(?:abstract\s+)?class\s+(\w+)"),
            ("interface", r"(?m)^(?:public\s+)?interface\s+(\w+)"),
        ],
        _ => &[],
    };

    if patterns.is_empty() {
        return symbols;
    }

    // Compile and apply each pattern
    for (kind, pattern) in patterns {
        if symbols.len() >= max_symbols {
            break;
        }

        if let Ok(re) = regex::Regex::new(pattern) {
            for cap in re.captures_iter(&content) {
                if symbols.len() >= max_symbols {
                    break;
                }

                if let Some(name_match) = cap.get(1) {
                    let name = name_match.as_str().to_string();

                    // Skip private/underscore-prefixed names
                    if name.starts_with('_') {
                        continue;
                    }

                    // Extract the full signature line
                    let full_match = cap.get(0).map(|m| m.as_str()).unwrap_or("");
                    let line_start = content[..full_match.as_ptr() as usize - content.as_ptr() as usize]
                        .rfind('\n')
                        .map(|i| i + 1)
                        .unwrap_or(0);

                    // Find the end of the signature (next { or end of line for type aliases)
                    let sig_end = content[full_match.as_ptr() as usize - content.as_ptr() as usize..]
                        .find(|c: char| c == '{' || c == ';')
                        .map(|i| full_match.as_ptr() as usize - content.as_ptr() as usize + i)
                        .unwrap_or(content.len());

                    let signature = content[line_start..sig_end].trim().to_string();

                    // Only include if signature is reasonable length
                    if signature.len() < 200 && !signature.is_empty() {
                        symbols.push(SymbolSignature {
                            name,
                            kind: kind.to_string(),
                            signature,
                        });
                    }
                }
            }
        }
    }

    symbols
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_find_related_files() {
        // Create temp directory with test files
        let temp_dir = std::env::temp_dir().join("neecoder_test_multi_file");
        let _ = std::fs::create_dir_all(&temp_dir);

        // Create test files
        let _ = std::fs::write(temp_dir.join("main.rs"), "fn main() {}");
        let _ = std::fs::write(temp_dir.join("utils.rs"), "pub fn helper() {}");
        let _ = std::fs::write(temp_dir.join("test.rs"), "mod test {}");

        let files = find_related_files(&temp_dir, "rs", "main.rs", 5);
        assert!(!files.is_empty());

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_extract_symbols_rust() {
        let temp_file = std::env::temp_dir().join("test_extract.rs");
        std::fs::write(
            &temp_file,
            r#"
pub fn hello() {
    println!("hello");
}

pub struct Config {
    pub name: String,
}

pub trait Processor {
    fn process(&self);
}

fn private_fn() {}
"#,
        )
        .unwrap();

        let symbols = extract_symbols(&temp_file, 10).await;
        assert!(symbols.len() >= 3);
        assert!(symbols.iter().any(|s| s.name == "hello"));
        assert!(symbols.iter().any(|s| s.name == "Config"));
        assert!(symbols.iter().any(|s| s.name == "Processor"));
        assert!(!symbols.iter().any(|s| s.name == "private_fn"));

        let _ = std::fs::remove_file(&temp_file);
    }
}
