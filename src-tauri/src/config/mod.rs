use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::RwLock;
use crate::sandbox::SandboxConfig;

// ── API Key obfuscation (XOR + hex encoding) ──

const XOR_KEY: &[u8] = b"NeeCoder-v2-xor-key-2024";

fn xor_obfuscate(input: &str) -> String {
    input.bytes()
        .enumerate()
        .map(|(i, b)| b ^ XOR_KEY[i % XOR_KEY.len()])
        .map(|b| format!("{:02x}", b))
        .collect()
}

fn xor_deobfuscate(hex_str: &str) -> String {
    let bytes: Vec<u8> = (0..hex_str.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex_str[i..i + 2], 16).ok())
        .enumerate()
        .map(|(i, b)| b ^ XOR_KEY[i % XOR_KEY.len()])
        .collect();
    String::from_utf8_lossy(&bytes).to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub llm_provider: LlmProvider,
    pub completion_model: String,
    pub chat_model: String,
    pub embedding_model: String,
    /// Fast/cheap model for simple tasks (Ask mode, summaries). Falls back to chat_model if empty.
    #[serde(default)]
    pub fast_model: String,
    /// Enable automatic model routing based on task complexity
    #[serde(default)]
    pub model_routing_enabled: bool,
    /// Plain-text API key (runtime only, not persisted to disk)
    #[serde(default, skip_serializing)]
    pub api_key: String,
    /// Encrypted API key (persisted to disk, takes priority on load)
    #[serde(default)]
    pub api_key_encrypted: Option<String>,
    pub completion_enabled: bool,
    pub trigger_debounce_ms: u64,
    pub max_context_tokens: u32,
    pub max_prefix_lines: u32,
    pub max_suffix_lines: u32,
    pub ignore_patterns: Vec<String>,
    pub custom_instructions: String,
    pub project_paths: Vec<String>,
    pub theme: Theme,
    /// Sandbox security configuration
    #[serde(default)]
    pub sandbox: SandboxConfig,
    /// Max LLM API calls per session (0 = unlimited)
    #[serde(default = "default_max_api_calls")]
    pub max_api_calls_per_session: u32,
    /// Loop detection: identical (name+args+output) repeats before warning (0 = disabled)
    #[serde(default = "default_loop_no_progress")]
    pub loop_no_progress_threshold: u32,
    /// Loop detection: ping-pong A→B→A→B cycles before warning (0 = disabled)
    #[serde(default = "default_loop_ping_pong")]
    pub loop_ping_pong_cycles: u32,
    /// Loop detection: consecutive tool failures before warning (0 = disabled)
    #[serde(default = "default_loop_failure_streak")]
    pub loop_failure_streak_threshold: u32,
    /// Session expiry: auto-delete sessions older than N days (0 = never)
    #[serde(default)]
    pub session_expiry_days: u32,
    /// Enable Claude Extended Thinking (Anthropic only)
    #[serde(default)]
    pub thinking_enabled: bool,
    /// Thinking budget in tokens (1024-10000, default 1024)
    #[serde(default = "default_thinking_budget")]
    pub thinking_budget: u32,
    /// Tavily Search API key (encrypted on disk)
    #[serde(default)]
    pub tavily_api_key: String,
}

fn default_thinking_budget() -> u32 {
    1024
}

fn default_max_api_calls() -> u32 {
    200
}

fn default_loop_no_progress() -> u32 {
    3
}

fn default_loop_ping_pong() -> u32 {
    2
}

fn default_loop_failure_streak() -> u32 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    OpenAI,
    Anthropic,
    Ollama,
    DeepSeek,
}

impl LlmProvider {
    pub fn default_base_url(&self) -> &str {
        match self {
            LlmProvider::OpenAI => "https://api.openai.com/v1",
            LlmProvider::Anthropic => "https://api.anthropic.com/v1",
            LlmProvider::Ollama => "http://localhost:11434",
            LlmProvider::DeepSeek => "https://api.deepseek.com/v1",
        }
    }
}

/// Map model name to its context window size (in tokens).
/// Used by both backend (Agent compaction threshold) and frontend (UI hint).
pub fn model_context_window(model: &str) -> usize {
    let m = model.to_lowercase();
    if m.contains("deepseek-v4") || m.contains("deepseek-v3") {
        128_000
    } else if m.contains("deepseek") {
        64_000
    } else if m.contains("claude-3.5-sonnet") || m.contains("claude-3.5-haiku") {
        200_000
    } else if m.contains("claude-3-opus") || m.contains("claude-3-sonnet") {
        200_000
    } else if m.contains("gpt-4o") || m.contains("gpt-4-turbo") {
        128_000
    } else if m.contains("gpt-4") {
        8_192
    } else if m.contains("gpt-3.5") {
        16_385
    } else if m.contains("qwen") {
        128_000
    } else if m.contains("llama-3") || m.contains("llama3") {
        128_000
    } else {
        32_000
    }
}

