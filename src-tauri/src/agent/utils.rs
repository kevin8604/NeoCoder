use std::path::{Path, PathBuf};

/// 在文件遍历和 glob 搜索中跳过的目录
pub const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    "__pycache__",
    ".svn",
];

/// 检查路径中是否包含应跳过的目录
pub fn is_skipped_dir(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .map(|s| SKIP_DIRS.contains(&s))
            .unwrap_or(false)
    })
}

/// Normalize Cygwin/MSYS2 paths to Windows format.
/// - `/cygdrive/d/foo/bar` → `D:\foo\bar`
/// - `/d/foo/bar` (MSYS2) → `D:\foo\bar`
/// - Other paths returned unchanged
#[cfg(target_os = "windows")]
pub fn normalize_cygwin_path(input: &str) -> String {
    // Case 1: /cygdrive/X/... → X:\...
    if let Some(rest) = input
        .strip_prefix("/cygdrive/")
        .or_else(|| input.strip_prefix("\\cygdrive\\"))
        && let Some(drive) = rest.chars().next()
        && drive.is_ascii_alphabetic()
    {
        let remainder = &rest[1..]; // after drive letter
        let win_path = format!(
            "{}:{}",
            drive.to_ascii_uppercase(),
            remainder.replace('/', "\\")
        );
        return win_path;
    }
    // Case 2: /X/foo (MSYS2 style, single letter at root)
    let trimmed = input.strip_prefix('/').or_else(|| input.strip_prefix('\\'));
    if let Some(rest) = trimmed
        && rest.len() >= 2
    {
        let mut chars = rest.chars();
        let maybe_drive = chars.next().unwrap();
        let next = chars.next().unwrap();
        if maybe_drive.is_ascii_alphabetic() && (next == '/' || next == '\\') {
            let win_path = format!(
                "{}:{}",
                maybe_drive.to_ascii_uppercase(),
                rest[1..].replace('/', "\\")
            );
            return win_path;
        }
    }
    input.to_string()
}

#[cfg(not(target_os = "windows"))]
pub fn normalize_cygwin_path(input: &str) -> String {
    input.to_string()
}

/// 解析工具操作的目标路径
/// relative 为相对路径时，相对于 project_path；否则原样返回
pub fn resolve_path(base: Option<&str>, relative: &str) -> PathBuf {
    // Normalize Cygwin/MSYS2 paths before processing
    let normalized = normalize_cygwin_path(relative);
    let relative = normalized.as_str();

    if relative.is_empty() {
        return PathBuf::from(base.unwrap_or("."));
    }
    let p = Path::new(relative);
    if p.is_absolute() {
        p.to_path_buf()
    } else if let Some(b) = base {
        PathBuf::from(b).join(relative)
    } else {
        PathBuf::from(relative)
    }
}

/// 创建 HTTP 客户端（带超时和 UA）
pub fn http_client(timeout_secs: u64, user_agent: &str) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .user_agent(user_agent)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))
}

/// 简单 HTML 标签剥离
pub fn strip_html(input: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Safely truncate a string at a UTF-8 character boundary.
/// If `max_bytes` falls inside a multi-byte character, the truncation
/// is moved to the start of that character to avoid panics.
pub fn safe_truncate(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Find the nearest char boundary <= max_bytes
    let mut idx = max_bytes;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    &s[..idx]
}
