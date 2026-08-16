//! web_browser tool: interactive browser automation via Chrome DevTools Protocol.
//!
//! Spawns a *persistent* headless Edge/Chrome instance with `--remote-debugging-port=0`
//! and drives it over a local WebSocket. Unlike `web_preview` (one-shot screenshot),
//! this tool keeps the page alive between calls so the agent can navigate, click,
//! type, re-screenshot and extract text in sequence:
//!
//! ```text
//! web_browser { action: "navigate", url: "http://localhost:1420" }
//! web_browser { action: "click", selector: "#login-btn" }
//! web_browser { action: "type", selector: "#name", text: "admin" }
//! web_browser { action: "screenshot", name: "login" }
//! web_browser { action: "get_text" }
//! web_browser { action: "close" }
//! ```
//!
//! # Recording & replay
//! Every successful navigate/click/type/screenshot is appended to the session
//! action log. `export_script` dumps it as a JSON array that can be replayed
//! later with `replay` (same session or a fresh one) to re-run a UI flow.
//!
//! # Visual regression
//! `screenshot` accepts an optional `name`; a later `screenshot_diff` with
//! that name (or an absolute file path) compares the current page against the
//! reference PNG pixel-by-pixel and reports the changed-pixel ratio plus a
//! diff image (changed pixels highlighted in red) for the agent to attach.
//!
//! Screenshots reuse the `[SCREENSHOT] <path>` marker so `PreviewImageHook`
//! attaches the PNG as a vision-capable message, exactly like `web_preview`.

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

use super::web_preview::{find_headless_browser, url_error};
use super::{Tool, ToolContext};

pub struct WebBrowser;

/// A live headless browser instance (created on first `navigate`).
struct BrowserSession {
    child: tokio::process::Child,
    page_ws_url: String,
    /// Actions recorded for `export_script` / `replay` (navigate/click/type/screenshot).
    action_log: Vec<Value>,
    /// Named screenshots kept for `screenshot_diff` (name → png path).
    screenshots: HashMap<String, PathBuf>,
}

static SESSION: OnceLock<Mutex<Option<BrowserSession>>> = OnceLock::new();

fn session() -> &'static Mutex<Option<BrowserSession>> {
    SESSION.get_or_init(|| Mutex::new(None))
}

