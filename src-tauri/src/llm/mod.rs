use crate::config::LlmProvider;
use futures_util::StreamExt;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub mod health;
pub mod router;

pub use router::{LlmRoute, LlmRouter, TaskType};

/// Shared cancellation flag for streaming requests
pub type CancelFlag = Arc<AtomicBool>;

/// Streaming completion result
#[derive(Debug, Clone, Serialize)]
pub struct LlmStreamEvent {
    pub token: String,
    pub done: bool,
    /// Whether this event contains thinking content (Extended Thinking)
    pub thinking: bool,
}

/// FIM request parameters
pub struct FimRequest {
    pub model: String,
    pub prompt: String,
    pub system_prompt: String,
    pub max_tokens: u32,
    /// Sampling temperature (0.0 = deterministic). Default 0.2.
    pub temperature: f32,
}

/// Chat request parameters
pub struct ChatRequestParams {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub system: String,
    pub max_tokens: u32,
    pub temperature: f32,
    /// Enable Claude Extended Thinking (Anthropic only)
    pub thinking_enabled: bool,
    /// Thinking budget in tokens (1024-10000)
    pub thinking_budget: u32,
}

/// A single chat message for API calls.
///
/// Supports both text-only and multimodal (text + images) content.
/// When `images` is None, `content` serializes as a plain string.
/// When `images` is Some, `content` serializes as an array of content parts.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// Optional images as base64 data URLs (e.g., "data:image/png;base64,...")
    pub images: Option<Vec<ImageContent>>,
    pub tool_calls: Option<serde_json::Value>,
    pub tool_call_id: Option<String>,
}

/// Image content for vision-capable models.
#[derive(Debug, Clone)]
pub struct ImageContent {
    /// Base64-encoded data URL or HTTP(S) URL
    pub url: String,
    /// Detail level: "low", "high", or "auto" (default)
    pub detail: Option<String>,
}

impl Serialize for ChatMessage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let has_images = self.images.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
        let field_count = 2usize
            + if has_images { 0 } else { 0 }  // content is always present
            + self.tool_calls.is_some() as usize
            + self.tool_call_id.is_some() as usize;
        let mut s = serializer.serialize_struct("ChatMessage", field_count)?;
        s.serialize_field("role", &self.role)?;
        if has_images {
            let mut parts: Vec<serde_json::Value> = vec![serde_json::json!({"type": "text", "text": self.content})];
            for img in self.images.as_ref().unwrap() {
                let mut image_url = serde_json::json!({"url": img.url});
                if let Some(ref detail) = img.detail {
                    image_url["detail"] = serde_json::Value::String(detail.clone());
                }
                parts.push(serde_json::json!({"type": "image_url", "image_url": image_url}));
            }
            s.serialize_field("content", &parts)?;
        } else {
            s.serialize_field("content", &self.content)?;
        }
        if let Some(ref tc) = self.tool_calls {
            s.serialize_field("tool_calls", tc)?;
        }
        if let Some(ref tci) = self.tool_call_id {
            s.serialize_field("tool_call_id", tci)?;
        }
        s.end()
    }
}

impl ChatMessage {
    /// Create a text-only message (backward-compatible shorthand).
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            images: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }
}

/// Stream a FIM completion from the configured provider
pub async fn stream_fim(
    provider: &LlmProvider,
    api_key: &str,
    base_url: Option<&str>,
    request: FimRequest,
    mut on_token: impl FnMut(String) -> Result<(), String>,
    cancel: Option<CancelFlag>,
) -> Result<(), String> {
    match provider {
        LlmProvider::OpenAI => stream_openai_fim(api_key, base_url, request, &mut on_token, cancel).await,
        LlmProvider::DeepSeek => {
            let b = base_url.unwrap_or_else(|| provider.default_base_url());
            stream_openai_fim(api_key, Some(b), request, &mut on_token, cancel).await
        }
        LlmProvider::Anthropic => stream_anthropic_fim(api_key, base_url, request, &mut on_token, cancel).await,
        LlmProvider::Ollama => stream_ollama_fim(api_key, base_url.unwrap_or("http://localhost:11434"), request, &mut on_token, cancel).await,
    }
} 

