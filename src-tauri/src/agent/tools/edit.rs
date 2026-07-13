use crate::agent::utils::resolve_path;
use super::{Tool, ToolContext};

pub struct Edit;

#[async_trait::async_trait]
impl Tool for Edit {
    fn name(&self) -> &str { "edit" }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let raw = args["file_path"].as_str().unwrap_or("");
        if raw.is_empty() {
            return "Error: file_path is required".to_string();
        }

        let file_path = resolve_path(ctx.project_path.as_deref(), raw);
        if let Err(e) = ctx.sandbox.check_path(&file_path, ctx.project_path.as_deref(), true) {
            return format!("Error: Sandbox blocked: {}", e);
        }

        let original_content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(e) => return format!("Error reading file {}: {}", file_path.display(), e),
        };

        // ── P2: Multi-hunk editing ──
        if let Some(hunks_val) = args.get("hunks") {
            if let Some(hunks_arr) = hunks_val.as_array() {
                return self.execute_multi_hunk(hunks_arr, &original_content, &file_path);
            }
        }

        let old_string = args["old_string"].as_str().unwrap_or("");
        let new_string = args["new_string"].as_str().unwrap_or("");
        let start_line = args["start_line"].as_u64().map(|n| n as usize);
        let end_line = args["end_line"].as_u64().map(|n| n as usize);
        let replace_all = args["replace_all"].as_bool().unwrap_or(false);

        if old_string.is_empty() {
            return "Error: old_string is required".to_string();
        }

        self.execute_single(
            ctx,
            &original_content, &file_path, old_string, new_string,
            start_line, end_line, replace_all,
        ).await
    }
}

impl Edit {
    // ────────────────────────────────────────────
    //  Single-hunk edit with 5-pass matching (P4 = AI-assisted)
    // ────────────────────────────────────────────
    async fn execute_single(
        &self,
        ctx: &ToolContext,
        original_content: &str,
        file_path: &std::path::Path,
        old_string: &str,
        new_string: &str,
        start_line: Option<usize>,
        end_line: Option<usize>,
        replace_all: bool,
    ) -> String {
        // ── Pass 1: Exact match ──
        let matches: Vec<(usize, usize)> = original_content
            .match_indices(old_string)
            .map(|(off, _)| (off, byte_offset_to_line(original_content, off)))
            .collect();
        if !matches.is_empty() {
            return handle_matches(
                original_content, file_path, old_string, new_string,
                &matches, start_line, end_line, replace_all,
            );
        }

        // ── Pass 2: Line-ending normalized match ──
        let norm_old = normalize_line_endings(old_string);
        let norm_content = normalize_line_endings(original_content);
        let norm_matches: Vec<(usize, usize)> = norm_content
            .match_indices(&norm_old)
            .map(|(off, _)| (off, byte_offset_to_line(&norm_content, off)))
            .collect();
        if !norm_matches.is_empty() {
            log::info!("Edit Pass 2: line-ending normalized match ({} hits)", norm_matches.len());
            let norm_new = normalize_line_endings(new_string);
            return handle_matches(
                &norm_content, file_path, &norm_old, &norm_new,
                &norm_matches, start_line, end_line, replace_all,
            );
        }

        // ── Pass 3: Whitespace-trimmed per-line match ──
        let trimmed_old = trim_lines(old_string);
        let trimmed_content = trim_lines(original_content);
        let trimmed_matches: Vec<(usize, usize)> = trimmed_content
            .match_indices(&trimmed_old)
            .map(|(off, _)| (off, byte_offset_to_line(&trimmed_content, off)))
            .collect();
        if !trimmed_matches.is_empty() {
            log::info!("Edit Pass 3: whitespace-trimmed match ({} hits)", trimmed_matches.len());
            let first_line = byte_offset_to_line(&trimmed_content, trimmed_matches[0].0);
            let old_line_count = trimmed_old.lines().count();
            let adjusted = adjust_indentation(original_content, old_string, new_string, first_line);
            return replace_by_lines(
                original_content, file_path, old_string, &adjusted,
                first_line, old_line_count,
            );
        }

        // ── Pass 4 (P0): Fuzzy match via Levenshtein sliding window ──
        if let Some((line, score)) = fuzzy_match(original_content, old_string, start_line) {
            log::info!(
                "Edit Pass 4: fuzzy match at line {} (score {:.3})",
                line, score
            );
            let old_lines = old_string.lines().count();
            let adjusted = adjust_indentation(original_content, old_string, new_string, line);
            return replace_by_lines(
                original_content, file_path, old_string, &adjusted,
                line, old_lines,
            );
        }

        // ── All deterministic passes failed ──
        // ── Pass 5 (P4): AI-assisted fallback ──
        if let Some(corrected_old) = ai_assisted_edit(ctx, original_content, old_string, start_line).await {
            log::info!("Edit Pass 5: AI-assisted match succeeded");
            // Retry with corrected old_string through normal pipeline
            let retry_matches: Vec<(usize, usize)> = original_content
                .match_indices(&corrected_old)
                .map(|(off, _)| (off, byte_offset_to_line(original_content, off)))
                .collect();
            if !retry_matches.is_empty() {
                return handle_matches(
                    original_content, file_path, &corrected_old, new_string,
                    &retry_matches, start_line, end_line, replace_all,
                );
            }
        }

        // ── All passes failed ──
        build_diagnostic_error(original_content, file_path, old_string, start_line)
    }

