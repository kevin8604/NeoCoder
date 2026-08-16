//! coverage: line-coverage guidance tool for test writing.
//!
//! Runs `cargo llvm-cov` and reports which source lines are NOT covered by
//! tests. The agent uses this BEFORE writing tests (e.g. in TDD mode or after
//! fixing a baseline) to target the highest-value gaps first, then verifies
//! progress with run_tests.
//!
//! Actions:
//! - `scan` (default): run coverage (or reuse the cached report) and list the
//!   files with the most uncovered lines
//! - `uncovered`: read the cached report without re-running, supports
//!   `path` / `limit` / `min_lines` filters
//! - `status`: show whether a cached report exists and its summary

use super::{Tool, ToolContext};
use crate::terminal::run_one_shot;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct CoverageTool;

const COVERAGE_TIMEOUT_SECS: u64 = 900;
const DEFAULT_LIMIT: usize = 10;

// ── llvm-cov JSON report shapes (only the fields we need) ──

#[derive(Deserialize)]
struct LlvmCovReport {
    data: Vec<LlvmCovData>,
}

#[derive(Deserialize)]
struct LlvmCovData {
    files: Vec<LlvmCovFile>,
}

#[derive(Deserialize)]
struct LlvmCovFile {
    filename: String,
    segments: Vec<LlvmCovSegment>,
}

/// llvm-cov segment 是 6 元素数组：
/// [line, column, count, hasCount, isRegionEntry, isGapRegion]
/// 只需 line 与 count，但必须容忍 6 元素（serde 派生结构体无法从数组取前 N 个）。
#[derive(Clone, Copy)]
struct LlvmCovSegment {
    line: u64,
    count: u64,
}

impl<'de> Deserialize<'de> for LlvmCovSegment {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SegVisitor;
        impl<'de> serde::de::Visitor<'de> for SegVisitor {
            type Value = LlvmCovSegment;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "a segment array [line, column, count, ...]")
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let line: u64 = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &"segment array"))?;
                let _col: u64 = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &"segment array"))?;
                let count: u64 = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(2, &"segment array"))?;
                // 消费剩余元素（hasCount / isRegionEntry / isGapRegion），
                // serde 在 visit_seq 返回后会检查数组是否已耗尽，否则报 trailing characters
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(LlvmCovSegment { line, count })
            }
        }
        deserializer.deserialize_seq(SegVisitor)
    }
}

// ── Cached summary (parsed once, reused by `uncovered` / `scan`) ──

#[derive(Serialize, Deserialize, Clone)]
struct FileCoverage {
    filename: String,
    total_lines: u64,
    covered_lines: u64,
    uncovered_ranges: Vec<(u64, u64)>,
}

#[derive(Serialize, Deserialize)]
struct CoverageCache {
    scanned_at: String,
    total_lines: u64,
    covered_lines: u64,
    files: Vec<FileCoverage>,
}

impl FileCoverage {
    fn uncovered_lines(&self) -> u64 {
        self.total_lines - self.covered_lines
    }
}

impl CoverageCache {
    fn pct_covered(&self) -> f64 {
        if self.total_lines == 0 {
            0.0
        } else {
            self.covered_lines as f64 / self.total_lines as f64 * 100.0
        }
    }
}

/// Merge a sorted list of line numbers into inclusive (start, end) ranges.
fn merge_ranges(lines: &[u64]) -> Vec<(u64, u64)> {
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    for &line in lines {
        if let Some(last) = ranges.last_mut()
            && line == last.1 + 1
        {
            last.1 = line;
            continue;
        }
        ranges.push((line, line));
    }
    ranges
}

/// Parse llvm-cov JSON output, tolerating any non-JSON prefix (cargo/rustup
/// may print info lines to stdout before the report starts). The report is a
/// single top-level JSON object, so we look for the first line that starts
/// with '{' and parse from there.
fn parse_llvm_cov_output(text: &str) -> Result<LlvmCovReport, serde_json::Error> {
    match serde_json::from_str::<LlvmCovReport>(text) {
        Ok(r) => Ok(r),
        Err(_) => {
            // 跳过前导的非 JSON 行（info/警告），从第一个以 '{' 开头的行开始解析
            let mut offset = 0usize;
            for line in text.split_inclusive('\n') {
                if line.trim_start().starts_with('{') {
                    break;
                }
                offset += line.len();
            }
            let json_start = text[offset..].find('{').ok_or_else(|| {
                serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "no JSON object found in output",
                ))
            })? + offset;
            serde_json::from_str::<LlvmCovReport>(&text[json_start..])
        }
    }
}