/// Stream a chat completion from the configured provider
pub async fn stream_chat(
    provider: &LlmProvider,
    api_key: &str,
    base_url: Option<&str>,
    request: ChatRequestParams,
    mut on_token: impl FnMut(String) -> Result<(), String>,
    cancel: Option<CancelFlag>,
) -> Result<(), String> {
    match provider {
        LlmProvider::OpenAI => stream_openai_chat(api_key, base_url, request, &mut on_token, cancel).await,
        LlmProvider::DeepSeek => {
            let b = base_url.unwrap_or_else(|| provider.default_base_url());
            stream_openai_chat(api_key, Some(b), request, &mut on_token, cancel).await
        }
        LlmProvider::Anthropic => stream_anthropic_chat(api_key, base_url, request, &mut on_token, cancel).await,
        LlmProvider::Ollama => stream_ollama_chat(api_key, base_url.unwrap_or("http://localhost:11434"), request, &mut on_token, cancel).await,
    }
}

// ─── OpenAI ─────────────────────────────────────────────────────────────────

async fn stream_openai_fim(
    api_key: &str,
    base_url: Option<&str>,
    request: FimRequest,
    on_token: &mut impl FnMut(String) -> Result<(), String>,
    cancel: Option<CancelFlag>,
) -> Result<(), String> {
    let base = base_url.unwrap_or("https://api.openai.com/v1");
    let url = format!("{}/completions", base);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let body = serde_json::json!({
        "model": request.model,
        "prompt": request.prompt,
        "suffix": "",
        "max_tokens": request.max_tokens,
        "temperature": request.temperature,
        "stream": true,
        "stop": ["\n\n\n"],
    });

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("API request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("OpenAI API error ({}): {}", status, text));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        if let Some(ref cancel) = cancel {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".to_string());
            }
        }
        let chunk = chunk.map_err(|e| format!("Stream error: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process SSE lines
        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim().to_string();
            buffer = buffer[pos + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            if line == "data: [DONE]" {
                return Ok(());
            }
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(text) = parsed["choices"][0]["text"].as_str() {
                        on_token(text.to_string())?;
                    }
                }
            }
        }
    }

    Ok(())
}

