//! Web preview tool: screenshots a running web app via headless Edge/Chrome
//! so the agent can visually verify UI changes.
//!
//! The tool itself only saves the PNG and returns a `[SCREENSHOT] <path>`
//! marker. `PreviewImageHook` (agent/hooks.rs) picks up the marker, converts
//! the file to a base64 data URL and injects it as a vision-capable message
//! into the LLM context.

use async_trait::async_trait;

use super::{Tool, ToolContext};

pub struct WebPreview;

/// Common install paths for headless-capable browsers on Windows.
const BROWSER_CANDIDATES: &[&str] = &[
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
];

/// Locate a headless-capable browser (Edge preferred, Chrome fallback).
pub(crate) fn find_headless_browser() -> Option<std::path::PathBuf> {
    for c in BROWSER_CANDIDATES {
        let p = std::path::Path::new(c);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }
    // PATH lookup (unusual installs / non-Windows)
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(';') {
            for name in ["msedge.exe", "chrome.exe", "msedge", "google-chrome"] {
                let p = std::path::Path::new(dir).join(name);
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Return the error message for an invalid URL, or None when acceptable.
pub(crate) fn url_error(url: &str) -> Option<String> {
    if url.is_empty() {
        return Some(
            "[ERROR] web_preview requires a 'url' argument (e.g. web_preview { url: 'http://localhost:1420' })".to_string(),
        );
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Some(format!(
            "[ERROR] Invalid url '{}': must start with http:// or https:// (start the dev server first via run_terminal_command)",
            url
        ));
    }
    None
}

#[async_trait]
impl Tool for WebPreview {
    fn name(&self) -> &str {
        "web_preview"
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> String {
        let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(err) = url_error(url) {
            return err;
        }
        let wait_ms = args
            .get("wait_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(1500);

        let Some(browser) = find_headless_browser() else {
            return "[ERROR] No headless browser found (searched Edge/Chrome install paths). \
                    Install Microsoft Edge or Chrome to use web_preview."
                .to_string();
        };

        // Output file in a temp dir (never pollutes the project)
        let stamp = chrono::Utc::now().timestamp_millis();
        let out_dir = std::env::temp_dir().join("neecoder_previews");
        if let Err(e) = std::fs::create_dir_all(&out_dir) {
            return format!("[ERROR] Cannot create preview dir {}: {}", out_dir.display(), e);
        }
        let out_path = out_dir.join(format!("preview_{}.png", stamp));
        let out_str = out_path.to_string_lossy().to_string();

        let mut cmd = tokio::process::Command::new(&browser);
        cmd.args([
            "--headless=new",
            "--disable-gpu",
            "--hide-scrollbars",
            "--no-first-run",
            "--no-default-browser-check",
            "--window-size=1440,900",
        ])
        .arg(format!("--screenshot={}", out_str))
        .arg("--virtual-time-budget")
        .arg(wait_ms.to_string())
        .arg(url);

        let result = tokio::time::timeout(std::time::Duration::from_secs(45), cmd.output()).await;

        match result {
            Ok(Ok(out)) => {
                if out.status.success() && out_path.exists() {
                    log::info!("[WebPreview] Screenshot saved: {}", out_str);
                    format!(
                        "[SCREENSHOT] {}\nVisual preview captured for {}. The image is attached for review.\n\
                         If the screenshot is blank, the dev server may not be serving this URL — check with run_terminal_command.",
                        out_str, url
                    )
                } else {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    format!(
                        "[ERROR] Screenshot failed (exit {:?}): {}",
                        out.status.code(),
                        stderr.chars().take(500).collect::<String>()
                    )
                }
            }
            Ok(Err(e)) => format!("[ERROR] Failed to run browser: {}", e),
            Err(_) => "[TIMEOUT] Screenshot timed out after 45s. The dev server may be down.".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_urls() {
        assert!(url_error("").is_some());
        assert!(url_error("ftp://x").is_some());
        assert!(url_error("localhost:1420").is_some());
        assert!(url_error("http://localhost:1420").is_none());
        assert!(url_error("https://example.com").is_none());
    }
}