/// Wait for the DevToolsActivePort file and return the debug port.
async fn wait_for_debug_port(profile_dir: &std::path::Path) -> Result<u16, String> {
    let port_file = profile_dir.join("DevToolsActivePort");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if tokio::time::Instant::now() > deadline {
            return Err("Timed out waiting for DevToolsActivePort (browser did not start)".into());
        }
        if let Ok(content) = std::fs::read_to_string(&port_file)
            && let Some(line) = content.lines().next()
            && let Ok(port) = line.trim().parse::<u16>()
        {
            return Ok(port);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Create a fresh page target via the CDP HTTP endpoint and return its WS URL.
async fn create_page_target(port: u16) -> Result<String, String> {
    let url = format!("http://127.0.0.1:{}/json/new?about:blank", port);
    let resp = reqwest::Client::new()
        .put(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("CDP /json/new failed: {}", e))?;
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("CDP /json/new bad response: {}", e))?;
    body["webSocketDebuggerUrl"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "CDP /json/new returned no webSocketDebuggerUrl".to_string())
}

/// Spawn the persistent headless browser if none is running.
async fn ensure_browser() -> Result<(), String> {
    {
        let guard = session()
            .lock()
            .map_err(|e| format!("Session lock poisoned: {}", e))?;
        if guard.is_some() {
            return Ok(());
        }
    } // guard dropped before any await
    let browser = find_headless_browser().ok_or_else(|| {
        "[ERROR] No headless browser found (searched Edge/Chrome install paths). \
         Install Microsoft Edge or Chrome to use web_browser."
            .to_string()
    })?;
    let profile_dir = std::env::temp_dir().join(format!("nee_cdp_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&profile_dir)
        .map_err(|e| format!("Cannot create CDP profile dir: {}", e))?;

    let child = tokio::process::Command::new(&browser)
        .args([
            "--headless=new",
            "--remote-debugging-port=0",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--hide-scrollbars",
            "--window-size=1440,900",
        ])
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg("about:blank")
        .spawn()
        .map_err(|e| format!("Failed to launch browser: {}", e))?;

    let port = wait_for_debug_port(&profile_dir).await?;
    let page_ws_url = create_page_target(port).await?;
    let mut guard = session()
        .lock()
        .map_err(|e| format!("Session lock poisoned: {}", e))?;
    if guard.is_none() {
        log::info!("[WebBrowser] Headless browser ready on port {}", port);
        *guard = Some(BrowserSession {
            child,
            page_ws_url,
            action_log: Vec::new(),
            screenshots: HashMap::new(),
        });
    }
    Ok(())
}

fn page_ws_url() -> Result<String, String> {
    let guard = session()
        .lock()
        .map_err(|e| format!("Session lock poisoned: {}", e))?;
    guard
        .as_ref()
        .map(|s| s.page_ws_url.clone())
        .ok_or_else(|| {
            "No browser session — call web_browser { action: 'navigate', url: ... } first"
                .to_string()
        })
}

/// Match a CDP response frame against the expected request id.
///
/// Returns `None` for non-matching (event) frames, `Some(Err)` for protocol
/// errors and `Some(Ok)` for the command result.
fn match_cdp_response(frame: &Value, id: u64) -> Option<Result<Value, String>> {
    if frame["id"].as_u64() != Some(id) {
        return None;
    }
    if let Some(err) = frame.get("error") {
        Some(Err(format!("CDP error: {}", err)))
    } else {
        Some(Ok(frame["result"].clone()))
    }
}

/// Send one CDP command over a fresh WebSocket connection and return the result.
async fn cdp_call(ws_url: &str, method: &str, params: Value) -> Result<Value, String> {
    let (mut ws, _) = connect_async(ws_url)
        .await
        .map_err(|e| format!("CDP connect failed: {}", e))?;
    let id = 1u64;
    let msg = json!({ "id": id, "method": method, "params": params });
    ws.send(Message::Text(msg.to_string()))
        .await
        .map_err(|e| format!("CDP send failed: {}", e))?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!("CDP {} timed out (10s)", method));
        }
        let item = tokio::time::timeout(remaining, ws.next())
            .await
            .map_err(|_| format!("CDP {} timed out (10s)", method))?
            .ok_or("CDP connection closed")?
            .map_err(|e| format!("CDP read error: {}", e))?;
        if let Message::Text(txt) = item {
            let v: Value =
                serde_json::from_str(&txt).map_err(|e| format!("CDP bad response JSON: {}", e))?;
            if let Some(result) = match_cdp_response(&v, id) {
                return result;
            }
            // Non-matching ids are protocol events — ignore them.
        }
    }
}

/// Evaluate a JS expression and return the (stringified) value.
async fn eval_js(ws_url: &str, expression: &str) -> Result<String, String> {
    let result = cdp_call(
        ws_url,
        "Runtime.evaluate",
        json!({ "expression": expression, "returnByValue": true }),
    )
    .await?;
    let value = &result["result"]["value"];
    if value.is_null() && result["result"]["type"].as_str() == Some("undefined") {
        Ok(String::new())
    } else {
        Ok(value.to_string())
    }
}

/// Build the JS snippet that locates a selector and returns its center coords.
fn selector_center_expr(selector: &str) -> String {
    format!(
        "(() => {{ const el = document.querySelector({}); if (!el) return null; \
         el.scrollIntoView({{ block: 'center' }}); const r = el.getBoundingClientRect(); \
         return {{ x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) }}; }})()",
        json!(selector)
    )
}

