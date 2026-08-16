//! MCP Tool Bridge — wraps an MCP tool as a NeoCoder `Tool` implementation.
//!
//! Each MCP tool from a connected server gets an `McpToolWrapper` that implements
//! the `Tool` trait. The wrapper delegates `execute()` to the `McpRegistry`.

use async_trait::async_trait;
use std::sync::Arc;

use super::client::McpRegistry;
use super::McpToolDef;
use crate::agent::tools::{PostExecuteAction, Tool, ToolContext};

/// A single MCP tool wrapped as a NeoCoder Tool.
///
/// Created per discovered tool. The prefixed name (e.g., `mcp_filesystem__read_file`)
/// uniquely identifies this tool across all connected MCP servers.
pub struct McpToolWrapper {
    /// Prefixed tool name used in tool registration and LLM function calling.
    pub prefixed_name: String,
    /// The original tool definition from the MCP server (for schema generation).
    pub definition: McpToolDef,
    /// Reference to the shared MCP registry for dispatching calls.
    registry: Arc<McpRegistry>,
}

impl McpToolWrapper {
    pub fn new(
        prefixed_name: String,
        definition: McpToolDef,
        registry: Arc<McpRegistry>,
    ) -> Self {
        Self {
            prefixed_name,
            definition,
            registry,
        }
    }

    /// Generate the OpenAI-compatible tool JSON schema for this tool.
    pub fn to_openai_tool(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.prefixed_name,
                "description": self.definition.description,
                "parameters": self.definition.input_schema,
            }
        })
    }
}

#[async_trait]
impl Tool for McpToolWrapper {
    fn name(&self) -> &str {
        &self.prefixed_name
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> String {
        match self.registry.call_tool(&self.prefixed_name, args).await {
            Ok(result) => result,
            Err(e) => format!("[MCP_ERROR] {}: {}", self.prefixed_name, e),
        }
    }

    fn post_execute_action(&self, _args: &serde_json::Value) -> PostExecuteAction {
        PostExecuteAction::None
    }
}