async fn stream_openai_chat(
    api_key: &str,
    base_url: Option<&str>,
    request: ChatRequestParams,
    on_token: &mut impl FnMut(String) -> Result<(), String>,
    cancel: Option<CancelFlag>,
) -> Result<(), String> {
    let base = base_url.unwrap_or("https://api.openai.com/v1");
    let url = format!("{}/chat/completions", base);

    log::info!(
        "[LLM] stream_chat: model={}, url={}, messages={}",
        request.model, url, request.messages.len()
    );
    let stream_start = std::time::Instant::now();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let mut api_messages = vec![serde_json::json!({
        "role": "system",
        "content": request.system,
    })];

    for msg in &request.messages {
        api_messages.push(serde_json::json!({
            "role": msg.role,
            "content": msg.content,
        }));
    }

    let body = serde_json::json!({
        "model": request.model,
        "messages": api_messages,
        "max_tokens": request.max_tokens,
        "temperature": request.temperature,
        "stream": true,
    });

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("API request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        log::error!("[LLM] stream_chat FAILED: status={}, body={}", status, text.chars().take(500).collect::<String>());
        return Err(format!("OpenAI API error ({}): {}", status, text));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut token_count = 0u32;

    while let Some(chunk) = stream.next().await {
        if let Some(ref cancel) = cancel {
            if cancel.load(Ordering::Relaxed) {
                log::info!("[LLM] stream_chat cancelled after {} tokens", token_count);
                return Err("Cancelled".to_string());
            }
        }
        let chunk = chunk.map_err(|e| format!("Stream error: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim().to_string();
            buffer = buffer[pos + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            if line == "data: [DONE]" {
                log::info!("[LLM] stream_chat completed: {} tokens, elapsed={}ms", token_count, stream_start.elapsed().as_millis());
                return Ok(());
            }
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(delta) = parsed["choices"][0]["delta"]["content"].as_str() {
                        token_count += 1;
                        on_token(delta.to_string())?;
                    }
                }
            }
        }
    }

    log::info!("[LLM] stream_chat ended (no DONE signal): {} tokens, elapsed={}ms", token_count, stream_start.elapsed().as_millis());
    Ok(())
}

// ─── Anthropic ──────────────────────────────────────────────────────────────

async fn stream_anthropic_fim(
    api_key: &str,
    base_url: Option<&str>,
    request: FimRequest,
    on_token: &mut impl FnMut(String) -> Result<(), String>,
    cancel: Option<CancelFlag>,
) -> Result<(), String> {
    // Anthropic doesn't have a dedicated FIM API; use chat with system prompt
    let chat_request = ChatRequestParams {
        model: request.model,
        messages: vec![ChatMessage {
            role: "user".into(),
            content: request.prompt,
            images: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        system: request.system_prompt,
        max_tokens: request.max_tokens,
        temperature: request.temperature,
        thinking_enabled: false,
        thinking_budget: 0,
    };
    stream_anthropic_chat(api_key, base_url, chat_request, on_token, cancel).await
}

async fn stream_anthropic_chat(
    api_key: &str,
    base_url: Option<&str>,
    request: ChatRequestParams,
    on_token: &mut impl FnMut(String) -> Result<(), String>,
    cancel: Option<CancelFlag>,
) -> Result<(), String> {
    let base = base_url.unwrap_or("https://api.anthropic.com/v1");
    let url = format!("{}/messages", base);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let mut api_messages = Vec::new();
    for msg in &request.messages {
        api_messages.push(serde_json::json!({
            "role": msg.role,
            "content": msg.content,
        }));
    }

    // Build request body with optional Extended Thinking support
    let mut body = serde_json::json!({
        "model": request.model,
        "messages": api_messages,
        "system": request.system,
        "max_tokens": request.max_tokens,
        "stream": true,
    });

    // Extended Thinking requires temperature=1 and the thinking parameter
    if request.thinking_enabled && request.thinking_budget > 0 {
        body["thinking"] = serde_json::json!({
            "type": "enabled",
            "budget_tokens": request.thinking_budget,
        });
        body["temperature"] = serde_json::json!(1.0);
    } else {
        body["temperature"] = serde_json::json!(request.temperature);
    }

    let response = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("API request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Anthropic API error ({}): {}", status, text));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        if let Some(ref cancel) = cancel {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".to_string());
            }
        }
        let chunk = chunk.map_err(|e| format!("Stream error: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim().to_string();
            buffer = buffer[pos + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    let event_type = parsed.get("type").and_then(|t| t.as_str());

                    match event_type {
                        Some("content_block_delta") => {
                            // Check delta type: "text_delta" for regular text, "thinking_delta" for thinking
                            let delta_type = parsed["delta"]["type"].as_str().unwrap_or("");
                            match delta_type {
                                "thinking_delta" => {
                                    // Thinking content - emit with [THINKING] prefix for frontend to parse
                                    if let Some(thinking) = parsed["delta"]["thinking"].as_str() {
                                        on_token(format!("[THINKING]{}", thinking))?;
                                    }
                                }
                                "text_delta" | _ => {
                                    // Regular text content
                                    if let Some(text) = parsed["delta"]["text"].as_str() {
                                        on_token(text.to_string())?;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(())
}

// ─── Ollama ─────────────────────────────────────────────────────────────────

async fn stream_ollama_fim(
    _api_key: &str,
    base_url: &str,
    request: FimRequest,
    on_token: &mut impl FnMut(String) -> Result<(), String>,
    cancel: Option<CancelFlag>,
) -> Result<(), String> {
    let url = format!("{}/api/generate", base_url);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let body = serde_json::json!({
        "model": request.model,
        "prompt": request.prompt,
        "system": request.system_prompt,
        "stream": true,
        "options": {
            "temperature": request.temperature,
            "num_predict": request.max_tokens,
        }
    });

    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Ollama request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Ollama error ({}): {}", status, text));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        if let Some(ref cancel) = cancel {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".to_string());
            }
        }
        let chunk = chunk.map_err(|e| format!("Stream error: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim().to_string();
            buffer = buffer[pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&line) {
                if parsed.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
                    return Ok(());
                }
                if let Some(text) = parsed["response"].as_str() {
                    on_token(text.to_string())?;
                }
            }
        }
    }

    Ok(())
}

async fn stream_ollama_chat(
    _api_key: &str,
    base_url: &str,
    request: ChatRequestParams,
    on_token: &mut impl FnMut(String) -> Result<(), String>,
    cancel: Option<CancelFlag>,
) -> Result<(), String> {
    let url = format!("{}/api/chat", base_url);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let mut api_messages = vec![serde_json::json!({
        "role": "system",
        "content": request.system,
    })];

    for msg in &request.messages {
        api_messages.push(serde_json::json!({
            "role": msg.role,
            "content": msg.content,
        }));
    }

    let body = serde_json::json!({
        "model": request.model,
        "messages": api_messages,
        "stream": true,
        "options": {
            "temperature": request.temperature,
            "num_predict": request.max_tokens,
        }
    });

    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Ollama request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Ollama error ({}): {}", status, text));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        if let Some(ref cancel) = cancel {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".to_string());
            }
        }
        let chunk = chunk.map_err(|e| format!("Stream error: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim().to_string();
            buffer = buffer[pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&line) {
                if parsed.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
                    return Ok(());
                }
                if let Some(msg) = parsed.get("message") {
                    if let Some(content) = msg["content"].as_str() {
                        on_token(content.to_string())?;
                    }
                }
            }
        }
    }

    Ok(())
}

// ─── Embeddings ──────────────────────────────────────────────────────────────

/// Generate embeddings for a batch of texts using the configured provider.
pub async fn embed_texts(
    provider: &LlmProvider,
    api_key: &str,
    base_url: Option<&str>,
    model: &str,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, String> {
    match provider {
        LlmProvider::OpenAI => embed_openai(api_key, base_url, model, texts).await,
        LlmProvider::Ollama => embed_ollama(api_key, base_url.unwrap_or("http://localhost:11434"), model, texts).await,
        LlmProvider::Anthropic => Err("Anthropic does not provide embeddings".to_string()),
        LlmProvider::DeepSeek => Err("DeepSeek does not provide embeddings".to_string()),
    }
}

async fn embed_openai(
    api_key: &str,
    base_url: Option<&str>,
    model: &str,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, String> {
    let base = base_url.unwrap_or("https://api.openai.com/v1");
    let url = format!("{}/embeddings", base);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let body = serde_json::json!({
        "model": model,
        "input": texts,
    });

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Embedding request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("OpenAI embedding error ({}): {}", status, text));
    }

    let data: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse embedding response: {}", e))?;

    let mut embeddings = Vec::new();
    if let Some(arr) = data["data"].as_array() {
        for item in arr {
            if let Some(vec) = item["embedding"].as_array() {
                let emb: Vec<f32> = vec
                    .iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect();
                embeddings.push(emb);
            }
        }
    }

    Ok(embeddings)
}

async fn embed_ollama(
    _api_key: &str,
    base_url: &str,
    model: &str,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, String> {
    let url = format!("{}/api/embed", base_url);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let body = serde_json::json!({
        "model": model,
        "input": texts,
    });

    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Ollama embedding request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Ollama embedding error ({}): {}", status, text));
    }

    let data: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse embedding response: {}", e))?;

    let mut embeddings = Vec::new();
    if let Some(arr) = data["embeddings"].as_array() {
        for item in arr {
            let emb: Vec<f32> = item
                .as_array()
                .map(|v| v.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect())
                .unwrap_or_default();
            embeddings.push(emb);
        }
    }

    Ok(embeddings)
}

pub struct ToolCallRequest {
    pub id :String,
    pub name :String,
    pub arguments :serde_json::Value,
}

/// Token usage statistics from LLM API response
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TokenUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

pub enum LlmResponse {
    Text(String),
    ToolCalls { calls: Vec<ToolCallRequest>, content: Option<String> },
}


pub async fn chat_with_tools(
    provider: &LlmProvider,
    api_key: &str,
    base_url: Option<&str>,
    request: ChatRequestParams,
    tools: &[serde_json::Value],
    cancel: Option<CancelFlag>,
) -> Result<(LlmResponse, Option<TokenUsage>), String> {
    match provider {
        LlmProvider::OpenAI => chat_with_tools_openai(api_key, base_url, request, tools, cancel).await,
        LlmProvider::DeepSeek => {
            let b = base_url.unwrap_or_else(|| provider.default_base_url());
            chat_with_tools_openai(api_key, Some(b), request, tools, cancel).await
        }
        LlmProvider::Ollama => Err("Ollama tool calling not yet supported".to_string()),
        LlmProvider::Anthropic => Err("Anthropic tool calling not yet supported".to_string()),
    }
}
pub async fn chat_with_tools_openai(
        api_key: &str, 
    base_url : Option<&str>,
    request: ChatRequestParams, 
    tools: &[serde_json::Value],
    cancel: Option<CancelFlag>,
)-> Result<(LlmResponse, Option<TokenUsage>), String> {
    let real_base_url = base_url.unwrap_or("https://api.openai.com/v1");
    let url =  format!("{}/chat/completions", real_base_url);

    log::info!(
        "[LLM] chat_with_tools: model={}, url={}, messages={}, tools={}",
        request.model, url, request.messages.len(), tools.len()
    );
    let request_start = std::time::Instant::now();

    let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|e|
            format!("Failed to create HTTP client: {}", e))?;

    let mut api_message = vec![serde_json::json!({
        "role": "system",
        "content": request.system,
    })];
    
    let sanitized = sanitize_messages(&request.messages);
    for msg in &sanitized {
        let mut json_msg = serde_json::json!({
            "role": msg.role,
            "content": msg.content,
        });
        if let Some(ref tc) = msg.tool_calls {
            json_msg["tool_calls"] = tc.clone();
        }
        if let Some(ref tcid) = msg.tool_call_id {
            json_msg["tool_call_id"] = serde_json::Value::String(tcid.clone());
        }
        api_message.push(json_msg);
    }

    let mut body = serde_json::json!({
        "model": request.model,
        "messages": api_message,
        "stream": false,
        "tool_choice": "auto",
    });

    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools.to_vec());
    }

    // Use tokio::select! to make the HTTP request cancellable
    let response = if let Some(ref cancel_flag) = cancel {
        tokio::select! {
            result = client
                .post(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .json(&body)
                .send() => {
                result.map_err(|e| format!("Chat request failed: {}", e))?
            }
            _ = async {
                loop {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    if cancel_flag.load(Ordering::Relaxed) {
                        break;
                    }
                }
            } => {
                return Err("Cancelled by user".to_string());
            }
        }
    } else {
        client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Chat request failed: {}", e))?
    };
        

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        log::error!(
            "[LLM] chat_with_tools FAILED: status={}, body={}, elapsed={}ms",
            status, text.chars().take(500).collect::<String>(), request_start.elapsed().as_millis()
        );
        return Err(format!("Chat request failed ({}): {}", status, text));
    }

    let data =response.json::<serde_json::Value>()
        .await
        .map_err(|e| {
            log::error!("[LLM] Failed to parse response JSON: {}", e);
            format!("Failed to parse chat response: {}", e)
        })?;

    let message = &data["choices"][0]["message"];
    let elapsed_ms = request_start.elapsed().as_millis();

    // Parse token usage from API response
    let usage = data.get("usage").map(|u| TokenUsage {
        prompt_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as usize,
        completion_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as usize,
        total_tokens: u["total_tokens"].as_u64().unwrap_or(0) as usize,
    });
    if let Some(ref u) = usage {
        log::info!("[LLM] Token usage: prompt={}, completion={}, total={}", u.prompt_tokens, u.completion_tokens, u.total_tokens);
    }

    // 检查是否有 tool_calls
    if let Some(tool_calls) = message["tool_calls"].as_array() {
        let mut calls = Vec::new();
        for tc in tool_calls {
            let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
            let args: serde_json::Value = serde_json::from_str(args_str)
                .unwrap_or(serde_json::json!({}));
    
            calls.push(ToolCallRequest {
                id: tc["id"].as_str().unwrap_or("").to_string(),
                name: tc["function"]["name"].as_str().unwrap_or("").to_string(),
                arguments: args,
            });
        }
        let content = message["content"].as_str().map(|s| s.to_string());
        log::info!(
            "[LLM] chat_with_tools OK: {} tool_calls, elapsed={}ms",
            calls.len(), elapsed_ms
        );
        Ok((LlmResponse::ToolCalls { calls, content }, usage))
    } else {
        // 纯文本回复
        let content = message["content"].as_str().unwrap_or("").to_string();
        log::info!(
            "[LLM] chat_with_tools OK: text ({} chars), elapsed={}ms",
            content.len(), elapsed_ms
        );
        Ok((LlmResponse::Text(content), usage))
    }
    
}

