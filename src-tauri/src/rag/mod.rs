use crate::config::LlmProvider;
use crate::llm;
use crate::lsp;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

// ── Public Data Types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeChunk {
    pub id: String,
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub language: String,
    pub chunk_type: ChunkType,
    pub content: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChunkType {
    File,
    Function,
    Class,
    Module,
    Block,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub chunk: CodeChunk,
    pub score: f32,
}

/// Build a context block from code chunks for RAG-enhanced prompts
pub fn build_rag_context(chunks: &[CodeChunk], max_chunks: usize) -> String {
    let mut context = String::from("--- Relevant Code Context ---\n");
    let count = chunks.len().min(max_chunks);

    for chunk in chunks.iter().take(count) {
        context.push_str(&format!(
            "File: {} (lines {}-{})\n```{}\n{}\n```\n\n",
            chunk.file_path,
            chunk.start_line,
            chunk.end_line,
            chunk.language.to_lowercase(),
            chunk.content
        ));
    }

    context.push_str("--- End Context ---\n");
    context
}

// ── Indexed Chunk (with embedding) ─────────────────────────────────────────

#[derive(Debug, Clone)]
struct IndexedChunk {
    chunk: CodeChunk,
    embedding: Vec<f32>,
}

// ── Code Indexer ───────────────────────────────────────────────────────────

pub struct CodeIndexer {
    chunks: Arc<RwLock<Vec<IndexedChunk>>>,
    file_map: Arc<RwLock<HashMap<String, Vec<usize>>>>, // file_path -> indices in chunks
    provider: LlmProvider,
    api_key: String,
    base_url: Option<String>,
    embed_model: String,
    /// Supported file extensions for indexing
    supported_extensions: Vec<String>,
}

impl CodeIndexer {
    pub fn new(provider: LlmProvider, api_key: String, base_url: Option<String>, embed_model: String) -> Self {
        CodeIndexer {
            chunks: Arc::new(RwLock::new(Vec::new())),
            file_map: Arc::new(RwLock::new(HashMap::new())),
            provider,
            api_key,
            base_url,
            embed_model,
            supported_extensions: vec![
                "rs".into(), "go".into(), "py".into(), "js".into(), "ts".into(),
                "jsx".into(), "tsx".into(), "java".into(), "rb".into(), "php".into(),
                "c".into(), "h".into(), "cpp".into(), "hpp".into(), "cs".into(),
                "swift".into(), "kt".into(), "scala".into(), "sql".into(), "sh".into(),
                "html".into(), "css".into(), "json".into(), "yaml".into(), "toml".into(),
                "md".into(),
            ],
        }
    }

    /// Set supported file extensions.
    pub fn with_extensions(mut self, exts: Vec<String>) -> Self {
        self.supported_extensions = exts;
        self
    }

    /// Index a single file: chunk it, generate embedding, and store.
    pub async fn index_file(&self, file_path: &str, content: &str) -> Result<usize, String> {
        let chunks = chunk_code(file_path, content);
        if chunks.is_empty() {
            return Ok(0);
        }

        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();

        // Generate embeddings
        let embeddings = llm::embed_texts(
            &self.provider,
            &self.api_key,
            self.base_url.as_deref(),
            &self.embed_model,
            &texts,
        )
        .await?;

        if embeddings.len() != chunks.len() {
            return Err(format!(
                "Embedding count mismatch: got {}, expected {}",
                embeddings.len(),
                chunks.len()
            ));
        }

        // Remove old entries for this file
        self.remove_file(file_path).await;

        // Store new entries
        let mut chunks_lock = self.chunks.write().await;
        let mut file_map_lock = self.file_map.write().await;
        let mut indices = Vec::new();

        for (i, chunk) in chunks.into_iter().enumerate() {
            let idx = chunks_lock.len();
            chunks_lock.push(IndexedChunk {
                chunk,
                embedding: embeddings[i].clone(),
            });
            indices.push(idx);
        }

        file_map_lock.insert(file_path.to_string(), indices);

        log::info!("Indexed {} chunks from {}", chunks_lock.len(), file_path);
        Ok(chunks_lock.len())
    }