    // ────────────────────────────────────────────
    //  P2: Multi-hunk editing
    // ────────────────────────────────────────────
    fn execute_multi_hunk(
        &self,
        hunks: &[serde_json::Value],
        original_content: &str,
        file_path: &std::path::Path,
    ) -> String {
        if hunks.is_empty() {
            return "Error: hunks array is empty".to_string();
        }

        // Phase 1: resolve all hunks (find match positions)
        let mut resolved: Vec<ResolvedHunk> = Vec::with_capacity(hunks.len());
        for (i, hunk) in hunks.iter().enumerate() {
            let old_s = hunk["old_string"].as_str().unwrap_or("");
            let new_s = hunk["new_string"].as_str().unwrap_or("");
            let hint = hunk["start_line"].as_u64().map(|n| n as usize);

            if old_s.is_empty() {
                return format!("Error: hunks[{}].old_string is empty", i);
            }

            match find_best_match(original_content, old_s, hint) {
                Some((start_line, _score)) => {
                    let old_lines = old_s.lines().count();
                    let adjusted = adjust_indentation(original_content, old_s, new_s, start_line);
                    resolved.push(ResolvedHunk { start_line, old_lines, new_text: adjusted });
                }
                None => {
                    return format!(
                        "Error: hunks[{}].old_string not found in file (even with fuzzy match).\n\
                         Use read_file to check the current file content.",
                        i
                    );
                }
            }
        }

        // Phase 2: sort by start_line descending → apply from bottom to top (no offset shift)
        resolved.sort_by(|a, b| b.start_line.cmp(&a.start_line));

        let mut lines: Vec<String> = original_content.lines().map(|l| l.to_string()).collect();
        for hunk in &resolved {
            let start_idx = hunk.start_line - 1;
            let end_idx = (start_idx + hunk.old_lines).min(lines.len());
            let new_lines: Vec<String> = hunk.new_text.lines().map(|l| l.to_string()).collect();
            lines.splice(start_idx..end_idx, new_lines);
        }

        let new_content = lines.join("\n") + "\n";
        let diff = generate_unified_diff(original_content, &new_content, file_path);

        match std::fs::write(file_path, &new_content) {
            Ok(()) => format!(
                "Successfully edited {} ({} hunks applied)\n\n{}",
                file_path.display(), resolved.len(), diff
            ),
            Err(e) => format!("Error writing to {}: {}", file_path.display(), e),
        }
    }
}

// ────────────────────────────────────────────────
//  Resolved hunk (for multi-hunk editing)
// ────────────────────────────────────────────────
struct ResolvedHunk {
    start_line: usize,
    old_lines: usize,
    new_text: String,
}