/// Streaming version of `chat_with_tools` — emits text tokens in real time via callback,
/// while accumulating tool_call chunks for a complete response at the end.
///
/// The OpenAI streaming format sends `delta` objects:
/// - Text: `{"choices":[{"delta":{"content":"token"}}]}`
/// - Tool calls: `{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"...","function":{"name":"...","arguments":"..."}}]}}]}`
///
/// Text tokens are forwarded to `on_token` immediately; tool_call argument fragments are
/// accumulated per-index and assembled into complete `ToolCallRequest` objects at `[DONE]`.
pub async fn stream_chat_with_tools(
    provider: &LlmProvider,
    api_key: &str,
    base_url: Option<&str>,
    request: ChatRequestParams,
    tools: &[serde_json::Value],
    cancel: Option<CancelFlag>,
    mut on_token: impl FnMut(String) -> Result<(), String>,
) -> Result<(LlmResponse, Option<TokenUsage>), String> {
    match provider {
        LlmProvider::OpenAI | LlmProvider::DeepSeek => {
            let b = base_url.unwrap_or_else(|| provider.default_base_url());
            stream_chat_with_tools_openai(api_key, Some(b), request, tools, cancel, &mut on_token).await
        }
        LlmProvider::Ollama => Err("Ollama streaming tool calling not yet supported".to_string()),
        LlmProvider::Anthropic => Err("Anthropic streaming tool calling not yet supported".to_string()),
    }
}

