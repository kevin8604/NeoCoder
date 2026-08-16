use super::{Tool, ToolContext};
use crate::agent::utils;

pub struct WebFetch;

#[async_trait::async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &str {
        "web_fetch"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let url = args["url"].as_str().unwrap_or("");
        if url.is_empty() {
            return "Error: URL is required".to_string();
        }

        // Sandbox: check URL (SSRF protection + domain whitelist)
        if let Err(e) = ctx.sandbox.check_url(url) {
            return format!("Error: Sandbox blocked: {}", e);
        }

        match utils::http_client(30, "Mozilla/5.0 (compatible; NeoCoder/1.0)") {
            Ok(cl) => match cl.get(url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    match resp.text().await {
                        Ok(body) => {
                            let max_len = 200 * 1024;
                            let text = utils::strip_html(&body);
                            let display = if text.len() > max_len {
                                // Use char-aware truncation to avoid UTF-8 boundary panics
                                let truncated: String = text.chars().take(max_len).collect();
                                format!(
                                    "{}...\n\n[Content truncated at {}KB. Full page size: {}KB]",
                                    truncated,
                                    max_len / 1024,
                                    text.len() / 1024
                                )
                            } else {
                                text
                            };
                            format!("Fetched {} [Status: {}]:\n\n{}", url, status, display)
                        }
                        Err(e) => format!("Error reading response from {}: {}", url, e),
                    }
                }
                Err(e) => format!("Error: Request to {} failed: {}", url, e),
            },
            Err(e) => format!("Error: {}", e),
        }
    }
}