/// Reduce an llvm-cov file entry to per-line coverage (a line is covered if
/// any of its segments has count > 0), then merge uncovered lines into ranges.
fn summarize_file(file: &LlvmCovFile) -> FileCoverage {
    let mut line_covered: BTreeMap<u64, bool> = BTreeMap::new();
    for seg in &file.segments {
        let covered = line_covered.entry(seg.line).or_insert(false);
        *covered |= seg.count > 0;
    }
    let total = line_covered.len() as u64;
    let covered = line_covered.values().filter(|c| **c).count() as u64;
    let uncovered_lines: Vec<u64> = line_covered
        .iter()
        .filter(|(_, c)| !**c)
        .map(|(l, _)| *l)
        .collect();
    FileCoverage {
        filename: file.filename.clone(),
        total_lines: total,
        covered_lines: covered,
        uncovered_ranges: merge_ranges(&uncovered_lines),
    }
}

/// Only source files inside a crate `src/` directory count; skip build output
/// and third-party sources that llvm-cov may report.
fn is_src_file(filename: &str) -> bool {
    let comps: Vec<String> = Path::new(filename)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    comps.contains(&"src".to_string()) && !comps.contains(&"target".to_string())
}

/// Display filename relative to the project root when possible.
fn display_path(filename: &str, project: &str) -> String {
    if let Ok(rel) = Path::new(filename).strip_prefix(project) {
        rel.to_string_lossy().to_string()
    } else {
        filename.to_string()
    }
}

/// Where the parsed report is cached: {project}/target/coverage_report.json
fn cache_path(ctx: &ToolContext) -> Option<PathBuf> {
    ctx.project_path
        .as_ref()
        .map(|p| Path::new(p).join("target").join("coverage_report.json"))
}

/// Filter a cache down to files matching all of: path substring, min uncovered
/// lines; then take the top `limit` by uncovered line count.
fn filter_and_sort(
    cache: &CoverageCache,
    path_filter: Option<&str>,
    min_lines: u64,
    limit: usize,
    project: &str,
) -> Vec<(String, u64, u64, Vec<(u64, u64)>)> {
    let mut rows: Vec<(String, u64, u64, Vec<(u64, u64)>)> = cache
        .files
        .iter()
        .filter(|f| {
            let path_ok = path_filter.is_none_or(|p| f.filename.contains(p));
            path_ok && f.uncovered_lines() >= min_lines
        })
        .map(|f| {
            (
                display_path(&f.filename, project),
                f.total_lines,
                f.uncovered_lines(),
                f.uncovered_ranges.clone(),
            )
        })
        .collect();
    rows.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    rows.truncate(limit);
    rows
}

fn format_ranges(ranges: &[(u64, u64)], cap: usize) -> String {
    let parts: Vec<String> = ranges
        .iter()
        .map(|(s, e)| {
            if s == e {
                format!("{}", s)
            } else {
                format!("{}-{}", s, e)
            }
        })
        .collect();
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i >= cap {
            out.push_str(&format!(", +{} more", parts.len() - cap));
            break;
        }
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(part);
    }
    out
}

fn format_report(
    cache: &CoverageCache,
    project: &str,
    path_filter: Option<&str>,
    limit: usize,
    min_lines: u64,
    source_note: &str,
) -> String {
    let rows = filter_and_sort(cache, path_filter, min_lines, limit, project);
    let mut out = String::new();
    out.push_str(&format!(
        "Coverage: {:.1}% lines covered ({} of {}) across {} src files{}",
        cache.pct_covered(),
        cache.covered_lines,
        cache.total_lines,
        cache.files.len(),
        source_note,
    ));
    out.push('\n');

    if rows.is_empty() {
        out.push_str(&format!(
            "No files match the filter (path={:?}, min_lines={})",
            path_filter.unwrap_or("any"),
            min_lines
        ));
        return out;
    }

    out.push_str(&format!("Top {} files by uncovered lines:\n", rows.len()));
    for (i, (name, total, uncovered, ranges)) in rows.iter().enumerate() {
        let pct = if *total == 0 {
            0.0
        } else {
            *uncovered as f64 / *total as f64 * 100.0
        };
        out.push_str(&format!(
            "{}. {} — {} uncovered ({} of {} lines, {:.0}% of file): {}\n",
            i + 1,
            name,
            uncovered,
            uncovered,
            total,
            pct,
            format_ranges(ranges, 8),
        ));
    }
    out.push_str("\nGuidance: write tests for the file with the most uncovered lines first; ");
    out.push_str("after adding tests, run_tests to confirm they pass, then rescan with coverage { action: 'scan', force: true }.\n");
    out
}