// ────────────────────────────────────────────────
//  Find best match (exact → normalized → trimmed → fuzzy)
// ────────────────────────────────────────────────
fn find_best_match(content: &str, old_string: &str, hint: Option<usize>) -> Option<(usize, f64)> {
    // Exact
    let exact: Vec<(usize, usize)> = content
        .match_indices(old_string)
        .map(|(off, _)| (off, byte_offset_to_line(content, off)))
        .collect();
    if !exact.is_empty() {
        let best = pick_best(&exact, hint);
        return Some((best.1, 1.0));
    }

    // Line-ending normalized
    let norm_old = normalize_line_endings(old_string);
    let norm_content = normalize_line_endings(content);
    let norm_matches: Vec<(usize, usize)> = norm_content
        .match_indices(&norm_old)
        .map(|(off, _)| (off, byte_offset_to_line(&norm_content, off)))
        .collect();
    if !norm_matches.is_empty() {
        let best = pick_best(&norm_matches, hint);
        return Some((best.1, 0.99));
    }

    // Trimmed
    let trimmed_old = trim_lines(old_string);
    let trimmed_content = trim_lines(content);
    let trimmed_matches: Vec<(usize, usize)> = trimmed_content
        .match_indices(&trimmed_old)
        .map(|(off, _)| (off, byte_offset_to_line(&trimmed_content, off)))
        .collect();
    if !trimmed_matches.is_empty() {
        let best = pick_best(&trimmed_matches, hint);
        return Some((best.1, 0.95));
    }

    // Fuzzy
    fuzzy_match(content, old_string, hint)
}

fn pick_best(matches: &[(usize, usize)], hint: Option<usize>) -> (usize, usize) {
    if let Some(h) = hint {
        matches.iter()
            .min_by_key(|(_, line)| (*line as i64 - h as i64).unsigned_abs())
            .copied()
            .unwrap_or(matches[0])
    } else {
        matches[0]
    }
}

// ────────────────────────────────────────────────
//  P0: Fuzzy match (Levenshtein sliding window)
// ────────────────────────────────────────────────
fn fuzzy_match(content: &str, old_string: &str, hint: Option<usize>) -> Option<(usize, f64)> {
    let old_lines: Vec<&str> = old_string.lines().collect();
    let old_count = old_lines.len();
    if old_count == 0 { return None; }

    let content_lines: Vec<&str> = content.lines().collect();
    let total = content_lines.len();
    if total < old_count { return None; }

    let threshold = 0.65;
    let mut best: Option<(usize, f64)> = None; // (1-based line, score)

    // Build search window: prefer hint region, then scan all
    let search_start = 0usize;
    let search_end = total.saturating_sub(old_count);

    // If hint given, search hint±20% first, then expand
    let ranges = if let Some(h) = hint {
        let center = h.saturating_sub(1);
        let radius = (total / 5).max(old_count);
        let r_start = center.saturating_sub(radius);
        let r_end = (center + radius).min(search_end);
        vec![(r_start, r_end), (search_start, r_start), (r_end, search_end)]
    } else {
        vec![(search_start, search_end)]
    };

    let mut found = false;
    for &(range_start, range_end) in &ranges {
        if found { break; }
        for i in range_start..=range_end {
            let window = content_lines[i..(i + old_count)].join("\n");
            let score = strsim::normalized_levenshtein(&window, old_string);
            if score > threshold {
                if best.is_none() || score > best.unwrap().1 {
                    best = Some((i + 1, score)); // 1-based
                    if score > 0.90 { return best; } // good enough, early exit
                }
            }
        }
        if best.is_some() { found = true; }
    }

    best
}

