//! LLM Router: routes tasks to local (Ollama) or remote providers with auto-degradation.
//!
//! Task types are routed based on quality/latency/cost requirements:
//! - Agent main loop → remote (high quality, tool calling)
//! - Dreaming/summary → local first (free + privacy), fallback remote
//! - Simple chat → local if enabled, fallback remote
//! - Embeddings → local Ollama first (if enabled), fallback remote
//! - Code completion → remote (low latency requirement)

use crate::config::{LlmProvider, LocalModelConfig};

/// Task types that can be routed to different LLM backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskType {
    /// Agent main loop reasoning (requires tool calling, high quality)
    AgentMainLoop,
    /// Session summarization / memory consolidation
    Dreaming,
    /// Simple Q&A (Ask mode)
    SimpleChat,
    /// FIM code completion
    CodeCompletion,
    /// Text embedding generation
    Embedding,
    /// Fine-tune data generation
    FinetuneDataGen,
}

/// Resolved LLM route: provider + connection parameters.
#[derive(Debug, Clone)]
pub struct LlmRoute {
    pub provider: LlmProvider,
    pub base_url: Option<String>,
    pub api_key: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

/// Router decides which backend handles a given task.
pub struct LlmRouter;

impl LlmRouter {
    /// Route a task based on local model availability (probed at call time).
    ///
    /// `local_available` should be the result of `health::check_ollama()`.
    pub fn route(
        task: TaskType,
        local: &LocalModelConfig,
        local_available: bool,
        remote_provider: &LlmProvider,
        remote_api_key: &str,
        remote_model: &str,
    ) -> LlmRoute {
        let remote = || LlmRoute {
            provider: remote_provider.clone(),
            base_url: None,
            api_key: remote_api_key.to_string(),
            model: remote_model.to_string(),
            temperature: 0.7,
            max_tokens: 4096,
        };

        let local_route = |model: String, temperature: f32, max_tokens: u32| LlmRoute {
            provider: LlmProvider::Ollama,
            base_url: Some(local.base_url.clone()),
            api_key: String::new(),
            model,
            temperature,
            max_tokens,
        };

        let local_on = local.enabled && local_available;

        match task {
            // Agent main loop always uses remote (tool calling + quality)
            TaskType::AgentMainLoop => remote(),

            // Dreaming: local first (privacy + cost), fallback to remote
            TaskType::Dreaming | TaskType::FinetuneDataGen => {
                if local_on && !local.dreaming_model.is_empty() {
                    local_route(local.dreaming_model.clone(), 0.3, 512)
                } else {
                    let mut r = remote();
                    r.temperature = 0.3;
                    r.max_tokens = 512;
                    r
                }
            }

            // Simple chat: local inference if enabled
            TaskType::SimpleChat => {
                if local_on && !local.inference_model.is_empty() {
                    local_route(local.inference_model.clone(), 0.7, 2048)
                } else {
                    remote()
                }
            }

            // Code completion: remote preferred (latency + FIM support)
            TaskType::CodeCompletion => remote(),

            // Embedding: local Ollama first, fallback to remote (e.g. OpenAI)
            TaskType::Embedding => {
                if local_on && !local.embedding_model.is_empty() {
                    LlmRoute {
                        provider: LlmProvider::Ollama,
                        base_url: Some(local.base_url.clone()),
                        api_key: String::new(),
                        model: local.embedding_model.clone(),
                        temperature: 0.0,
                        max_tokens: 0,
                    }
                } else {
                    remote()
                }
            }
        }
    }
}