    /// Index all files in a directory recursively.
    pub async fn index_project(&self, project_path: &str) -> Result<(usize, usize), String> {
        let path = Path::new(project_path);
        if !path.is_dir() {
            return Err(format!("Not a directory: {}", project_path));
        }

        let mut total_files = 0usize;
        let mut total_chunks = 0usize;

        let mut entries: Vec<_> = Vec::new();
        collect_files(path, &mut entries, &self.supported_extensions);

        for file_path in &entries {
            let content = match std::fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            match self.index_file(&file_path.to_string_lossy(), &content).await {
                Ok(count) => {
                    total_files += 1;
                    total_chunks += count;
                }
                Err(e) => {
                    log::warn!("Failed to index {}: {}", file_path.display(), e);
                }
            }
        }

        log::info!(
            "Indexed project: {} files, {} chunks total",
            total_files,
            total_chunks
        );
        Ok((total_files, total_chunks))
    }

    /// Remove all indexed chunks for a file.
    pub async fn remove_file(&self, file_path: &str) {
        let mut file_map_lock = self.file_map.write().await;
        if let Some(indices) = file_map_lock.remove(file_path) {
            let mut chunks_lock = self.chunks.write().await;
            // Mark removed indices by setting them to None (we use a sentinel approach)
            // Since we're using Vec, we rebuild the Vec without the removed indices
            let mut new_chunks: Vec<IndexedChunk> = Vec::with_capacity(chunks_lock.len());
            let removed_set: HashSet<usize> =
                indices.into_iter().collect();
            let mut new_index_map: HashMap<String, Vec<usize>> = HashMap::new();

            let old_chunks = std::mem::take(&mut *chunks_lock);
            for (old_idx, chunk) in old_chunks.into_iter().enumerate() {
                if removed_set.contains(&old_idx) {
                    continue; // Skip removed
                }
                let new_idx = new_chunks.len();
                new_chunks.push(chunk);

                // Update file_map for all files
                let fp = new_chunks[new_idx].chunk.file_path.clone();
                new_index_map.entry(fp).or_default().push(new_idx);
            }

            *chunks_lock = new_chunks;

            // Rebuild the full file_map (other entries that weren't touched)
            for (fp, _) in file_map_lock.drain() {
                if fp != file_path {
                    // These entries were stored in new_index_map already
                }
            }
            *file_map_lock = new_index_map;
        }
    }

    /// Handle file change event: re-index on create/modify, remove on delete.
    /// Called by the background auto-reindex loop in lib.rs for incremental RAG updates.
    /// Returns `true` if the index was actually modified.
    pub async fn handle_file_change(&self, path: &Path, kind: crate::fs_watcher::FileChangeKind) -> bool {
        let path_str = path.to_string_lossy().to_string();

        // Check if file extension is supported
        let ext = path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase());
        let is_supported = ext.as_ref().map(|e| self.supported_extensions.contains(e)).unwrap_or(false);

        if !is_supported {
            return false;
        }