/// Build the JS snippet that focuses a selector.
fn selector_focus_expr(selector: &str) -> String {
    format!(
        "(() => {{ const el = document.querySelector({}); if (!el) return null; el.focus(); return true; }})()",
        json!(selector)
    )
}

async fn do_navigate(url: &str, wait_ms: u64) -> Result<String, String> {
    ensure_browser().await?;
    let ws_url = page_ws_url()?;
    cdp_call(&ws_url, "Page.navigate", json!({ "url": url })).await?;
    // Wait for readyState == complete (poll; give up at the caller's deadline).
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms.max(500));
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let state = eval_js(&ws_url, "document.readyState")
            .await
            .unwrap_or_default();
        if state.contains("complete") || tokio::time::Instant::now() > deadline {
            break;
        }
    }
    let title = eval_js(&ws_url, "document.title").await.unwrap_or_default();
    log::info!("[WebBrowser] Navigated to {} (title: {})", url, title);
    Ok(format!(
        "Navigated to {}.\nPage title: {}\nUse web_browser screenshot / get_text / click / type to interact with the page.",
        url,
        if title.is_empty() {
            "(empty)"
        } else {
            title.trim()
        }
    ))
}

async fn do_click(selector: &str) -> Result<String, String> {
    let ws_url = page_ws_url()?;
    let expr = selector_center_expr(selector);
    let pos: Value = cdp_call(
        &ws_url,
        "Runtime.evaluate",
        json!({ "expression": expr, "returnByValue": true }),
    )
    .await?;
    let center = &pos["result"]["value"];
    let (x, y) = match (center["x"].as_f64(), center["y"].as_f64()) {
        (Some(x), Some(y)) => (x.round() as i64, y.round() as i64),
        _ => {
            return Err(format!(
                "[ERROR] Selector '{}' not found on the page (checked querySelector). \
                 Verify the selector with web_browser get_text first.",
                selector
            ));
        }
    };
    let mouse = |mtype: &str| {
        cdp_call(
            &ws_url,
            "Input.dispatchMouseEvent",
            json!({
                "type": mtype, "x": x, "y": y,
                "button": "left", "clickCount": 1, "buttons": if mtype == "mousePressed" { 1 } else { 0 }
            }),
        )
    };
    mouse("mousePressed").await?;
    mouse("mouseReleased").await?;
    log::info!("[WebBrowser] Clicked '{}' at ({}, {})", selector, x, y);
    Ok(format!("Clicked '{}' at ({}, {}).", selector, x, y))
}

async fn do_type(selector: &str, text: &str) -> Result<String, String> {
    let ws_url = page_ws_url()?;
    let expr = selector_focus_expr(selector);
    let focused: Value = cdp_call(
        &ws_url,
        "Runtime.evaluate",
        json!({ "expression": expr, "returnByValue": true }),
    )
    .await?;
    if focused["result"]["value"] != json!(true) {
        return Err(format!(
            "[ERROR] Selector '{}' not found on the page — cannot focus for typing.",
            selector
        ));
    }
    cdp_call(&ws_url, "Input.insertText", json!({ "text": text })).await?;
    log::info!(
        "[WebBrowser] Typed {} chars into '{}'",
        text.chars().count(),
        selector
    );
    Ok(format!(
        "Typed {} characters into '{}'.",
        text.chars().count(),
        selector
    ))
}

