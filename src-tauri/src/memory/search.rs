use std::path::Path;
use std::fs;
use std::collections::HashMap;

/// Tokenize text into lowercase alphanumeric words (min 2 chars).
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() >= 2)
        .map(|s| s.to_string())
        .collect()
}

/// Compute normalized term frequencies for a document.
fn term_frequencies(doc: &str) -> HashMap<String, f32> {
    let tokens = tokenize(doc);
    let total = tokens.len() as f32;
    if total == 0.0 {
        return HashMap::new();
    }
    let mut tf: HashMap<String, f32> = HashMap::new();
    for t in tokens {
        *tf.entry(t).or_default() += 1.0;
    }
    for v in tf.values_mut() {
        *v /= total;
    }
    tf
}

/// BM25-based memory search (keyword relevance ranking).
pub struct MemorySearch {
    memory_dir: std::path::PathBuf,
}

#[derive(Debug, Clone)]
pub struct MemSearchResult {
    pub file_path: String,
    pub line_number: usize,
    pub line_content: String,
    pub relevance: f32,
}

impl MemorySearch {
    pub fn new(base_dir: &Path) -> Self {
        Self {
            memory_dir: base_dir.to_path_buf(),
        }
    }

    /// Search all `.md` files in the memory directory using BM25 ranking.
    /// Returns results sorted by BM25 relevance score descending.
    pub fn search(&self, query: &str, max_results: usize) -> Result<Vec<MemSearchResult>, String> {
        if !self.memory_dir.exists() {
            return Ok(Vec::new());
        }

        let trimmed = query.trim();
        if trimmed.is_empty() || trimmed.len() < 2 {
            return Ok(Vec::new());
        }

        // Collect all .md files and their content
        let mut documents: Vec<(String, String)> = Vec::new(); // (relative_path, content)
        self.collect_docs(&self.memory_dir, &mut documents)
            .map_err(|e| format!("Failed to search memory: {}", e))?;

        if documents.is_empty() {
            return Ok(Vec::new());
        }

        // BM25 parameters
        let k1 = 1.5f32;
        let b = 0.75f32;
        let query_terms = tokenize(trimmed);
        if query_terms.is_empty() {
            return Ok(Vec::new());
        }

        let n = documents.len() as f32;

        // Compute average document length (in terms)
        let avg_dl: f32 = documents.iter()
            .map(|(_, content)| tokenize(content).len() as f32)
            .sum::<f32>() / n.max(1.0);

        // Compute document frequency for each query term
        let mut df_map: HashMap<String, f32> = HashMap::new();
        for (_, content) in &documents {
            let lower = content.to_lowercase();
            for qt in &query_terms {
                if lower.contains(qt.as_str()) {
                    *df_map.entry(qt.clone()).or_insert(0.0) += 1.0;
                }
            }
        }

        // Score each document with BM25, then extract matching lines
        let mut results = Vec::new();

        for (file_path, content) in &documents {
            let tf_map = term_frequencies(content);
            let dl = tokenize(content).len() as f32;

            // Compute BM25 score for the whole document
            let doc_score: f32 = query_terms.iter()
                .map(|qt| {
                    let tf = tf_map.get(qt).copied().unwrap_or(0.0);
                    let df = df_map.get(qt).copied().unwrap_or(0.0);
                    if df == 0.0 {
                        return 0.0;
                    }
                    let idf = ((n - df + 0.5) / (df + 0.5)).ln_1p();
                    (tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * dl / avg_dl)) * idf
                })
                .sum();

            if doc_score <= 0.0 {
                continue;
            }

            // Find matching lines with their own BM25-like scores
            let body = if let Some(end) = content.find("\n---") {
                &content[end + 4..]
            } else {
                content.as_str()
            };

            for (i, line) in body.lines().enumerate() {
                let line_lower = line.to_lowercase();
                let mut line_score: f32 = 0.0;
                for qt in &query_terms {
                    if line_lower.contains(qt.as_str()) {
                        // Line-level scoring: full match >> partial
                        let line_tokens = tokenize(line);
                        let line_tf = line_tokens.iter().filter(|t| t.as_str() == qt.as_str()).count() as f32;
                        let line_dl = line_tokens.len() as f32;
                        let df = df_map.get(qt).copied().unwrap_or(1.0);
                        let idf = ((n - df + 0.5) / (df + 0.5)).ln_1p();
                        // Shorter lines with more matches get higher scores
                        let local_score = if line_dl > 0.0 {
                            (line_tf * (k1 + 1.0)) / (line_tf + k1 * (1.0 - b + b * line_dl / avg_dl.max(1.0))) * idf
                        } else {
                            idf
                        };
                        line_score = line_score.max(local_score);
                    }
                }
                if line_score > 0.0 {
                    results.push(MemSearchResult {
                        file_path: file_path.clone(),
                        line_number: i + 1,
                        line_content: line.to_string(),
                        relevance: line_score.min(1.0),
                    });
                }
            }
        }

        // Sort by relevance descending, take top N
        results.sort_by(|a, b| b.relevance.partial_cmp(&a.relevance).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(max_results);
        Ok(results)
    }

    /// Recursively collect .md files (skipping messages/ subdirectories).
    fn collect_docs(&self, dir: &Path, docs: &mut Vec<(String, String)>) -> Result<(), String> {
        let entries = fs::read_dir(dir)
            .map_err(|e| format!("Failed to read dir {}: {}", dir.display(), e))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) == Some("messages") {
                    continue; // Skip message files
                }
                self.collect_docs(&path, docs)?;
            } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
                let content = fs::read_to_string(&path)
                    .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
                let relative = path.strip_prefix(&self.memory_dir)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| path.to_string_lossy().to_string());
                docs.push((relative, content));
            }
        }
        Ok(())
    }
}