        match kind {
            crate::fs_watcher::FileChangeKind::Deleted => {
                self.remove_file(&path_str).await;
                log::info!("[RAG] Removed indexed chunks for deleted file: {}", path_str);
                true
            }
            crate::fs_watcher::FileChangeKind::Created | crate::fs_watcher::FileChangeKind::Modified => {
                match tokio::fs::read_to_string(path).await {
                    Ok(content) => {
                        match self.index_file(&path_str, &content).await {
                            Ok(count) => {
                                log::info!("[RAG] Re-indexed {} chunks for file: {}", count, path_str);
                                true
                            }
                            Err(e) => {
                                log::warn!("[RAG] Failed to index file {}: {}", path_str, e);
                                false
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("[RAG] Failed to read file {}: {}", path_str, e);
                        false
                    }
                }
            }
        }
    }

    /// Search for code chunks similar to the query (vector similarity).
    pub async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>, String> {
        let chunks_lock = self.chunks.read().await;
        if chunks_lock.is_empty() {
            return Ok(vec![]);
        }

        let trimmed = query.trim();
        if trimmed.is_empty() || trimmed.len() < 2 {
            return Ok(vec![]);
        }

        // Truncate to prevent token limit issues
        let safe_query = if trimmed.len() > 500 { crate::agent::utils::safe_truncate(trimmed, 500) } else { trimmed };
        let query_texts = vec![safe_query.to_string()];
        let query_embeddings = llm::embed_texts(
            &self.provider,
            &self.api_key,
            self.base_url.as_deref(),
            &self.embed_model,
            &query_texts,
        )
        .await?;

        if query_embeddings.is_empty() {
            return Ok(vec![]);
        }

        let query_embedding = &query_embeddings[0];

        // Compute cosine similarity with all chunks
        let mut scored: Vec<(f32, &IndexedChunk)> = chunks_lock
            .iter()
            .map(|ic| (cosine_similarity(query_embedding, &ic.embedding), ic))
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // Take top results
        Ok(scored
            .into_iter()
            .take(max_results)
            .filter(|(score, _)| *score > 0.3)
            .map(|(score, ic)| SearchResult {
                chunk: ic.chunk.clone(),
                score,
            })
            .collect())
    }

    /// BM25 keyword-based search (no embedding needed).
    /// Returns scored results using BM25 ranking.
    pub async fn bm25_search(&self, query: &str, max_results: usize) -> Vec<SearchResult> {
        let chunks = self.chunks.read().await;
        if chunks.is_empty() { return vec![]; }

        let trimmed = query.trim();
        if trimmed.is_empty() || trimmed.len() < 2 { return vec![]; }
        let safe_query = if trimmed.len() > 500 { crate::agent::utils::safe_truncate(trimmed, 500) } else { trimmed };

        let query_terms: Vec<String> = tokenize(safe_query);
        if query_terms.is_empty() { return vec![]; }

        let n = chunks.len() as f32;
        let avg_dl: f32 = chunks.iter().map(|c| c.chunk.content.len() as f32).sum::<f32>() / n.max(1.0);

        // Precompute document frequency for each query term (one pass)
        let query_terms_lower: Vec<String> = query_terms.iter().map(|t| t.to_lowercase()).collect();
        let mut df_map: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
        for ic in chunks.iter() {
            let lower = ic.chunk.content.to_lowercase();
            for qt in &query_terms_lower {
                if lower.contains(qt.as_str()) {
                    *df_map.entry(qt.clone()).or_insert(0.0) += 1.0;
                }
            }
        }

        let k1 = 1.5f32;
        let b = 0.75f32;

        let mut scored: Vec<(f32, &IndexedChunk)> = chunks
            .iter()
            .map(|ic| {
                let doc = &ic.chunk.content;
                let dl = doc.len() as f32;
                let tf_map = term_frequencies(doc);

                let score: f32 = query_terms_lower
                    .iter()
                    .map(|qt| {
                        let tf = tf_map.get(qt).copied().unwrap_or(0.0);
                        let df = df_map.get(qt).copied().unwrap_or(0.0);
                        if df == 0.0 { return 0.0; }
                        let idf = ((n - df + 0.5) / (df + 0.5)).ln_1p();
                        (tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * dl / avg_dl)) * idf
                    })
                    .sum();

                (score, ic)
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        scored
            .into_iter()
            .take(max_results)
            .filter(|(s, _)| *s > 0.0)
            .map(|(score, ic)| SearchResult {
                chunk: ic.chunk.clone(),
                score: score.min(1.0),
            })
            .collect()
    }

    /// Hybrid search: vector + BM25, merged via RRF (Reciprocal Rank Fusion).
    pub async fn hybrid_search(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>, String> {
        let trimmed = query.trim();
        if trimmed.is_empty() || trimmed.len() < 2 {
            return Ok(vec![]);
        }

        let k = max_results * 2; // fetch more for better fusion

        let (vec_results, bm25_results) = tokio::join!(
            self.search(query, k),
            async { self.bm25_search(query, k).await },
        );

        let vec_results = vec_results.unwrap_or_default();

        // Reciprocal Rank Fusion
        let rrf_k = 60f32;
        let mut merged: std::collections::HashMap<String, (f32, &CodeChunk)> = std::collections::HashMap::new();

        for (rank, sr) in vec_results.iter().enumerate() {
            let key = &sr.chunk.id;
            let rrf_score = 1.0 / (rrf_k + rank as f32 + 1.0);
            merged.entry(key.clone()).or_insert_with(|| (0.0, &sr.chunk)).0 += rrf_score;
        }

        for (rank, sr) in bm25_results.iter().enumerate() {
            let key = &sr.chunk.id;
            let rrf_score = 1.0 / (rrf_k + rank as f32 + 1.0);
            merged.entry(key.clone()).or_insert_with(|| (0.0, &sr.chunk)).0 += rrf_score;
        }

        let mut results: Vec<SearchResult> = merged
            .into_values()
            .map(|(score, chunk)| SearchResult {
                chunk: chunk.clone(),
                score,
            })
            .collect();

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(max_results);
        Ok(results)
    }

    /// Save indexed chunks to SQLite database.
    pub async fn save_to_db(&self, db_path: &str) -> Result<usize, String> {
        let conn = rusqlite::Connection::open(db_path)
            .map_err(|e| format!("Failed to open DB: {}", e))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS chunks (
                id TEXT PRIMARY KEY,
                file_path TEXT NOT NULL,
                start_line INTEGER,
                end_line INTEGER,
                language TEXT,
                chunk_type TEXT,
                content TEXT,
                summary TEXT,
                embedding BLOB
            );
            DELETE FROM chunks;",
        )
        .map_err(|e| format!("DB init: {}", e))?;

        let chunks = self.chunks.read().await;
        let count = chunks.len();

        let mut stmt = conn
            .prepare("INSERT INTO chunks VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)")
            .map_err(|e| format!("Prepare: {}", e))?;

        for ic in chunks.iter() {
            let embedding_bytes: Vec<u8> = ic.embedding
                .iter()
                .flat_map(|f| f.to_le_bytes())
                .collect();
            let ctype = format!("{:?}", ic.chunk.chunk_type);
            stmt.execute(rusqlite::params![
                ic.chunk.id,
                ic.chunk.file_path,
                ic.chunk.start_line as i64,
                ic.chunk.end_line as i64,
                ic.chunk.language,
                ctype,
                ic.chunk.content,
                ic.chunk.summary,
                embedding_bytes,
            ])
            .map_err(|e| format!("Insert: {}", e))?;
        }

        log::info!("Saved {} chunks to {}", count, db_path);
        Ok(count)
    }

    /// Load indexed chunks from SQLite database.
    pub async fn load_from_db(&self, db_path: &str) -> Result<usize, String> {
        let conn = rusqlite::Connection::open(db_path)
            .map_err(|e| format!("Failed to open DB: {}", e))?;

        let mut stmt = conn
            .prepare("SELECT id, file_path, start_line, end_line, language, chunk_type, content, summary, embedding FROM chunks")
            .map_err(|e| format!("Prepare: {}", e))?;

        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let file_path: String = row.get(1)?;
                let start_line: i64 = row.get(2)?;
                let end_line: i64 = row.get(3)?;
                let language: String = row.get(4)?;
                let chunk_type_str: String = row.get(5)?;
                let content: String = row.get(6)?;
                let summary: String = row.get(7)?;
                let embedding_blob: Vec<u8> = row.get(8)?;

                let chunk_type = match chunk_type_str.as_str() {
                    "File" => ChunkType::File,
                    "Function" => ChunkType::Function,
                    "Class" => ChunkType::Class,
                    "Module" => ChunkType::Module,
                    _ => ChunkType::Block,
                };

                let embedding: Vec<f32> = embedding_blob
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();

                Ok((id, file_path, start_line, end_line, language, chunk_type, content, summary, embedding))
            })
            .map_err(|e| format!("Query: {}", e))?;

        let mut chunks = self.chunks.write().await;
        let mut file_map = self.file_map.write().await;
        chunks.clear();
        file_map.clear();

        let mut count = 0usize;
        for row in rows {
            let (id, file_path, start_line, end_line, language, chunk_type, content, summary, embedding) =
                row.map_err(|e| format!("Row: {}", e))?;
            let idx = chunks.len();
            chunks.push(IndexedChunk {
                chunk: CodeChunk {
                    id,
                    file_path: file_path.clone(),
                    start_line: start_line as usize,
                    end_line: end_line as usize,
                    language,
                    chunk_type,
                    content,
                    summary,
                },
                embedding,
            });
            file_map.entry(file_path).or_default().push(idx);
            count += 1;
        }

        log::info!("Loaded {} chunks from {}", count, db_path);
        Ok(count)
    }