/// Capture the current page. `name` (optional) registers the PNG for
/// later `screenshot_diff` comparisons.
async fn do_screenshot(name: Option<&str>) -> Result<String, String> {
    let ws_url = page_ws_url()?;
    let result = cdp_call(
        &ws_url,
        "Page.captureScreenshot",
        json!({ "format": "png" }),
    )
    .await?;
    let b64 = result["data"]
        .as_str()
        .ok_or_else(|| "CDP captureScreenshot returned no data".to_string())?;
    let bytes = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("Screenshot base64 decode failed: {}", e))?
    };
    let out_dir = std::env::temp_dir().join("neocoder_previews");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("Cannot create preview dir: {}", e))?;
    let file_name = match name {
        Some(n) if !n.trim().is_empty() => format!("browser_{}.png", n.trim()),
        _ => format!("browser_{}.png", chrono::Utc::now().timestamp_millis()),
    };
    let out_path = out_dir.join(&file_name);
    std::fs::write(&out_path, &bytes).map_err(|e| format!("Cannot write screenshot: {}", e))?;

    if let Some(n) = name.filter(|n| !n.trim().is_empty())
        && let Ok(mut guard) = session().lock()
        && let Some(s) = guard.as_mut()
    {
        s.screenshots.insert(n.trim().to_string(), out_path.clone());
    }

    log::info!("[WebBrowser] Screenshot saved: {}", out_path.display());
    let hint = if name.is_some() {
        "\nUse web_browser { action: 'screenshot_diff', reference: '<name>' } to compare against a later state."
    } else {
        ""
    };
    Ok(format!(
        "[SCREENSHOT] {}\nInteractive screenshot captured via web_browser. The image is attached for review.{}",
        out_path.to_string_lossy(),
        hint
    ))
}

/// Pixel-wise PNG diff. Returns `(changed_ratio, diff_png_bytes)` where the
/// diff image marks changed pixels in red on a dark background.
fn pixel_diff_png(reference: &[u8], current: &[u8]) -> Result<(f64, Vec<u8>), String> {
    use image::{ImageFormat, Rgba, RgbaImage};
    let a = image::load_from_memory(reference)
        .map_err(|e| format!("Reference PNG decode failed: {}", e))?
        .to_rgba8();
    let b = image::load_from_memory(current)
        .map_err(|e| format!("Current PNG decode failed: {}", e))?
        .to_rgba8();
    let (wa, ha) = a.dimensions();
    let (wb, hb) = b.dimensions();
    let w = wa.min(wb);
    let h = ha.min(hb);

    let mut diff = RgbaImage::new(w, h);
    let mut changed: u64 = 0;
    for y in 0..h {
        for x in 0..w {
            let pa = a.get_pixel(x, y).0;
            let pb = b.get_pixel(x, y).0;
            let delta: i64 = (pa[0] as i64 - pb[0] as i64).abs()
                + (pa[1] as i64 - pb[1] as i64).abs()
                + (pa[2] as i64 - pb[2] as i64).abs();
            if delta > 60 {
                changed += 1;
                diff.put_pixel(x, y, Rgba([255, 60, 70, 255]));
            } else {
                diff.put_pixel(x, y, Rgba([22, 22, 26, 255]));
            }
        }
    }
    let total = w as u64 * h as u64;
    let ratio = if total == 0 {
        1.0
    } else {
        changed as f64 / total as f64
    };
    let mut out = Vec::new();
    diff.write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|e| format!("Diff PNG encode failed: {}", e))?;
    Ok((ratio, out))
}

