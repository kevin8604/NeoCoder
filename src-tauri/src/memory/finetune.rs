//! Fine-tune data pipeline: MEMORY.md entries → JSONL training dataset.
//!
//! The dataset uses the chat format (`messages` array) accepted by mainstream
//! fine-tuning frameworks (Llama-Factory, axolotl, etc.). Each memory entry is
//! converted into a question/answer pair where the model learns to recall and
//! phrase the user's personal development experience — the "personality" layer
//! for a local LoRA fine-tune (docs/MEMORY_LOCAL_LLM_ARCHITECTURE.md Phase 4/5).

use super::ebbinghaus::{self, MemoryEntry};
use std::fs;
use std::path::Path;

const SYSTEM_PROMPT: &str = "You are a coding assistant with persistent memory of the user's \
development experience. When asked about the user's own experience, recall it from memory \
and answer in the user's style.";

/// Map a memory category to a natural user question that would retrieve it.
fn category_question(category: &ebbinghaus::MemoryCategory) -> String {
    use ebbinghaus::MemoryCategory::*;
    match category {
        Core => "你希望我长期记住的原则、偏好或项目核心是什么？".to_string(),
        Pattern => "你在编码中最常用的可复用模式或写法是什么？".to_string(),
        Decision => "你在架构或技术选型上做过哪些关键决策？".to_string(),
        Lesson => "你踩过哪些坑？解决之后学到了什么教训？".to_string(),
        BugFix => "你修复过哪些 Bug？修复的方法是什么？".to_string(),
        Performance => "你做性能优化时用过哪些方法？效果如何？".to_string(),
        ApiProtocol => "你在 API 或协议使用中积累了哪些经验？".to_string(),
        _ => "你有哪些值得记住的开发经验？".to_string(),
    }
}

/// Convert a single memory entry into a chat-format training sample.
fn entry_to_sample(entry: &MemoryEntry) -> String {
    let question = category_question(&entry.category);
    let record = serde_json::json!({
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": question },
            { "role": "assistant", "content": entry.text },
        ],
        // Auxiliary fields consumed by pipelines that support metadata
        "category": entry.category.to_tag(),
        "source": "neocoder-memory",
    });
    record.to_string()
}

/// Export all long-term memory entries to a JSONL training dataset.
///
/// Returns a summary line with the output path, sample count, and total chars.
/// Errors are returned when no entries exist or the file cannot be written.
pub fn export_training_data(
    base_dir: &Path,
    entries: &[MemoryEntry],
    output_path: Option<&Path>,
) -> Result<String, String> {
    if entries.is_empty() {
        return Err("No long-term memory entries to export. Run some sessions first.".to_string());
    }

    let out_dir = match output_path {
        Some(p) => {
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create output dir: {}", e))?;
            }
            p.to_path_buf()
        }
        None => base_dir.join("finetune"),
    };
    if output_path.is_none() {
        fs::create_dir_all(&out_dir)
            .map_err(|e| format!("Failed to create finetune dir: {}", e))?;
    }
    let out_file = out_dir.join("neocoder_memory.jsonl");

    let mut total_chars = 0usize;
    let mut lines = Vec::with_capacity(entries.len());
    for entry in entries {
        let line = entry_to_sample(entry);
        total_chars += line.len();
        lines.push(line);
    }

    let content = lines.join("\n");
    fs::write(&out_file, content).map_err(|e| format!("Failed to write dataset: {}", e))?;

    let summary = format!(
        "Exported {} training samples ({:.1} KB) to {}",
        entries.len(),
        total_chars as f64 / 1024.0,
        out_file.display()
    );
    log::info!("[Finetune] {}", summary);
    Ok(summary)
}
