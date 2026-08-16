//! Multi-file context collection for code completion.
//!
//! Two complementary strategies:
//! 1. Same-directory scan: extract public symbols (function signatures,
//!    class/trait/struct declarations) from sibling files.
//! 2. RAG search: query the local code index with identifiers extracted from
//!    the cursor context, returning semantically related chunks from anywhere
//!    in the project (cross-directory awareness).

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
    pub kind: String, // "function", "class", "trait", "struct", "interface", "type"
    pub signature: String, // The actual signature line(s)
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

/// Extract identifier-like tokens from the tail of the cursor prefix.
///
/// These become the RAG query: they name the symbols the user is most likely
/// referencing (function calls, types, variables) at the completion point.
pub fn extract_identifiers(prefix: &str, max: usize) -> Vec<String> {
    // Take only the last ~600 chars — identifiers far back are stale.
    let tail: String = prefix
        .chars()
        .rev()
        .take(600)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for token in tail.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let t = token.trim();
        if t.is_empty() || t.len() < 2 {
            continue;
        }
        // Skip language keywords and pure numbers
        if t.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if is_language_keyword(t) {
            continue;
        }
        if seen.insert(t.to_string()) {
            out.push(t.to_string());
            if out.len() >= max {
                break;
            }
        }
    }
    out
}

/// Common language keywords that carry no search signal.
fn is_language_keyword(t: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "fn",
        "pub",
        "use",
        "mod",
        "let",
        "mut",
        "impl",
        "struct",
        "enum",
        "trait",
        "match",
        "if",
        "else",
        "for",
        "while",
        "loop",
        "return",
        "async",
        "await",
        "const",
        "static",
        "type",
        "self",
        "super",
        "crate",
        "function",
        "class",
        "const",
        "var",
        "let",
        "new",
        "this",
        "import",
        "from",
        "export",
        "default",
        "def",
        "return",
        "None",
        "True",
        "False",
        "and",
        "or",
        "not",
        "in",
        "is",
        "with",
        "as",
        "try",
        "except",
        "func",
        "package",
        "interface",
        "select",
        "go",
        "defer",
        "chan",
    ];
    KEYWORDS.contains(&t)
}

/// RAG-based related context: query the code index with the cursor's
/// identifier context and return the most relevant chunks (cross-directory).
///
/// Falls back gracefully to an empty context when the index is empty or the
/// search fails — completion must never block on indexing issues.
pub async fn collect_rag_context(
    prefix: &str,
    indexer: &crate::rag::CodeIndexer,
    project_path: Option<&str>,
    max_chunks: usize,
) -> RelatedContext {
    let identifiers = extract_identifiers(prefix, 6);
    if identifiers.is_empty() {
        return RelatedContext { files: Vec::new() };
    }
    let query = identifiers.join(" ");

    let results = match indexer.hybrid_search(&query, max_chunks).await {
        Ok(r) => r,
        Err(_) => return RelatedContext { files: Vec::new() },
    };
    if results.is_empty() {
        return RelatedContext { files: Vec::new() };
    }

    // Deduplicate by file path, keep the strongest chunk per file
    let mut by_file: std::collections::HashMap<String, (f32, &crate::rag::CodeChunk)> =
        std::collections::HashMap::new();
    for r in &results {
        let chunk = &r.chunk;
        match by_file.get(&chunk.file_path) {
            Some((best, _)) if *best >= r.score => continue,
            _ => {
                by_file.insert(chunk.file_path.clone(), (r.score, chunk));
            }
        }
    }

    let mut files = Vec::new();
    for (file_path, (_score, chunk)) in by_file {
        // Reuse SymbolSignature to carry chunk content — name = chunk summary
        // (or "<first symbol>"), kind = chunk_type, signature = content body.
        let name = if chunk.summary.trim().is_empty() {
            format!("chunk:{}-{}", chunk.start_line, chunk.end_line)
        } else {
            chunk.summary.trim().to_string()
        };
        let content_preview: String = chunk.content.chars().take(400).collect();
        let display_path = if let Some(proj) = project_path {
            Path::new(&file_path)
                .strip_prefix(proj)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| file_path.clone())
        } else {
            file_path.clone()
        };
        files.push(RelatedFile {
            path: display_path,
            symbols: vec![SymbolSignature {
                name,
                kind: format!("{:?}", chunk.chunk_type).to_lowercase(),
                signature: content_preview,
            }],
        });
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
        if file_name.starts_with('.') || file_name.contains("_test") || file_name.contains(".test.")
        {
            continue;
        }

        // Skip very large files (> 100KB)
        if let Ok(meta) = path.metadata()
            && meta.len() > 100 * 1024
        {
            continue;
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
            (
                "function",
                r"(?m)^\s*public\s+(?:static\s+)?(?:\w+\s+)?(?:<\w+>\s+)?(\w+)\s*\(",
            ),
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
                    let line_start = content
                        [..full_match.as_ptr() as usize - content.as_ptr() as usize]
                        .rfind('\n')
                        .map(|i| i + 1)
                        .unwrap_or(0);

                    // Find the end of the signature (next { or end of line for type aliases)
                    let sig_end = content
                        [full_match.as_ptr() as usize - content.as_ptr() as usize..]
                        .find(['{', ';'])
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
        let temp_dir = std::env::temp_dir().join("neocoder_test_multi_file");
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

    #[test]
    fn test_extract_identifiers_basic() {
        let prefix = "let result = computeTotal(user, order);\nif (result > threshold) {";
        let ids = extract_identifiers(prefix, 10);
        assert!(ids.contains(&"computeTotal".to_string()));
        assert!(ids.contains(&"user".to_string()));
        assert!(ids.contains(&"order".to_string()));
        assert!(ids.contains(&"result".to_string()));
        assert!(ids.contains(&"threshold".to_string()));
        // No keywords, no single chars
        assert!(!ids.iter().any(|i| i == "let" || i == "if" || i == "fn"));
        assert!(!ids.iter().any(|i| i.len() < 2));
    }

    #[test]
    fn test_extract_identifiers_dedup_and_limit() {
        let prefix = "foo() foo() foo() bar() baz()";
        let ids = extract_identifiers(prefix, 3);
        assert_eq!(ids, vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn test_extract_identifiers_empty_and_keywords() {
        assert!(
            extract_identifiers("fn main() { let mut x = 1; }", 10).is_empty()
                || !extract_identifiers("fn main() { let mut x = 1; }", 10)
                    .iter()
                    .any(|i| i == "fn")
        );
        assert!(extract_identifiers("if for while return", 10).is_empty());
        assert!(extract_identifiers("123 456", 10).is_empty());
        assert!(extract_identifiers("", 10).is_empty());
    }

    #[tokio::test]
    async fn test_collect_rag_context_empty_index() {
        // Empty index → graceful empty context (no panic, no error)
        let indexer = crate::rag::CodeIndexer::new(
            crate::config::LlmProvider::OpenAI,
            "test-key".to_string(),
            None,
            "test-model".to_string(),
        );
        let ctx = collect_rag_context("processOrder(userId)", &indexer, None, 3).await;
        assert!(ctx.files.is_empty());
    }
}