async fn stream_chat_with_tools_openai(
    api_key: &str,
    base_url: Option<&str>,
    request: ChatRequestParams,
    tools: &[serde_json::Value],
    cancel: Option<CancelFlag>,
    on_token: &mut impl FnMut(String) -> Result<(), String>,
) -> Result<(LlmResponse, Option<TokenUsage>), String> {
    let real_base_url = base_url.unwrap_or("https://api.openai.com/v1");
    let url = format!("{}/chat/completions", real_base_url);

    log::info!(
        "[LLM] stream_chat_with_tools: model={}, url={}, messages={}, tools={}",
        request.model, url, request.messages.len(), tools.len()
    );
    let request_start = std::time::Instant::now();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let mut api_message = vec![serde_json::json!({
        "role": "system",
        "content": request.system,
    })];

    let sanitized = sanitize_messages(&request.messages);
    for msg in &sanitized {
        let mut json_msg = serde_json::json!({
            "role": msg.role,
            "content": msg.content,
        });
        if let Some(ref tc) = msg.tool_calls {
            json_msg["tool_calls"] = tc.clone();
        }
        if let Some(ref tcid) = msg.tool_call_id {
            json_msg["tool_call_id"] = serde_json::Value::String(tcid.clone());
        }
        api_message.push(json_msg);
    }

    let mut body = serde_json::json!({
        "model": request.model,
        "messages": api_message,
        "stream": true,
        "tool_choice": "auto",
    });

    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools.to_vec());
    }

    let response = if let Some(ref cancel_flag) = cancel {
        tokio::select! {
            result = client
                .post(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .json(&body)
                .send() => {
                result.map_err(|e| format!("Chat request failed: {}", e))?
            }
            _ = async {
                loop {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    if cancel_flag.load(Ordering::Relaxed) {
                        break;
                    }
                }
            } => {
                return Err("Cancelled by user".to_string());
            }
        }
    } else {
        client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Chat request failed: {}", e))?
    };

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        log::error!(
            "[LLM] stream_chat_with_tools FAILED: status={}, body={}, elapsed={}ms",
            status, text.chars().take(500).collect::<String>(), request_start.elapsed().as_millis()
        );
        return Err(format!("Chat request failed ({}): {}", status, text));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut token_count = 0u32;

    // Accumulate text content streamed in real time
    let mut accumulated_text = String::new();
    // Accumulate tool calls by index: (id, name, arguments)
    // Using a Vec indexed by tool_calls[].index
    let mut tc_accumulation: Vec<(String, String, String)> = Vec::new();

    while let Some(chunk) = stream.next().await {
        if let Some(ref cancel_flag) = cancel {
            if cancel_flag.load(Ordering::Relaxed) {
                log::info!("[LLM] stream_chat_with_tools cancelled after {} tokens", token_count);
                return Err("Cancelled by user".to_string());
            }
        }
        let chunk = chunk.map_err(|e| format!("Stream error: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim().to_string();
            buffer = buffer[pos + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            if line == "data: [DONE]" {
                let elapsed_ms = request_start.elapsed().as_millis();
                log::info!(
                    "[LLM] stream_chat_with_tools completed: {} tokens, elapsed={}ms",
                    token_count, elapsed_ms
                );

                // Decide: tool calls or plain text?
                if !tc_accumulation.is_empty() {
                    let calls: Vec<ToolCallRequest> = tc_accumulation
                        .into_iter()
                        .map(|(id, name, args_str)| {
                            let arguments: serde_json::Value =
                                serde_json::from_str(&args_str).unwrap_or(serde_json::json!({}));
                            ToolCallRequest { id, name, arguments }
                        })
                        .collect();
                    let content = if accumulated_text.is_empty() { None } else { Some(accumulated_text) };
                    log::info!(
                        "[LLM] stream_chat_with_tools OK: {} tool_calls, elapsed={}ms",
                        calls.len(), elapsed_ms
                    );
                    return Ok((LlmResponse::ToolCalls { calls, content }, None));
                } else {
                    log::info!(
                        "[LLM] stream_chat_with_tools OK: text ({} chars), elapsed={}ms",
                        accumulated_text.len(), elapsed_ms
                    );
                    return Ok((LlmResponse::Text(accumulated_text), None));
                }
            }
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    let delta = &parsed["choices"][0]["delta"];

                    // Text content token — emit immediately
                    if let Some(text) = delta["content"].as_str() {
                        if !text.is_empty() {
                            token_count += 1;
                            accumulated_text.push_str(text);
                            let _ = on_token(text.to_string());
                        }
                    }

                    // Tool call chunk — accumulate by index
                    if let Some(tc_array) = delta["tool_calls"].as_array() {
                        for tc in tc_array {
                            let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                            // Ensure slot exists
                            while tc_accumulation.len() <= idx {
                                tc_accumulation.push((String::new(), String::new(), String::new()));
                            }
                            if let Some(id) = tc["id"].as_str() {
                                tc_accumulation[idx].0 = id.to_string();
                            }
                            if let Some(name) = tc["function"]["name"].as_str() {
                                tc_accumulation[idx].1.push_str(name);
                            }
                            if let Some(args) = tc["function"]["arguments"].as_str() {
                                tc_accumulation[idx].2.push_str(args);
                            }
                        }
                    }
                }
            }
        }
    }

    // Stream ended without [DONE] — still return what we accumulated
    let elapsed_ms = request_start.elapsed().as_millis();
    log::info!(
        "[LLM] stream_chat_with_tools ended (no DONE signal): {} tokens, elapsed={}ms",
        token_count, elapsed_ms
    );
    if !tc_accumulation.is_empty() {
        let calls: Vec<ToolCallRequest> = tc_accumulation
            .into_iter()
            .map(|(id, name, args_str)| {
                let arguments: serde_json::Value =
                    serde_json::from_str(&args_str).unwrap_or(serde_json::json!({}));
                ToolCallRequest { id, name, arguments }
            })
            .collect();
        let content = if accumulated_text.is_empty() { None } else { Some(accumulated_text) };
        Ok((LlmResponse::ToolCalls { calls, content }, None))
    } else {
        Ok((LlmResponse::Text(accumulated_text), None))
    }
}

