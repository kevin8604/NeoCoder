//! MCP (Model Context Protocol) Client
//!
//! Implements a JSON-RPC 2.0 client that spawns MCP server processes (stdio transport)
//! and bridges their tools into NeeCoder's agent tool system.
//!
//! ## Architecture
//!
//! ```text
//! Agent Loop → ToolExecutor → McpToolBridge → McpClient ──stdin──► MCP Server Process
//!                                                      ◄─stdout── (node/python binary)
//! ```
//!
//! Each MCP server is a child process. Communication uses line-delimited JSON-RPC 2.0
//! over stdin/stdout. Tool discovery happens at startup; tool calls are dispatched
//! synchronously per invocation.
//!
//! ## References
//!
//! - MCP Spec: <https://spec.modelcontextprotocol.io/>
//! - MCP Servers: <https://github.com/modelcontextprotocol/servers>

pub mod client;
pub mod tool_bridge;

use serde::{Deserialize, Serialize};

// ── JSON-RPC 2.0 Types ──────────────────────────────────────────────────────

/// A JSON-RPC 2.0 request.
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub method: String,
    pub params: Option<serde_json::Value>,
}

/// A JSON-RPC 2.0 response (success).
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<u64>,
    pub result: Option<serde_json::Value>,
    #[allow(dead_code)]
    pub error: Option<serde_json::Value>,
}

// ── MCP Protocol Types ──────────────────────────────────────────────────────

/// Server configuration for spawning an MCP server process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Human-readable name for this server
    pub name: String,
    /// Executable command (e.g., "npx", "python", "node")
    pub command: String,
    /// Arguments to pass to the command
    pub args: Vec<String>,
    /// Environment variables (key=value)
    #[serde(default)]
    pub env: Vec<String>,
    /// Whether this server is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool { true }

/// A tool definition returned by `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub input_schema: serde_json::Value,
}

/// Content item returned by `tools/call`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpContentItem {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: Option<String>,
}

/// Result of `tools/call`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpCallResult {
    pub content: Vec<McpContentItem>,
}