    /// Get total number of indexed chunks.
    pub async fn chunk_count(&self) -> usize {
        self.chunks.read().await.len()
    }

    /// Clear all indexed data.
    pub async fn clear(&self) {
        self.chunks.write().await.clear();
        self.file_map.write().await.clear();
        log::info!("Index cleared");
    }
}

// ── Cosine Similarity ──────────────────────────────────────────────────────

/// Tokenize text into lowercase words (alphanumeric only, min 2 chars).
pub(super) fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() >= 2)
        .map(|s| s.to_string())
        .collect()
}

/// Compute term frequencies for a document.
pub(super) fn term_frequencies(doc: &str) -> std::collections::HashMap<String, f32> {
    let tokens = tokenize(doc);
    let total = tokens.len() as f32;
    if total == 0.0 { return std::collections::HashMap::new(); }
    let mut tf: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
    for t in tokens {
        *tf.entry(t).or_default() += 1.0;
    }
    for v in tf.values_mut() {
        *v /= total;
    }
    tf
}

pub(super) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot / (norm_a * norm_b)
}

// ── Code Chunking ──────────────────────────────────────────────────────────

/// Chunk source code into meaningful pieces (file-level, function, class).
fn chunk_code(file_path: &str, content: &str) -> Vec<CodeChunk> {
    let mut chunks = Vec::new();
    let language = lsp::detect_language(file_path);
    let id_prefix = uuid::Uuid::new_v4().to_string();
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // Add file-level chunk if the file is small
    if total_lines <= 80 {
        chunks.push(CodeChunk {
            id: format!("{}-file", id_prefix),
            file_path: file_path.to_string(),
            start_line: 1,
            end_line: total_lines.max(1),
            language: language.clone(),
            chunk_type: ChunkType::File,
            content: content.to_string(),
            summary: format!("{} file", file_path),
        });
        return chunks;
    }

    // Try to find ALL functions and classes (not just the first one)
    let mut fn_starts: Vec<usize> = Vec::new();
    let mut class_starts: Vec<usize> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if is_function_start(trimmed, &language) {
            fn_starts.push(i);
        }
        if is_class_start(trimmed, &language) {
            class_starts.push(i);
        }
    }

    // If we found structural elements, chunk around them
    let mut chunk_ranges: Vec<(usize, usize, ChunkType, String)> = Vec::new();

    for cls in &class_starts {
        let end = find_block_end(&lines, *cls, &language).unwrap_or(total_lines - 1);
        let summary = lines[*cls].trim().to_string();
        chunk_ranges.push((*cls, end, ChunkType::Class, summary));
    }

    for fn_s in &fn_starts {
        let end = find_block_end(&lines, *fn_s, &language).unwrap_or(total_lines - 1);
        let summary = lines[*fn_s].trim().to_string();
        chunk_ranges.push((*fn_s, end, ChunkType::Function, summary));
    }

    // If we have structural chunks, use them; otherwise, fall back to sliding window
    if !chunk_ranges.is_empty() {
        // Sort by start line
        chunk_ranges.sort_by_key(|r| r.0);

        // Remove overlapping ranges (keep the one that starts later if overlap)
        let mut filtered: Vec<(usize, usize, ChunkType, String)> = Vec::new();
        for range in chunk_ranges {
            if let Some(last) = filtered.last() {
                if range.0 < last.1 {
                    continue; // Overlap, skip
                }
            }
            filtered.push(range);
        }

        for (start, end, ctype, summary) in filtered {
            if start <= end && end < total_lines {
                let chunk_content = lines[start..=end].join("\n");
                chunks.push(CodeChunk {
                    id: format!("{}-{}-{}", id_prefix, chunks.len(), ctype.clone() as u8),
                    file_path: file_path.to_string(),
                    start_line: start + 1,
                    end_line: end + 1,
                    language: language.clone(),
                    chunk_type: ctype,
                    content: chunk_content,
                    summary,
                });
            }
        }
    } else {
        // Fallback: sliding window of ~60 lines
        let window_size = 60usize;
        let overlap = 15usize;
        let mut start = 0usize;

        while start < total_lines {
            let end = (start + window_size).min(total_lines) - 1;
            let chunk_content = lines[start..=end].join("\n");
            chunks.push(CodeChunk {
                id: format!("{}-block-{}", id_prefix, chunks.len()),
                file_path: file_path.to_string(),
                start_line: start + 1,
                end_line: end + 1,
                language: language.clone(),
                chunk_type: ChunkType::Block,
                content: chunk_content,
                summary: format!("Lines {}-{}", start + 1, end + 1),
            });
            if end >= total_lines - 1 {
                break;
            }
            start += window_size - overlap;
        }
    }

    chunks
}