/// Sanitize messages for OpenAI/DeepSeek API compatibility.
///
/// Ensures that every `role: "tool"` message is preceded by an `assistant` message
/// with `tool_calls`. Orphaned `tool` messages (caused by context compaction,
/// session restore, etc.) are silently removed.
///
/// Also converts orphaned `assistant(tool_calls)` messages (no following `tool`
/// responses) to plain `assistant` messages.
pub fn sanitize_messages(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut result: Vec<ChatMessage> = Vec::with_capacity(messages.len());
    let mut i = 0;

    while i < messages.len() {
        let msg = &messages[i];

        if msg.role == "assistant" && msg.tool_calls.is_some() {
            // assistant(tool_calls) — check if next message(s) are tool responses
            let mut has_tool_response = false;
            let mut j = i + 1;
            while j < messages.len() && messages[j].role == "tool" {
                has_tool_response = true;
                j += 1;
            }
            if has_tool_response {
                // Keep assistant(tool_calls) + all tool responses
                result.push(msg.clone());
                for k in (i + 1)..j {
                    result.push(messages[k].clone());
                }
                i = j;
            } else {
                // Orphaned assistant(tool_calls) — convert to plain assistant
                result.push(ChatMessage {
                    role: "assistant".into(),
                    content: if msg.content.is_empty() {
                        "[Tool call was made but no response received]".to_string()
                    } else {
                        msg.content.clone()
                    },
                    images: msg.images.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                });
                i += 1;
            }
        } else if msg.role == "tool" {
            // Orphaned tool message (no preceding assistant with tool_calls) — skip
            log::debug!(
                "[LLM] sanitize: removing orphaned tool message (tool_call_id={:?})",
                msg.tool_call_id
            );
            i += 1;
        } else {
            // Normal message (user, assistant text, system)
            result.push(msg.clone());
            i += 1;
        }
    }

    if result.len() < messages.len() {
        log::info!(
            "[LLM] sanitize: removed {} orphaned messages ({} -> {})",
            messages.len() - result.len(),
            messages.len(),
            result.len()
        );
    }

    result
}