#[async_trait::async_trait]
impl Tool for CoverageTool {
    fn name(&self) -> &str {
        "coverage"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let action = args["action"].as_str().unwrap_or("scan");
        let project = ctx.project_path.as_deref().unwrap_or(".").to_string();
        let path_filter = args["path"].as_str().filter(|p| !p.is_empty());
        let limit = args["limit"].as_u64().unwrap_or(DEFAULT_LIMIT as u64) as usize;
        let min_lines = args["min_lines"].as_u64().unwrap_or(1);

        match action {
            "status" => {
                let Some(path) = cache_path(ctx) else {
                    return "Coverage cache: no project path configured".to_string();
                };
                match std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|c| serde_json::from_str::<CoverageCache>(&c).ok())
                {
                    Some(cache) => format!(
                        "Coverage cache: {} (scanned at {})\nSummary: {:.1}% lines covered ({} of {}) across {} files\nUse action=scan to rescan, action=uncovered to list gaps.",
                        path.display(),
                        cache.scanned_at,
                        cache.pct_covered(),
                        cache.covered_lines,
                        cache.total_lines,
                        cache.files.len()
                    ),
                    None => format!(
                        "Coverage cache: no report yet at {}\nRun action=scan to generate one (first run compiles with coverage instrumentation and can take several minutes).",
                        path.display()
                    ),
                }
            }
            "uncovered" => {
                let Some(path) = cache_path(ctx) else {
                    return "Error: no project path configured".to_string();
                };
                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(_) => {
                        return format!(
                            "Error: no cached coverage report at {} — run coverage {{ action: 'scan' }} first",
                            path.display()
                        );
                    }
                };
                let cache = match serde_json::from_str::<CoverageCache>(&content) {
                    Ok(c) => c,
                    Err(e) => {
                        return format!(
                            "Error: cached report at {} is corrupt ({}). Rescan with coverage {{ action: 'scan', force: true }}.",
                            path.display(),
                            e
                        );
                    }
                };
                format_report(
                    &cache,
                    &project,
                    path_filter,
                    limit,
                    min_lines,
                    &format!(" (cached, scanned at {})", cache.scanned_at),
                )
            }
            _ => {
                // scan: reuse an existing cache unless force=true, so repeated
                // calls don't recompile the whole crate with instrumentation.
                let force = args["force"].as_bool().unwrap_or(false);
                if !force
                    && let Some(path) = cache_path(ctx)
                    && let Ok(content) = std::fs::read_to_string(&path)
                    && let Ok(cache) = serde_json::from_str::<CoverageCache>(&content)
                {
                    return format_report(
                        &cache,
                        &project,
                        path_filter,
                        limit,
                        min_lines,
                        &format!(
                            " (cached, scanned at {}; pass force:true to rescan)",
                            cache.scanned_at
                        ),
                    );
                }

                let command = "cargo llvm-cov --json".to_string();
                let output = match run_one_shot(&command, &project, COVERAGE_TIMEOUT_SECS).await {
                    Ok(o) => o,
                    Err(e) => {
                        return format!(
                            "Error running '{}': {}\nIf cargo-llvm-cov is missing: cargo install cargo-llvm-cov\n\
                             If llvm-tools is missing: rustup component add llvm-tools-preview \
                             (slow networks: $env:RUSTUP_DIST_SERVER=\"https://rsproxy.cn\")",
                            command, e
                        );
                    }
                };

                let report: LlvmCovReport = match parse_llvm_cov_output(&output.stdout) {
                    Ok(r) => r,
                    Err(e) => {
                        let mut msg = format!(
                            "Error parsing llvm-cov JSON output (exit {}): {}\n",
                            output.exit_code, e
                        );
                        if !output.stderr.is_empty() {
                            msg.push_str(&format!(
                                "stderr:\n{}",
                                &output.stderr[..output.stderr.len().min(2000)]
                            ));
                        }
                        return msg;
                    }
                };

                let mut files: Vec<FileCoverage> = Vec::new();
                for data in &report.data {
                    for file in &data.files {
                        if is_src_file(&file.filename) {
                            files.push(summarize_file(file));
                        }
                    }
                }
                files.sort_by(|a, b| {
                    b.uncovered_lines()
                        .cmp(&a.uncovered_lines())
                        .then(a.filename.cmp(&b.filename))
                });

                let total_lines: u64 = files.iter().map(|f| f.total_lines).sum();
                let covered_lines: u64 = files.iter().map(|f| f.covered_lines).sum();
                let cache = CoverageCache {
                    scanned_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                    total_lines,
                    covered_lines,
                    files,
                };

                if let Some(path) = cache_path(ctx) {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = serde_json::to_string_pretty(&cache)
                        .ok()
                        .and_then(|json| std::fs::write(&path, json).ok());
                }

                format_report(
                    &cache,
                    &project,
                    path_filter,
                    limit,
                    min_lines,
                    " (fresh scan)",
                )
            }
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(line: u64, count: u64) -> LlvmCovSegment {
        LlvmCovSegment { line, count }
    }