/// Compare the current page against a reference screenshot (named via
/// `screenshot { name }` or an absolute file path) and report the changed
/// pixel ratio plus a highlighted diff image.
async fn do_screenshot_diff(reference: &str) -> Result<String, String> {
    if reference.trim().is_empty() {
        return Err("[ERROR] screenshot_diff requires a 'reference' argument: \
                    a name from a previous screenshot call (e.g. screenshot { name: 'login' }) \
                    or an absolute PNG path"
            .to_string());
    }

    // Resolve the reference PNG path (named screenshot > absolute path)
    let ref_path: Option<PathBuf> = {
        let named = session()
            .lock()
            .map_err(|e| format!("Session lock poisoned: {}", e))?
            .as_ref()
            .and_then(|s| s.screenshots.get(reference.trim()).cloned());
        match named {
            Some(p) => Some(p),
            None => {
                let p = Path::new(reference.trim()).to_path_buf();
                if p.is_file() { Some(p) } else { None }
            }
        }
    };
    let ref_path = ref_path.ok_or_else(|| {
        format!(
            "[ERROR] Reference '{}' not found: no screenshot with that name in this session \
         and no file at that path. Take one first with screenshot {{ name: '{}' }}.",
            reference,
            reference.trim()
        )
    })?;

    let reference_bytes =
        std::fs::read(&ref_path).map_err(|e| format!("Cannot read reference PNG: {}", e))?;

    // Capture the current page
    let ws_url = page_ws_url()?;
    let result = cdp_call(
        &ws_url,
        "Page.captureScreenshot",
        json!({ "format": "png" }),
    )
    .await?;
    let b64 = result["data"]
        .as_str()
        .ok_or_else(|| "CDP captureScreenshot returned no data".to_string())?;
    let current = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("Screenshot base64 decode failed: {}", e))?
    };

    let (ratio, diff_png) = pixel_diff_png(&reference_bytes, &current)?;

    let out_dir = std::env::temp_dir().join("neocoder_previews");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("Cannot create preview dir: {}", e))?;
    let out_path = out_dir.join(format!(
        "diff_{}_{}.png",
        reference.trim(),
        chrono::Utc::now().timestamp_millis()
    ));
    std::fs::write(&out_path, &diff_png).map_err(|e| format!("Cannot write diff image: {}", e))?;

    let verdict = if ratio < 0.001 {
        "VISUALLY IDENTICAL"
    } else if ratio < 0.05 {
        "MINOR CHANGES"
    } else {
        "SIGNIFICANT CHANGES"
    };
    log::info!(
        "[WebBrowser] Screenshot diff vs '{}': {:.2}% changed -> {}",
        reference,
        ratio * 100.0,
        out_path.display()
    );
    Ok(format!(
        "[SCREENSHOT] {}\nVisual diff vs '{}': {:.2}% of pixels changed ({verdict}). \
         Red pixels mark differences; reference image: {}.",
        out_path.to_string_lossy(),
        reference,
        ratio * 100.0,
        ref_path.display()
    ))
}

/// Dump the recorded action log as a JSON script for `replay`.
async fn do_export_script() -> Result<String, String> {
    let log = {
        let guard = session()
            .lock()
            .map_err(|e| format!("Session lock poisoned: {}", e))?;
        guard
            .as_ref()
            .map(|s| s.action_log.clone())
            .unwrap_or_default()
    };
    if log.is_empty() {
        return Ok(
            "No actions recorded yet. Run navigate/click/type/screenshot first.".to_string(),
        );
    }
    let script = serde_json::to_string_pretty(&log)
        .map_err(|e| format!("Failed to serialize action log: {}", e))?;
    log::info!("[WebBrowser] Exported {} recorded actions", log.len());
    Ok(format!(
        "Recorded {} actions. Replay them with:\nweb_browser {{ action: 'replay', script: ... }}\n\n{}",
        log.len(),
        script
    ))
}