impl Default for LlmProvider {
    fn default() -> Self {
        Self::DeepSeek
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Theme {
    Light,
    Dark,
}

impl Default for Theme {
    fn default() -> Self {
        Self::Dark
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            llm_provider: LlmProvider::default(),
            completion_model: "deepseek-chat".to_string(),
            chat_model: "deepseek-chat".to_string(),
            embedding_model: "text-embedding-3-small".to_string(),
            fast_model: "deepseek-chat".to_string(),
            model_routing_enabled: false,
            api_key: "sk-a14a4f6e19f84b5998f6178d8283eaf8".to_string(),
            api_key_encrypted: None,
            completion_enabled: true,
            trigger_debounce_ms: 300,
            max_context_tokens: 8192,
            max_prefix_lines: 80,
            max_suffix_lines: 40,
            ignore_patterns: vec![
                "node_modules/**".into(),
                "target/**".into(),
                ".git/**".into(),
                "dist/**".into(),
                "build/**".into(),
            ],
            custom_instructions: String::new(),
            project_paths: vec![],
            theme: Theme::Dark,
            sandbox: SandboxConfig::default(),
            max_api_calls_per_session: default_max_api_calls(),
            loop_no_progress_threshold: default_loop_no_progress(),
            loop_ping_pong_cycles: default_loop_ping_pong(),
            loop_failure_streak_threshold: default_loop_failure_streak(),
            session_expiry_days: 0,
            thinking_enabled: false,
            thinking_budget: default_thinking_budget(),
            tavily_api_key: "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".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub models: Vec<String>,
    pub embedding_models: Vec<String>,
}

impl AppSettings {
    pub fn provider_config(&self) -> ProviderConfig {
        match self.llm_provider {
            LlmProvider::OpenAI => ProviderConfig {
                name: "openai".into(),
                models: vec![
                    "gpt-4o".into(),
                    "gpt-4o-mini".into(),
                    "gpt-4.1".into(),
                    "gpt-4.1-mini".into(),
                ],
                embedding_models: vec![
                    "text-embedding-3-small".into(),
                    "text-embedding-3-large".into(),
                ],
            },
            LlmProvider::Anthropic => ProviderConfig {
                name: "anthropic".into(),
                models: vec![
                    "claude-sonnet-4-20250514".into(),
                    "claude-haiku-3-5-20241022".into(),
                ],
                embedding_models: vec![],
            },
            LlmProvider::Ollama => ProviderConfig {
                name: "ollama".into(),
                models: vec![
                    "codellama".into(),
                    "deepseek-coder".into(),
                    "qwen2.5-coder".into(),
                ],
                embedding_models: vec!["nomic-embed-text".into()],
            },
            LlmProvider::DeepSeek => ProviderConfig {
                name: "deepseek".into(),
                models: vec![
                    "deepseek-chat".into(),
                    "deepseek-coder".into(),
                ],
                embedding_models: vec![],
            },
        }
    }
}

pub struct ConfigManager {
    config_path: PathBuf,
    settings: Arc<RwLock<AppSettings>>,
}

impl ConfigManager {
    pub fn new(config_dir: PathBuf) -> Self {
        let config_path = config_dir.join("settings.json");
        let mut settings: AppSettings = if config_path.exists() {
            std::fs::read_to_string(&config_path)
                .ok()
                .and_then(|content| serde_json::from_str(&content).ok())
                .unwrap_or_default()
        } else {
            AppSettings::default()
        };

        // Decrypt API key on load: prefer encrypted version over plain-text
        if let Some(ref encrypted) = settings.api_key_encrypted {
            if !encrypted.is_empty() {
                settings.api_key = xor_deobfuscate(encrypted);
            }
        } else if !settings.api_key.is_empty() {
            // Migration: old config has plain-text api_key, encrypt it
            settings.api_key_encrypted = Some(xor_obfuscate(&settings.api_key));
        }

        let manager = Self {
            config_path,
            settings: Arc::new(RwLock::new(settings)),
        };

        // Save default/migrated config if not exists or needs migration
        if !manager.config_path.exists() || manager.config_path.exists() {
            let _ = manager.save();
        }

        manager
    }

    pub fn settings_handle(&self) -> Arc<RwLock<AppSettings>> {
        self.settings.clone()
    }

    pub async fn get_settings(&self) -> AppSettings {
        self.settings.read().await.clone()
    }

    pub async fn update_settings(&self, new_settings: AppSettings) -> Result<(), String> {
        let mut settings = self.settings.write().await;
        *settings = new_settings;
        // Encrypt API key before serializing (hold lock only during clone)
        settings.api_key_encrypted = Some(xor_obfuscate(&settings.api_key));
        let json = serde_json::to_string_pretty(&*settings).map_err(|e| e.to_string())?;
        // Drop write lock before file I/O to avoid holding lock across blocking operations
        drop(settings);
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.config_path, json).map_err(|e| e.to_string())
    }

    fn save(&self) -> Result<(), std::io::Error> {
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Encrypt API key before saving
        let mut settings = self.settings.blocking_read().clone();
        settings.api_key_encrypted = Some(xor_obfuscate(&settings.api_key));
        let json = serde_json::to_string_pretty(&settings)?;
        std::fs::write(&self.config_path, json)?;
        Ok(())
    }
}