// ────────────────────────────────────────────────
//  P1: Indentation preservation
// ────────────────────────────────────────────────
fn adjust_indentation(content: &str, old_string: &str, new_string: &str, match_line: usize) -> String {
    let content_lines: Vec<&str> = content.lines().collect();
    if match_line == 0 || match_line > content_lines.len() {
        return new_string.to_string();
    }

    // Capture the leading whitespace of the matched block in the file
    let original_indent = leading_whitespace(content_lines[match_line - 1]);

    // Capture the leading whitespace of old_string's first line
    let old_first_line = old_string.lines().next().unwrap_or("");
    let old_indent = leading_whitespace(old_first_line);

    // If indentation is the same, no adjustment needed
    if original_indent == old_indent {
        return new_string.to_string();
    }

    // Re-indent each line of new_string
    let new_lines: Vec<&str> = new_string.lines().collect();
    if new_lines.is_empty() { return new_string.to_string(); }

    let mut result = Vec::with_capacity(new_lines.len());

    // First line: replace old_indent prefix with original_indent
    let first = reindent_line(new_lines[0], old_indent, original_indent);
    result.push(first);

    // Subsequent lines: preserve their relative indentation to the first line
    for line in &new_lines[1..] {
        let line_indent = leading_whitespace(line);
        // Calculate relative indent from new_string's first line
        let relative = if line_indent.starts_with(old_indent) {
            // line_indent = old_indent + extra → replace old_indent with original_indent
            let extra = &line_indent[old_indent.len()..];
            format!("{}{}", original_indent, extra)
        } else if old_indent.is_empty() {
            // old had no indent; keep line's own indent + add original
            format!("{}{}", original_indent, line_indent)
        } else {
            // Different indent base; just use original + line's own indent
            format!("{}{}", original_indent, line_indent)
        };
        let content_part = line.trim_start();
        result.push(format!("{}{}", relative, content_part));
    }

    let trailing = if new_string.ends_with('\n') { "\n" } else { "" };
    result.join("\n") + trailing
}

fn leading_whitespace(line: &str) -> &str {
    let trimmed = line.trim_start();
    &line[..line.len() - trimmed.len()]
}

fn reindent_line(line: &str, old_indent: &str, new_indent: &str) -> String {
    let content = line.trim_start();
    let line_indent = leading_whitespace(line);
    if line_indent.starts_with(old_indent) {
        let extra = &line_indent[old_indent.len()..];
        format!("{}{}{}", new_indent, extra, content)
    } else if old_indent.is_empty() {
        format!("{}{}{}", new_indent, line_indent, content)
    } else {
        format!("{}{}", new_indent, content)
    }
}

// ────────────────────────────────────────────────
//  P3: Unified diff generation
// ────────────────────────────────────────────────
fn generate_unified_diff(old_content: &str, new_content: &str, file_path: &std::path::Path) -> String {
    let old_lines: Vec<&str> = old_content.lines().collect();
    let new_lines: Vec<&str> = new_content.lines().collect();

    // Simple LCS-based diff
    let mut diff_lines = Vec::new();
    let fname = file_path.file_name().and_then(|f| f.to_str()).unwrap_or("file");
    diff_lines.push(format!("--- a/{}", fname));
    diff_lines.push(format!("+++ b/{}", fname));

    // Use a simple line-by-line comparison with context
    let max_len = old_lines.len().max(new_lines.len());
    let mut i = 0;
    let mut hunks: Vec<Vec<String>> = Vec::new();
    let mut current_hunk: Vec<String> = Vec::new();
    let mut in_hunk = false;

    while i < max_len {
        let old_line = old_lines.get(i).copied();
        let new_line = new_lines.get(i).copied();

        match (old_line, new_line) {
            (Some(o), Some(n)) if o == n => {
                if in_hunk {
                    current_hunk.push(format!(" {}", o));
                    if current_hunk.len() > 6 {
                        // End context reached, flush hunk
                        hunks.push(std::mem::take(&mut current_hunk));
                        in_hunk = false;
                    }
                }
            }
            (Some(o), Some(n)) => {
                if !in_hunk {
                    // Add pre-context (up to 3 lines)
                    let ctx_start = i.saturating_sub(3);
                    for j in ctx_start..i {
                        current_hunk.push(format!(" {}", old_lines[j]));
                    }
                    in_hunk = true;
                }
                current_hunk.push(format!("-{}", o));
                current_hunk.push(format!("+{}", n));
            }
            (Some(o), None) => {
                if !in_hunk {
                    let ctx_start = i.saturating_sub(3);
                    for j in ctx_start..i {
                        current_hunk.push(format!(" {}", old_lines[j]));
                    }
                    in_hunk = true;
                }
                current_hunk.push(format!("-{}", o));
            }
            (None, Some(n)) => {
                if !in_hunk {
                    let ctx_start = i.saturating_sub(3);
                    for j in ctx_start..i {
                        current_hunk.push(format!(" {}", old_lines[j]));
                    }
                    in_hunk = true;
                }
                current_hunk.push(format!("+{}", n));
            }
            (None, None) => break,
        }
        i += 1;
    }

    if !current_hunk.is_empty() {
        hunks.push(current_hunk);
    }

    if hunks.is_empty() {
        return String::from("(no changes)");
    }

    for hunk in &hunks {
        diff_lines.push(format!("@@ -1,{} +1,{} @@", old_lines.len(), new_lines.len()));
        diff_lines.extend(hunk.iter().cloned());
    }

    diff_lines.join("\n")
}