    #[test]
    fn test_summarize_file_marks_uncovered_lines() {
        let file = LlvmCovFile {
            filename: "src/foo.rs".to_string(),
            segments: vec![
                seg(1, 1),
                seg(2, 1), // covered
                seg(3, 0),
                seg(4, 0), // uncovered
                seg(5, 5), // covered
                seg(6, 0),
                seg(7, 0),
                seg(8, 0), // uncovered
            ],
        };
        let s = summarize_file(&file);
        assert_eq!(s.total_lines, 8);
        assert_eq!(s.covered_lines, 3);
        assert_eq!(s.uncovered_ranges, vec![(3, 4), (6, 8)]);
    }

    #[test]
    fn test_line_covered_if_any_segment_positive() {
        // 同一行多个 segment：只要有一个 count>0 就算已覆盖
        let file = LlvmCovFile {
            filename: "src/x.rs".to_string(),
            segments: vec![seg(1, 0), seg(1, 3), seg(2, 0)],
        };
        let s = summarize_file(&file);
        assert_eq!(s.covered_lines, 1);
        assert_eq!(s.uncovered_ranges, vec![(2, 2)]);
    }

    #[test]
    fn test_merge_ranges_merges_consecutive() {
        assert_eq!(merge_ranges(&[1, 2, 3, 7, 8]), vec![(1, 3), (7, 8)]);
        assert_eq!(merge_ranges(&[5]), vec![(5, 5)]);
        assert_eq!(merge_ranges(&[]), Vec::<(u64, u64)>::new());
    }

    #[test]
    fn test_is_src_file_filters() {
        assert!(is_src_file(
            "d:/workspace/NeoCoder/src-tauri/src/agent/hooks.rs"
        ));
        assert!(is_src_file("src/agent/hooks.rs"));
        assert!(!is_src_file("d:/workspace/NeoCoder/target/debug/foo.rs"));
        assert!(!is_src_file(
            "d:/workspace/NeoCoder/src-tauri/tests/integration.rs"
        ));
    }

    fn sample_cache() -> CoverageCache {
        CoverageCache {
            scanned_at: "t".to_string(),
            total_lines: 30,
            covered_lines: 19,
            files: vec![
                FileCoverage {
                    filename: "d:/proj/src/a.rs".to_string(),
                    total_lines: 10,
                    covered_lines: 8,
                    uncovered_ranges: vec![(9, 10)],
                },
                FileCoverage {
                    filename: "d:/proj/src/b.rs".to_string(),
                    total_lines: 10,
                    covered_lines: 2,
                    uncovered_ranges: vec![(3, 10)],
                },
                FileCoverage {
                    filename: "d:/proj/src/c.rs".to_string(),
                    total_lines: 10,
                    covered_lines: 9,
                    uncovered_ranges: vec![(10, 10)],
                },
            ],
        }
    }