/// Replay a recorded action script (JSON array of { action, ... } objects).
async fn do_replay(script: &str) -> Result<String, String> {
    let parsed: Value = if script.trim_start().starts_with('[') {
        serde_json::from_str(script).map_err(|e| format!("Invalid replay script JSON: {}", e))?
    } else {
        serde_json::from_str(script).map_err(|e| format!("Invalid replay script JSON: {}", e))?
    };
    let actions = parsed
        .as_array()
        .ok_or_else(|| "Replay script must be a JSON array of action objects".to_string())?;

    let mut results = Vec::new();
    for (i, action) in actions.iter().enumerate() {
        let a = action.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let out = match a {
            "navigate" => {
                let url = action.get("url").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(err) = url_error(url) {
                    Err(err)
                } else {
                    let wait_ms = action
                        .get("wait_ms")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(1500);
                    do_navigate(url, wait_ms).await
                }
            }
            "click" => {
                let selector = action
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if selector.is_empty() {
                    Err("[ERROR] replay click requires a 'selector'".to_string())
                } else {
                    do_click(selector).await
                }
            }
            "type" => {
                let selector = action
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let text = action.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if selector.is_empty() || text.is_empty() {
                    Err("[ERROR] replay type requires 'selector' and 'text'".to_string())
                } else {
                    do_type(selector, text).await
                }
            }
            "screenshot" => {
                let name = action.get("name").and_then(|v| v.as_str());
                do_screenshot(name).await
            }
            "get_text" => do_get_text().await,
            "wait" => {
                let ms = action.get("ms").and_then(|v| v.as_u64()).unwrap_or(500);
                tokio::time::sleep(Duration::from_millis(ms)).await;
                Ok(format!("Waited {} ms", ms))
            }
            other => Err(format!("[ERROR] Unknown replay action '{}'", other)),
        };
        match out {
            Ok(text) => {
                results.push(format!("[{}] {}: {}", i + 1, a, first_line(&text)));
            }
            Err(e) => {
                results.push(format!("[{}] {}: FAILED — {}", i + 1, a, e));
            }
        }
    }

    let ok = results.iter().filter(|r| !r.contains("FAILED")).count();
    let failed = results.len() - ok;
    let joined = results.join("\n");
    Ok(format!(
        "Replayed {} actions ({} ok, {} failed):\n{}",
        results.len(),
        ok,
        failed,
        joined
    ))
}

/// First non-empty line of a (possibly multi-line) tool output.
fn first_line(text: &str) -> &str {
    text.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("(no output)")
}

/// Record a successful recordable action into the session log.
fn log_action(action: &Value) {
    let a = action.get("action").and_then(|v| v.as_str()).unwrap_or("");
    if !matches!(a, "navigate" | "click" | "type" | "screenshot") {
        return;
    }
    if let Ok(mut guard) = session().lock()
        && let Some(s) = guard.as_mut()
    {
        s.action_log.push(action.clone());
    }
}

async fn do_get_text() -> Result<String, String> {
    let ws_url = page_ws_url()?;
    let text = eval_js(&ws_url, "document.body ? document.body.innerText : ''").await?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok("(page has no visible text)".to_string());
    }
    const MAX: usize = 8000;
    if trimmed.chars().count() > MAX {
        let cut: String = trimmed.chars().take(MAX).collect();
        return Ok(format!(
            "{}\n... ({} more chars omitted)",
            cut,
            trimmed.chars().count() - MAX
        ));
    }
    Ok(trimmed.to_string())
}

async fn do_close() -> Result<String, String> {
    let taken = {
        let mut guard = session()
            .lock()
            .map_err(|e| format!("Session lock poisoned: {}", e))?;
        guard.take()
    };
    if let Some(mut s) = taken {
        let _ = s.child.kill().await;
        let _ = s.child.wait().await;
    }
    Ok(
        "Browser session closed. The next web_browser navigate will start a fresh instance."
            .to_string(),
    )
}