// ────────────────────────────────────────────────
//  Handle matched results — disambiguation, replace_all
// ────────────────────────────────────────────────
fn handle_matches(
    content: &str,
    file_path: &std::path::Path,
    old_string: &str,
    new_string: &str,
    matches: &[(usize, usize)],
    start_line: Option<usize>,
    end_line: Option<usize>,
    replace_all: bool,
) -> String {
    match matches.len() {
        1 if !replace_all => {
            let new_content = content.replacen(old_string, new_string, 1);
            write_edit(file_path, &new_content, content, old_string, 1)
        }
        _ if replace_all => {
            let count = matches.len();
            let new_content = content.replace(old_string, new_string);
            write_edit(file_path, &new_content, content, old_string, count)
        }
        _ => {
            // Multiple matches — disambiguate with start_line
            if let Some(target_line) = start_line {
                let candidates: Vec<&(usize, usize)> = if let Some(end) = end_line {
                    matches.iter()
                        .filter(|(_, line)| *line >= target_line && *line <= end)
                        .collect()
                } else {
                    matches.iter().collect()
                };

                let best = if candidates.is_empty() {
                    matches.iter().min_by_key(|(_, line)| {
                        (*line as i64 - target_line as i64).unsigned_abs()
                    })
                } else {
                    candidates.iter().min_by_key(|(_, line)| {
                        (*line as i64 - target_line as i64).unsigned_abs()
                    }).copied()
                };

                if let Some(&(offset, match_line)) = best {
                    log::info!(
                        "Edit: disambiguated {} matches, chose line {} (target: {})",
                        matches.len(), match_line, target_line
                    );
                    let new_content = replace_at_offset(content, offset, old_string, new_string);
                    write_edit(file_path, &new_content, content, old_string, 1)
                } else {
                    format!(
                        "Error: old_string appears {} times. start_line={} did not help.",
                        matches.len(), target_line
                    )
                }
            } else {
                let match_lines: Vec<String> = matches.iter()
                    .take(5)
                    .map(|(_, line)| format!("line {}", line))
                    .collect();
                let more = if matches.len() > 5 {
                    format!(" ... and {} more", matches.len() - 5)
                } else {
                    String::new()
                };
                format!(
                    "Error: old_string appears {} times (at {}). \
                     Provide start_line or use replace_all: true.",
                    matches.len(), match_lines.join(", ")
                ) + &more
            }
        }
    }
}

// ────────────────────────────────────────────────
//  Line-based replacement (Pass 3/4 fallback)
// ────────────────────────────────────────────────
fn replace_by_lines(
    content: &str,
    file_path: &std::path::Path,
    _old_string: &str,
    new_string: &str,
    start_line: usize,
    line_count: usize,
) -> String {
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    if start_line == 0 || start_line > lines.len() {
        return format!(
            "Error: line-based fallback failed, start_line={} out of range (file has {} lines)",
            start_line, lines.len()
        );
    }
    let start_idx = start_line - 1;
    let end_idx = (start_idx + line_count).min(lines.len());
    let new_lines: Vec<String> = new_string.lines().map(|l| l.to_string()).collect();
    lines.splice(start_idx..end_idx, new_lines);
    let new_content = lines.join("\n") + "\n";

    let diff = generate_unified_diff(content, &new_content, file_path);
    log::info!(
        "Edit: line-based replacement at lines {}-{} ({} old → {} new lines)",
        start_line, end_idx, end_idx - start_idx, new_string.lines().count()
    );

    match std::fs::write(file_path, &new_content) {
        Ok(()) => format!(
            "Successfully edited {} (fuzzy/normalized match at line {})\n\n{}",
            file_path.display(), start_line, diff
        ),
        Err(e) => format!("Error writing to {}: {}", file_path.display(), e),
    }
}