    #[test]
    fn test_filter_and_sort_orders_by_uncovered_desc() {
        let cache = sample_cache();
        let rows = filter_and_sort(&cache, None, 1, 10, "d:/proj");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].0, "src/b.rs"); // 8 未覆盖 → 最前
        assert_eq!(rows[1].0, "src/a.rs"); // 2
        assert_eq!(rows[2].0, "src/c.rs"); // 1
    }

    #[test]
    fn test_filter_and_sort_path_and_min_lines() {
        let cache = sample_cache();
        // path 子串过滤
        let rows = filter_and_sort(&cache, Some("b.rs"), 1, 10, "d:/proj");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "src/b.rs");
        // min_lines 过滤掉小残留
        let rows = filter_and_sort(&cache, None, 5, 10, "d:/proj");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "src/b.rs");
        // limit 截断
        let rows = filter_and_sort(&cache, None, 1, 2, "d:/proj");
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_format_report_smoke() {
        let cache = sample_cache();
        let out = format_report(&cache, "d:/proj", None, 10, 1, " (test)");
        assert!(out.contains("63.3% lines covered (19 of 30)"), "{} ", out);
        assert!(out.contains("src/b.rs"));
        assert!(out.contains("3-10"));
        assert!(out.contains("Guidance"));
    }

    /// 验证解析器与 `cargo llvm-cov --json` 真实输出兼容。
    /// 报告文件由 llvm-cov 生成到 target/coverage_report_raw.json；不存在时跳过。
    #[test]
    fn test_parse_real_llvm_cov_report() {
        // cargo test 的 current_dir 是 crate 目录（src-tauri），报告在 workspace 根的 target/
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let candidates = [
            manifest.join("../target/coverage_report_raw.json"),
            manifest.join("target/coverage_report_raw.json"),
        ];
        let Some(path) = candidates.iter().find(|p| p.exists()) else {
            eprintln!("skip: no real report found under target/");
            return;
        };
        let content = std::fs::read_to_string(path).unwrap();
        let report: LlvmCovReport =
            parse_llvm_cov_output(&content).expect("real report should parse");
        let mut files = 0;
        let mut total: u64 = 0;
        let mut covered: u64 = 0;
        for data in &report.data {
            for f in &data.files {
                if is_src_file(&f.filename) {
                    let s = summarize_file(f);
                    total += s.total_lines;
                    covered += s.covered_lines;
                    files += 1;
                }
            }
        }
        assert!(files > 0, "real report should contain src files");
        assert!(covered < total, "real report should have uncovered lines");
        eprintln!(
            "real report: {} src files, {}/{} lines covered ({:.1}%)",
            files,
            covered,
            total,
            covered as f64 / total as f64 * 100.0
        );
    }

    /// 从真实报告生成缓存文件（target/coverage_report.json），供工具 scan 复用。
    /// 默认忽略（不污染日常测试），需要时手动跑：cargo test --lib coverage::tests::regenerate_cache -- --ignored
    #[test]
    #[ignore]
    fn regenerate_cache() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let raw = manifest.join("../target/coverage_report_raw.json");
        let content = std::fs::read_to_string(&raw).expect("raw report missing");
        let report = parse_llvm_cov_output(&content).expect("parse");
        let mut files: Vec<FileCoverage> = Vec::new();
        for data in &report.data {
            for f in &data.files {
                if is_src_file(&f.filename) {
                    files.push(summarize_file(f));
                }
            }
        }
        files.sort_by(|a, b| {
            b.uncovered_lines()
                .cmp(&a.uncovered_lines())
                .then(a.filename.cmp(&b.filename))
        });
        let total_lines: u64 = files.iter().map(|f| f.total_lines).sum();
        let covered_lines: u64 = files.iter().map(|f| f.covered_lines).sum();
        let cache = CoverageCache {
            scanned_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            total_lines,
            covered_lines,
            files,
        };
        let out = manifest.join("../target/coverage_report.json");
        std::fs::write(&out, serde_json::to_string_pretty(&cache).unwrap()).unwrap();
        eprintln!("cache written: {}", out.display());
        eprintln!(
            "{}",
            format_report(
                &cache,
                manifest.parent().unwrap().to_str().unwrap(),
                None,
                15,
                1,
                " (regenerated)"
            )
        );
    }
}
