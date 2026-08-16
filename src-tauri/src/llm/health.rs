//! Ollama health check and status monitoring.
//!
//! Probes `GET {base_url}/api/tags` to determine availability and list loaded models.
//! Used by the LLM Router (auto-degradation) and the frontend status indicator.

use serde::Serialize;
use std::time::Duration;

/// Health snapshot of the local Ollama service.
#[derive(Debug, Clone, Serialize, Default)]
pub struct LocalModelHealth {
    pub running: bool,
    pub models: Vec<String>,
    pub error: Option<String>,
}

/// Check whether the Ollama service is reachable and list available models.
pub async fn check_ollama(base_url: &str) -> LocalModelHealth {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return LocalModelHealth {
                running: false,
                models: vec![],
                error: Some(format!("Failed to build HTTP client: {}", e)),
            };
        }
    };

    let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
    let response = match client.get(&url).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            return LocalModelHealth {
                running: false,
                models: vec![],
                error: Some(format!("HTTP {}", r.status())),
            };
        }
        Err(e) => {
            return LocalModelHealth {
                running: false,
                models: vec![],
                error: Some(format!("Connection failed: {}", e)),
            };
        }
    };

    match response.json::<serde_json::Value>().await {
        Ok(data) => {
            let models = data["models"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m["name"].as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            LocalModelHealth {
                running: true,
                models,
                error: None,
            }
        }
        Err(e) => LocalModelHealth {
            running: false,
            models: vec![],
            error: Some(format!("Invalid response: {}", e)),
        },
    }
}

/// Quick availability probe (only booleans, no model listing).
pub async fn is_ollama_running(base_url: &str) -> bool {
    check_ollama(base_url).await.running
}