// ────────────────────────────────────────────────
//  Write + diff feedback
// ────────────────────────────────────────────────
fn write_edit(
    file_path: &std::path::Path,
    new_content: &str,
    original_content: &str,
    old_string: &str,
    replacements: usize,
) -> String {
    let diff = generate_unified_diff(original_content, new_content, file_path);
    match std::fs::write(file_path, new_content) {
        Ok(()) => {
            let lines_changed = old_string.lines().count() * replacements;
            let msg = if replacements == 1 {
                format!("Successfully edited {}: 1 occurrence ({} lines)", file_path.display(), lines_changed)
            } else {
                format!("Successfully edited {}: {} occurrences ({} lines)", file_path.display(), replacements, lines_changed)
            };
            format!("{}\n\n{}", msg, diff)
        }
        Err(e) => format!("Error writing to {}: {}", file_path.display(), e),
    }
}

// ────────────────────────────────────────────────
//  Diagnostic error (all passes failed)
// ────────────────────────────────────────────────
fn build_diagnostic_error(
    content: &str,
    file_path: &std::path::Path,
    old_string: &str,
    start_line: Option<usize>,
) -> String {
    let all_lines: Vec<&str> = content.lines().collect();
    let total = all_lines.len();

    let first_old_line = old_string.lines().next().unwrap_or("").trim();
    let approx_line = if !first_old_line.is_empty() {
        all_lines.iter().position(|l| l.trim() == first_old_line)
    } else {
        None
    };

    let (show_start, show_end) = if let Some(sl) = start_line {
        let center = sl.min(total).saturating_sub(1);
        (center.saturating_sub(3), (center + 4).min(total))
    } else if let Some(al) = approx_line {
        (al.saturating_sub(3), (al + 4).min(total))
    } else {
        (0, total.min(10))
    };

    let mut snippet = String::new();
    for i in show_start..show_end {
        snippet.push_str(&format!("  {:>4} | {}\n", i + 1, all_lines[i]));
    }

    let old_lines: Vec<&str> = old_string.lines().collect();
    let old_preview = if old_lines.len() <= 4 {
        old_lines.iter().map(|l| format!("  | {}", l)).collect::<Vec<_>>().join("\n")
    } else {
        format!(
            "  | {}\n  | {}\n  | {}\n  | ... ({} lines omitted) ...\n  | {}",
            old_lines[0], old_lines[1], old_lines[2],
            old_lines.len() - 4,
            old_lines.last().unwrap()
        )
    };

    let has_crlf_old = old_string.contains("\r\n");
    let has_crlf_file = content.contains("\r\n");
    let cause = if has_crlf_old && !has_crlf_file {
        "Likely cause: old_string uses CRLF but file uses LF."
    } else if !has_crlf_old && has_crlf_file {
        "Likely cause: old_string uses LF but file uses CRLF."
    } else {
        "Likely cause: content mismatch (whitespace, encoding, or file has been modified since last read)."
    };

    format!(
        "Error: old_string not found in {} ({} lines) — all 4 match passes failed.\n\
         {}\n\n\
         Expected old_string:\n{}\n\n\
         Actual file content near {}:\n{}\n\n\
         Tip: Use read_file to see the exact current content, then retry with the correct text.",
        file_path.display(), total,
        cause,
        old_preview,
        if let Some(al) = approx_line {
            format!("line {}", al + 1)
        } else if let Some(sl) = start_line {
            format!("line {}", sl)
        } else {
            "file start".to_string()
        },
        snippet,
    )
}

// ────────────────────────────────────────────────
//  P4: AI-assisted edit fallback
// ────────────────────────────────────────────────

