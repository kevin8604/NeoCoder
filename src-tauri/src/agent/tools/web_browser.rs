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
//! web_browser { action: "screenshot" }
//! web_browser { action: "get_text" }
//! web_browser { action: "close" }
//! ```
//!
//! Screenshots reuse the `[SCREENSHOT] <path>` marker so `PreviewImageHook`
//! attaches the PNG as a vision-capable message, exactly like `web_preview`.

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
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
        if let Ok(content) = std::fs::read_to_string(&port_file) {
            if let Some(line) = content.lines().next() {
                if let Ok(port) = line.trim().parse::<u16>() {
                    return Ok(port);
                }
            }
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
        *guard = Some(BrowserSession { child, page_ws_url });
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
        .ok_or_else(|| "No browser session — call web_browser { action: 'navigate', url: ... } first".to_string())
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
    ws.send(Message::Text(msg.to_string().into()))
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
            let v: Value = serde_json::from_str(&txt)
                .map_err(|e| format!("CDP bad response JSON: {}", e))?;
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
        if title.is_empty() { "(empty)" } else { title.trim() }
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
            ))
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
    log::info!("[WebBrowser] Typed {} chars into '{}'", text.chars().count(), selector);
    Ok(format!("Typed {} characters into '{}'.", text.chars().count(), selector))
}

async fn do_screenshot() -> Result<String, String> {
    let ws_url = page_ws_url()?;
    let result = cdp_call(&ws_url, "Page.captureScreenshot", json!({ "format": "png" })).await?;
    let b64 = result["data"]
        .as_str()
        .ok_or_else(|| "CDP captureScreenshot returned no data".to_string())?;
    let bytes = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("Screenshot base64 decode failed: {}", e))?
    };
    let out_dir = std::env::temp_dir().join("neecoder_previews");
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| format!("Cannot create preview dir: {}", e))?;
    let out_path = out_dir.join(format!(
        "browser_{}.png",
        chrono::Utc::now().timestamp_millis()
    ));
    std::fs::write(&out_path, &bytes)
        .map_err(|e| format!("Cannot write screenshot: {}", e))?;
    log::info!("[WebBrowser] Screenshot saved: {}", out_path.display());
    Ok(format!(
        "[SCREENSHOT] {}\nInteractive screenshot captured via web_browser. The image is attached for review.",
        out_path.to_string_lossy()
    ))
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
        return Ok(format!("{}\n... ({} more chars omitted)", cut, trimmed.chars().count() - MAX));
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
    Ok("Browser session closed. The next web_browser navigate will start a fresh instance.".to_string())
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
            "screenshot" => do_screenshot().await,
            "get_text" => do_get_text().await,
            "close" => do_close().await,
            other => Err(format!(
                "[ERROR] Unknown web_browser action '{}'. Valid actions: navigate, click, type, screenshot, get_text, close.",
                other
            )),
        };
        match result {
            Ok(text) => text,
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
        assert!(expr.contains("\\\"a'b\\\"") || expr.contains("\\\"a'b\\\""), "{}", expr);
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
        let m = match_cdp_response(&v, 1).expect("matching id").expect("no error");
        assert_eq!(m["foo"], "bar");
        // Event (different id) → None
        let v = json!({ "id": 99, "method": "Page.loadEventFired" });
        assert!(match_cdp_response(&v, 1).is_none());
        // Error payload → Err with message
        let v = json!({ "id": 1, "error": { "message": "boom", "code": -32000 } });
        let err = match_cdp_response(&v, 1).expect("matching id").expect_err("error payload");
        assert!(err.contains("boom"), "{}", err);
    }
}
