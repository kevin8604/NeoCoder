//! AST-accurate code chunker backed by tree-sitter grammars.
//!
//! Enabled via `cargo build --features treesitter`. When the feature is off,
//! `chunk_code` in the parent module falls through to the line-based
//! heuristic; when on, we try this module first for Rust / TypeScript /
//! TSX / JavaScript / Python files and return `None` on any failure so the
//! heuristic still runs.

use super::{ChunkType, CodeChunk};
use tree_sitter::{Language, Node, Parser};

/// Return a tree-sitter [`Language`] for a file path, or None if we do not
/// have a grammar registered for this extension.
fn grammar_for(file_path: &str) -> Option<(&'static str, Language)> {
    let ext = file_path.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some(("rust", tree_sitter_rust::LANGUAGE.into())),
        "ts" => Some((
            "typescript",
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        )),
        "tsx" => Some(("tsx", tree_sitter_typescript::LANGUAGE_TSX.into())),
        "js" | "jsx" => Some(("javascript", tree_sitter_typescript::LANGUAGE_TSX.into())),
        "py" | "pyi" => Some(("python", tree_sitter_python::LANGUAGE.into())),
        _ => None,
    }
}

/// Node kinds we treat as one emitted chunk (function-ish definitions).
fn chunk_kind(node: &Node, language: &str) -> Option<(&'static str, ChunkType)> {
    let kind = node.kind();
    match language {
        "rust" => match kind {
            "function_item" => Some((kind, ChunkType::Function)),
            "impl_item" | "impl_block" => Some((kind, ChunkType::Class)),
            "struct_item" | "enum_item" | "trait_item" => Some((kind, ChunkType::Class)),
            "mod_item" => Some((kind, ChunkType::Module)),
            _ => None,
        },
        "typescript" | "tsx" | "javascript" => match kind {
            "function_declaration"
            | "method_definition"
            | "arrow_function"
            | "generator_function_declaration" => Some((kind, ChunkType::Function)),
            "class_declaration" => Some((kind, ChunkType::Class)),
            "interface_declaration" | "enum_declaration" | "module" => {
                Some((kind, ChunkType::Class))
            }
            _ => None,
        },
        "python" => match kind {
            "function_definition" => Some((kind, ChunkType::Function)),
            "class_definition" => Some((kind, ChunkType::Class)),
            "decorated_definition" => Some((kind, ChunkType::Function)),
            _ => None,
        },
        _ => None,
    }
}

/// Extract a short human-readable summary from the first line of a chunk.
fn first_line_summary(src: &str) -> String {
    src.lines()
        .next()
        .map(|l| l.trim().to_string())
        .unwrap_or_default()
}

/// Attempt to chunk a source file via tree-sitter. Returns `None` when:
/// - no grammar registered for this file extension
/// - parser/grammar initialization fails
/// - the tree contains no recognizable top-level definitions
///
/// In every `None` case, callers should fall back to their heuristic path.
pub fn chunk_with_tree_sitter(file_path: &str, content: &str) -> Option<Vec<CodeChunk>> {
    let (lang_name, language): (&str, Language) = grammar_for(file_path)?;

    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(content, None)?;
    let root = tree.root_node();
    if root.has_error() && root.child_count() == 0 {
        return None;
    }

    let bytes = content.as_bytes();
    let mut chunks: Vec<CodeChunk> = Vec::new();
    let id_prefix = uuid::Uuid::new_v4().to_string();

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if let Some((kind, ctype)) = chunk_kind(&child, lang_name) {
            let start_byte = child.start_byte();
            let end_byte = child.end_byte().min(bytes.len());
            if end_byte <= start_byte {
                continue;
            }
            let src = std::str::from_utf8(&bytes[start_byte..end_byte])
                .ok()?
                .to_string();
            let start_line = child.start_position().row + 1;
            let end_line = child.end_position().row + 1;
            chunks.push(CodeChunk {
                id: format!("{}-ts-{}-{}", id_prefix, kind, start_line),
                file_path: file_path.to_string(),
                start_line,
                end_line,
                language: lang_name.to_string(),
                chunk_type: ctype,
                content: src.clone(),
                summary: first_line_summary(&src),
            });
        }
    }

    // Nothing recognized → let the caller fall back.
    if chunks.is_empty() {
        return None;
    }
    Some(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_grammar_recognized() {
        assert!(grammar_for("a.rs").is_some());
        assert!(grammar_for("a.ts").is_some());
        assert!(grammar_for("a.tsx").is_some());
        assert!(grammar_for("a.py").is_some());
        assert!(grammar_for("a.txt").is_none());
    }

    #[test]
    fn rust_functions_chunked() {
        let src = r#"
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

struct Foo {
    x: i32,
}

impl Foo {
    pub fn new(x: i32) -> Self { Self { x } }
}
"#;
        let chunks = chunk_with_tree_sitter("lib.rs", src).expect("chunks");
        assert!(
            chunks
                .iter()
                .any(|c| matches!(c.chunk_type, ChunkType::Function))
        );
        assert!(
            chunks
                .iter()
                .any(|c| matches!(c.chunk_type, ChunkType::Class))
        );
        for c in &chunks {
            assert!(c.start_line >= 1);
            assert!(c.end_line >= c.start_line);
            assert!(!c.content.is_empty());
        }
    }

    #[test]
    fn tsx_components_chunked() {
        let src = r#"
export function Widget({ id }: { id: string }) {
  return <div>{id}</div>;
}

class Container extends Widget {}
"#;
        let chunks = chunk_with_tree_sitter("view.tsx", src).expect("chunks");
        assert!(!chunks.is_empty());
    }

    #[test]
    fn unknown_extension_returns_none() {
        assert!(chunk_with_tree_sitter("notes.txt", "hello").is_none());
    }

    #[test]
    fn empty_python_returns_none() {
        // No recognized definitions → fall back to heuristic path.
        assert!(chunk_with_tree_sitter("e.py", "").is_none());
    }
}