/// When all deterministic matching passes fail, call the LLM to find the
/// exact text in the file that corresponds to old_string.
/// Returns the corrected old_string that can be used for replacement.
async fn ai_assisted_edit(
    ctx: &ToolContext,
    file_content: &str,
    old_string: &str,
    start_line: Option<usize>,
) -> Option<String> {
    log::info!("Edit Pass 5: attempting AI-assisted match");

    // Truncate file content if too large (keep relevant region)
    let relevant_content = if let Some(line) = start_line {
        let lines: Vec<&str> = file_content.lines().collect();
        let center = line.saturating_sub(1);
        let radius = 50;
        let start = center.saturating_sub(radius);
        let end = (center + radius).min(lines.len());
        let snippet = lines[start..end].join("\n");
        if start > 0 {
            format!("... ({} lines omitted above) ...\n{}", start, snippet)
        } else {
            snippet
        }
    } else {
        // If no hint, take first 200 lines
        let lines: Vec<&str> = file_content.lines().collect();
        let take = lines.len().min(200);
        let snippet = lines[..take].join("\n");
        if lines.len() > take {
            format!("{}\n... ({} more lines omitted) ...", snippet, lines.len() - take)
        } else {
            snippet
        }
    };

    let prompt = format!(
        "You are a code editing assistant. The user wants to find and replace text in a file, \
         but the old_string they provided doesn't exactly match the file content.\n\
         \n\
         Your task: Find the EXACT text in the file that the user intended to match.\n\
         \n\
         Rules:\n\
         1. Return ONLY the exact text from the file (copy-paste, no modifications).\n\
         2. The text should semantically match what the user's old_string describes.\n\
         3. Include the same number of lines as the user's old_string.\n\
         4. Return nothing else — no explanation, no quotes, no markdown.\n\
         \n\
         User's old_string:\n\
         ```\n{}\n```\n\
         \n\
         File content (relevant region):\n\
         ```\n{}\n```\n\
         \n\
         Return the exact matching text from the file:",
        old_string, relevant_content
    );

    let request = crate::llm::ChatRequestParams {
        model: ctx.llm_model.clone(),
        messages: vec![crate::llm::ChatMessage {
            role: "user".into(),
            content: prompt,
            images: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        system: "You are a precise code matching assistant. Return only exact file text.".into(),
        max_tokens: 500,
        temperature: 0.0,
        thinking_enabled: false,
        thinking_budget: 0,
    };

    let empty_tools: Vec<serde_json::Value> = vec![];
    match crate::llm::chat_with_tools(
        &ctx.llm_provider,
        &ctx.llm_api_key,
        ctx.llm_base_url.as_deref(),
        request,
        &empty_tools,
        None,
    ).await {
        Ok((response, _usage)) => {
            match response {
                crate::llm::LlmResponse::Text(text) => {
                    let trimmed = text.trim().to_string();
                    if trimmed.is_empty() {
                        log::warn!("Edit Pass 5: LLM returned empty response");
                        None
                    } else {
                        log::info!("Edit Pass 5: LLM returned {} chars", trimmed.len());
                        Some(trimmed)
                    }
                }
                _ => {
                    log::warn!("Edit Pass 5: LLM returned non-text response");
                    None
                }
            }
        }
        Err(e) => {
            log::warn!("Edit Pass 5: LLM call failed: {}", e);
            None
        }
    }
}

// ───────────────────────────────────────────────
//  Helpers
// ────────────────────────────────────────────────
fn byte_offset_to_line(content: &str, offset: usize) -> usize {
    content[..offset].matches('\n').count() + 1
}

fn replace_at_offset(content: &str, offset: usize, old_string: &str, new_string: &str) -> String {
    let mut result = String::with_capacity(content.len() - old_string.len() + new_string.len());
    result.push_str(&content[..offset]);
    result.push_str(new_string);
    result.push_str(&content[offset + old_string.len()..]);
    result
}

fn normalize_line_endings(s: &str) -> String {
    s.replace("\r\n", "\n").replace("\r", "\n")
}

fn trim_lines(s: &str) -> String {
    s.lines().map(|line| line.trim_end()).collect::<Vec<_>>().join("\n")
}