pub(super) fn is_function_start(line: &str, _language: &str) -> bool {
    let line_lower = line.to_lowercase();
    // Language-agnostic heuristics
    (line_lower.starts_with("fn ")
        || line_lower.starts_with("def ")
        || line_lower.starts_with("function ")
        || line_lower.starts_with("func ")
        || line_lower.starts_with("pub fn ")
        || line_lower.starts_with("pub async fn ")
        || line_lower.starts_with("async fn ")
        || line_lower.starts_with("private ")
        || line_lower.starts_with("public ")
        || line_lower.starts_with("protected "))
        && line.contains('(')
}

pub(super) fn is_class_start(line: &str, _language: &str) -> bool {
    let line_lower = line.to_lowercase();
    line_lower.starts_with("class ")
        || line_lower.starts_with("struct ")
        || line_lower.starts_with("enum ")
        || line_lower.starts_with("interface ")
        || line_lower.starts_with("pub struct ")
        || line_lower.starts_with("pub enum ")
        || line_lower.starts_with("pub trait ")
        || line_lower.starts_with("trait ")
        || line_lower.starts_with("impl ")
        || line_lower.starts_with("pub impl ")
        || line_lower.starts_with("export ")
        || line_lower.starts_with("module ")
        || line_lower.starts_with("type ")
}

/// Find the end of a block by matching braces (simplified).
pub(super) fn find_block_end(lines: &[&str], start: usize, _language: &str) -> Option<usize> {
    let mut brace_depth = 0i32;
    let mut found_open = false;

    for i in start..lines.len() {
        for c in lines[i].chars() {
            match c {
                '{' => {
                    brace_depth += 1;
                    found_open = true;
                }
                '}' => {
                    brace_depth -= 1;
                }
                _ => {}
            }
        }
        if found_open && brace_depth <= 0 {
            return Some(i);
        }
    }

    None
}

/// Recursively collect source files from a directory.
fn collect_files(dir: &Path, files: &mut Vec<PathBuf>, extensions: &[String]) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            // Skip common non-source directories
            if !name.starts_with('.')
                && name != "node_modules"
                && name != "target"
                && name != "dist"
                && name != "build"
                && name != ".next"
                && name != "__pycache__"
            {
                collect_files(&path, files, extensions);
            }
        } else if path.is_file() {
            if let Some(ext) = path.extension() {
                if extensions.contains(&ext.to_string_lossy().to_lowercase()) {
                    files.push(path);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
