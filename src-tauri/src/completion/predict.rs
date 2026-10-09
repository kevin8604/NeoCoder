//! Next-edit prediction: after the user makes an edit, ask the fast model
//! where the NEXT edit is likely to be (Cursor-Tab-style "jump to the next
//! change site"). The prompt feeds the model the active document, the cursor
//! position and the recent-edit trail maintained by EditIntentTracker; the
//! model responds with a strict JSON object describing a find/replace suggestion.
//!
//! If no useful next edit exists, the model returns `{"file":""}` and the
//! frontend simply does not display a hint.

use serde::{Deserialize, Serialize};

/// A predicted next edit — the target file, the exact `find` text to replace,
/// the `replace` suggestion, and a short human-readable `reason`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NextEdit {
    /// Absolute or project-relative path to the target file. Empty = no suggestion.
    pub file: String,
    /// 1-based line hint from the model (may be absent when `find` is empty).
    #[serde(default)]
    pub line: u32,
    /// Exact text to locate at the target line.
    #[serde(default)]
    pub find: String,
    /// Replacement text (may be empty when the suggestion is "move cursor here").
    #[serde(default)]
    pub replace: String,
    /// Short one-line rationale shown next to the ghost hint.
    #[serde(default)]
    pub reason: String,
}

impl NextEdit {
    pub fn is_empty(&self) -> bool {
        self.file.is_empty()
    }
}

/// System prompt enforcing strict JSON output.
pub const PREDICT_SYSTEM_PROMPT: &str = "You are a code-edit-prediction model. Given a snippet \
of the file the developer is editing plus a trail of recently edited files, respond with ONE \
JSON object describing the SINGLE most likely next edit site. Format: \
{\"file\":\"<path>\",\"line\":<1-based int>,\"find\":\"<exact substring to replace>\",\"replace\":\"<new text>\",\"reason\":\"<one short clause>\"}. \
Rules: prefer same-file edits the user would naturally make next (analogous change, related \
branch, matching pattern); if no strong next site exists, respond with {\"file\":\"\"}. \
Never wrap the JSON in markdown fences; never add prose before or after.";

/// Build the user prompt for the fast model.
pub fn build_predict_prompt(
    active_file: &str,
    cursor_line: u32,
    document: &str,
    recent_edits: &[String],
) -> String {
    let doc = truncate_for_prompt(document, 6000);
    let mut s = String::new();
    s.push_str("Active file: ");
    s.push_str(active_file);
    s.push_str(&format!("\nCursor line: {}\n", cursor_line));
    if !recent_edits.is_empty() {
        s.push_str("\nRecently edited files (most recent first):\n");
        for p in recent_edits.iter().take(6) {
            s.push_str("  - ");
            s.push_str(p);
            s.push('\n');
        }
    }
    s.push_str("\nDocument (may be truncated):\n```\n");
    s.push_str(&doc);
    s.push_str("\n```\n\nReply with the JSON object only.");
    s
}

fn truncate_for_prompt(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max_chars).collect();
        out.push_str("\n... (truncated)");
        out
    }
}

/// Extract the first JSON object from an LLM response. Tolerates fenced code
/// blocks and surrounding prose; returns None if nothing parses.
pub fn extract_predict_json(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        let after = after
            .strip_prefix("json\n")
            .or_else(|| after.strip_prefix("json"))
            .unwrap_or(after);
        if let Some(end) = after.find("```") {
            let block = after[..end].trim();
            if let Some(ob) = block.find('{')
                && let Some(cb) = block.rfind('}')
            {
                return Some(block[ob..=cb].to_string());
            }
        }
    }
    let ob = trimmed.find('{')?;
    let cb = trimmed.rfind('}')?;
    if cb > ob {
        Some(trimmed[ob..=cb].to_string())
    } else {
        None
    }
}

/// Parse the LLM output into a [`NextEdit`]. Any failure returns None.
pub fn parse_predict_response(text: &str) -> Option<NextEdit> {
    let block = extract_predict_json(text)?;
    let edit: NextEdit = serde_json::from_str(&block).ok()?;
    if edit.is_empty() { None } else { Some(edit) }
}

/// Locate a `find` string inside `document` and return its 0-based line +
/// column (chars). Falls back to (line-1, 0) if the hint line is provided but
/// the find substring is missing (the frontend can still position the ghost
/// hint at the line start).
pub fn locate_edit(edit: &NextEdit, document: &str) -> (usize, usize) {
    if !edit.find.is_empty() {
        for (idx, line) in document.lines().enumerate() {
            if let Some(col) = line.find(&edit.find) {
                return (idx, col);
            }
        }
    }
    if edit.line > 0 {
        ((edit.line as usize).saturating_sub(1), 0)
    } else {
        (0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_includes_file_cursor_and_recent() {
        let p = build_predict_prompt(
            "src/main.rs",
            42,
            "fn main() { /* ... */ }",
            &["src/lib.rs".to_string(), "Cargo.toml".to_string()],
        );
        assert!(p.contains("src/main.rs"));
        assert!(p.contains("Cursor line: 42"));
        assert!(p.contains("src/lib.rs"));
        assert!(p.contains("Cargo.toml"));
        assert!(p.contains("Reply with the JSON object only"));
    }

    #[test]
    fn prompt_truncates_long_documents() {
        let big = "x".repeat(20000);
        let p = build_predict_prompt("f.rs", 1, &big, &[]);
        assert!(p.contains("(truncated)"));
        assert!(p.len() < 8000);
    }

    #[test]
    fn parses_bare_json() {
        let txt = r#"{"file":"src/main.rs","line":10,"find":"let x = 1;","replace":"let x = 2;","reason":"bump"}"#;
        let e = parse_predict_response(txt).unwrap();
        assert_eq!(e.file, "src/main.rs");
        assert_eq!(e.line, 10);
        assert_eq!(e.replace, "let x = 2;");
    }

    #[test]
    fn parses_fenced_json_with_prose() {
        let txt = "Sure!\n```json\n{\"file\":\"a.rs\",\"line\":3,\"find\":\"\",\"replace\":\"\",\"reason\":\"\"}\n```\n";
        let e = parse_predict_response(txt).unwrap();
        assert_eq!(e.file, "a.rs");
        assert_eq!(e.line, 3);
    }

    #[test]
    fn empty_file_is_treated_as_no_prediction() {
        let txt = r#"{"file":""}"#;
        assert!(parse_predict_response(txt).is_none());
    }

    #[test]
    fn locate_falls_back_to_hint_line() {
        let doc = "line0\nline1\nfind me here\n";
        let e = NextEdit {
            file: "x".into(),
            line: 3,
            find: "find me here".into(),
            replace: "".into(),
            reason: "".into(),
        };
        assert_eq!(locate_edit(&e, doc), (2, 0));

        let miss = NextEdit {
            file: "x".into(),
            line: 2,
            find: "nope".into(),
            replace: "".into(),
            reason: "".into(),
        };
        assert_eq!(locate_edit(&miss, doc), (1, 0));
    }

    #[test]
    fn locate_finds_within_line() {
        let doc = "let x = 1;\nlet y = 2;";
        let e = NextEdit {
            file: "x".into(),
            line: 0,
            find: "y".into(),
            replace: "z".into(),
            reason: "".into(),
        };
        assert_eq!(locate_edit(&e, doc), (1, 4));
    }

    #[test]
    fn extract_json_ignores_surrounding_prose() {
        let txt = "Here you go: {\"a\":1} done.";
        assert_eq!(extract_predict_json(txt).unwrap(), "{\"a\":1}");
        assert_eq!(extract_predict_json("no json here"), None);
    }
}