#[async_trait]
impl Tool for WebBrowser {
    fn name(&self) -> &str {
        "web_browser"
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> String {
        let action = args["action"].as_str().unwrap_or("").trim().to_string();
        let result = match action.as_str() {
            "navigate" => {
                let url = args["url"].as_str().unwrap_or("");
                if let Some(err) = url_error(url) {
                    Err(err)
                } else {
                    let wait_ms = args["wait_ms"].as_u64().unwrap_or(1500);
                    do_navigate(url, wait_ms).await
                }
            }
            "click" => {
                let selector = args["selector"].as_str().unwrap_or("");
                if selector.is_empty() {
                    Err("[ERROR] web_browser click requires a 'selector' argument (CSS selector, e.g. '#submit' or 'button.primary')".to_string())
                } else {
                    do_click(selector).await
                }
            }
            "type" => {
                let selector = args["selector"].as_str().unwrap_or("");
                let text = args["text"].as_str().unwrap_or("");
                if selector.is_empty() || text.is_empty() {
                    Err("[ERROR] web_browser type requires 'selector' and 'text' arguments (e.g. web_browser { action: 'type', selector: '#name', text: 'admin' })".to_string())
                } else {
                    do_type(selector, text).await
                }
            }
            "screenshot" => {
                let name = args.get("name").and_then(|v| v.as_str());
                do_screenshot(name).await
            }
            "screenshot_diff" => {
                let reference = args["reference"].as_str().unwrap_or("");
                do_screenshot_diff(reference).await
            }
            "get_text" => do_get_text().await,
            "export_script" => do_export_script().await,
            "replay" => {
                let script = args["script"].as_str().unwrap_or("");
                do_replay(script).await
            }
            "close" => do_close().await,
            other => Err(format!(
                "[ERROR] Unknown web_browser action '{}'. Valid actions: navigate, click, type, screenshot, \
                 screenshot_diff, get_text, export_script, replay, close.",
                other
            )),
        };
        match result {
            Ok(text) => {
                log_action(&args);
                text
            }
            Err(e) => e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_expressions_escape_quotes() {
        // A selector containing quotes must survive JSON embedding
        let expr = selector_center_expr("div[data-name=\"a'b\"]");
        assert!(
            expr.contains("\\\"a'b\\\"") || expr.contains("\\\"a'b\\\""),
            "{}",
            expr
        );
        // The generated snippet is syntactically valid JS
        assert!(expr.starts_with("(() => {"));
        assert!(expr.ends_with("})()"));
        let focus = selector_focus_expr("#login");
        assert!(focus.contains("querySelector(\"#login\")"), "{}", focus);
    }

    #[test]
    fn cdp_result_extraction_matches_id() {
        // Response with matching id → result
        let v = json!({ "id": 1, "result": { "foo": "bar" } });
        let m = match_cdp_response(&v, 1)
            .expect("matching id")
            .expect("no error");
        assert_eq!(m["foo"], "bar");
        // Event (different id) → None
        let v = json!({ "id": 99, "method": "Page.loadEventFired" });
        assert!(match_cdp_response(&v, 1).is_none());
        // Error payload → Err with message
        let v = json!({ "id": 1, "error": { "message": "boom", "code": -32000 } });
        let err = match_cdp_response(&v, 1)
            .expect("matching id")
            .expect_err("error payload");
        assert!(err.contains("boom"), "{}", err);
    }

    #[test]
    fn pixel_diff_detects_changes() {
        use image::{Rgba, RgbaImage};
        let make = |color: [u8; 4]| -> Vec<u8> {
            let mut img = RgbaImage::new(8, 8);
            for p in img.pixels_mut() {
                *p = Rgba(color);
            }
            let mut buf = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
                .unwrap();
            buf
        };

        // Identical images → no changed pixels
        let a = make([10, 20, 30, 255]);
        let (ratio, diff) = pixel_diff_png(&a, &a).expect("diff identical");
        assert_eq!(ratio, 0.0);
        assert!(!diff.is_empty());

        // One pixel differs → small but nonzero ratio
        let mut b_img = RgbaImage::new(8, 8);
        for p in b_img.pixels_mut() {
            *p = Rgba([10, 20, 30, 255]);
        }
        b_img.put_pixel(0, 0, Rgba([250, 250, 250, 255]));
        let mut b_buf = Vec::new();
        b_img
            .write_to(
                &mut std::io::Cursor::new(&mut b_buf),
                image::ImageFormat::Png,
            )
            .unwrap();
        let (ratio2, _) = pixel_diff_png(&a, &b_buf).expect("diff one pixel");
        assert!(ratio2 > 0.0 && ratio2 < 0.1, "ratio {}", ratio2);

        // Invalid PNG → Err, not panic
        assert!(pixel_diff_png(b"not a png", &a).is_err());
    }
}
