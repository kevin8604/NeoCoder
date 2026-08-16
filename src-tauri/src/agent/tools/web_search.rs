use super::{Tool, ToolContext};
use crate::agent::utils;

pub struct WebSearch;

#[async_trait::async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &str {
        "web_search"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let query = args["query"].as_str().unwrap_or("");
        if query.is_empty() {
            return "Error: search query is required".to_string();
        }
        let max_results = args["max_results"].as_u64().unwrap_or(5) as usize;

        // Use Tavily API if key is configured
        if !ctx.tavily_api_key.is_empty() {
            return tavily_search(&ctx.tavily_api_key, query, max_results).await;
        }

        // Fallback: DuckDuckGo Lite (no API key needed)
        fallback_ddg_lite(query, max_results).await
    }
}

/// Tavily Search API: https://docs.tavily.com/docs/rest-api/api-reference
async fn tavily_search(api_key: &str, query: &str, max_results: usize) -> String {
    let url = "https://api.tavily.com/search";

    let body = serde_json::json!({
        "api_key": api_key,
        "query": query,
        "max_results": max_results,
        "search_depth": "basic",
        "include_answer": false,
        "include_raw_content": false,
    });

    match utils::http_client(30, "NeoCoder/1.0") {
        Ok(client) => match client.post(url).json(&body).send().await {
            Ok(resp) => {
                let status = resp.status();
                match resp.text().await {
                    Ok(text) => {
                        if !status.is_success() {
                            return format!(
                                "Error: Tavily API returned status {}: {}",
                                status,
                                text.chars().take(200).collect::<String>()
                            );
                        }
                        parse_tavily_response(&text, query)
                    }
                    Err(e) => format!("Error: Failed to read Tavily response: {}", e),
                }
            }
            Err(e) => format!("Error: Tavily search request failed: {}", e),
        },
        Err(e) => format!("Error: Failed to create HTTP client: {}", e),
    }
}

fn parse_tavily_response(body: &str, query: &str) -> String {
    let json: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return format!("Error: Failed to parse Tavily response: {}", e),
    };

    let results = json["results"].as_array();
    match results {
        Some(arr) if !arr.is_empty() => {
            let mut output = format!("Web search results for '{}':\n\n", query);
            for (i, r) in arr.iter().enumerate() {
                let title = r["title"].as_str().unwrap_or("Untitled");
                let url = r["url"].as_str().unwrap_or("");
                let content = r["content"].as_str().unwrap_or("");
                output.push_str(&format!(
                    "{}. {}\n   URL: {}\n   {}\n\n",
                    i + 1,
                    title,
                    url,
                    content
                ));
            }
            output
        }
        _ => format!("No web search results found for: '{}'", query),
    }
}

/// Fallback: DuckDuckGo Lite (no API key needed, may be unstable in China)
async fn fallback_ddg_lite(query: &str, max_results: usize) -> String {
    let url = format!(
        "https://lite.duckduckgo.com/lite/?q={}",
        urlencoding::encode(query)
    );

    match utils::http_client(15, "Mozilla/5.0") {
        Ok(cl) => match cl.get(&url).send().await {
            Ok(resp) => match resp.text().await {
                Ok(body) => parse_ddg_lite(&body, query, max_results),
                Err(e) => format!("Error: Failed to read search response: {}", e),
            },
            Err(e) => format!("Error: Search request failed: {}", e),
        },
        Err(e) => format!("Error: {}", e),
    }
}

fn parse_ddg_lite(body: &str, query: &str, max_results: usize) -> String {
    let mut results: Vec<String> = Vec::new();
    let mut in_result = false;
    let mut current_title = String::new();
    let mut current_snippet = String::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<a rel=\"nofollow\"") && results.len() < max_results {
            if !current_title.is_empty() {
                results.push(format!(
                    "{}. {}\n   {}",
                    results.len() + 1,
                    current_title,
                    current_snippet
                ));
            }
            in_result = true;
            current_title = trimmed
                .replace("<a rel=\"nofollow\"", "")
                .replace("class=\"result-link\"", "")
                .replace("href=", "")
                .trim()
                .trim_matches('"')
                .to_string();
            current_snippet = String::new();
        } else if in_result && (trimmed.starts_with("<td") || trimmed.starts_with("<span")) {
            current_snippet = crate::agent::utils::strip_html(trimmed);
            in_result = false;
        }
    }
    if results.is_empty() {
        format!("No web search results found for: '{}'", query)
    } else {
        let mut output = format!("Web search results for '{}':\n", query);
        for r in &results {
            output.push_str(&format!("{}\n\n", r));
        }
        output
    }
}
