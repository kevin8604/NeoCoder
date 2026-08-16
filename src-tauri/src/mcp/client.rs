//! MCP client: spawns a server process and communicates via stdio JSON-RPC 2.0.
//!
//! Uses a response-map pattern:
//!   1. Spawn server process with piped stdin/stdout
//!   2. Spawn a background reader task that reads stdout lines
//!   3. send_request writes to stdin, inserts a oneshot sender into a HashMap<id, sender>
//!   4. The reader task matches responses by id and sends them through the channel
//!   5. send_request awaits the oneshot receiver

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};

use super::{JsonRpcRequest, JsonRpcResponse, McpServerConfig, McpToolDef};

/// Connected MCP client wrapping a server child process.
pub struct McpClient {
    config_name: String,
    /// Stdin writer — wrapped in Mutex for sequential writes.
    stdin: Arc<Mutex<ChildStdin>>,
    /// Monotonic request ID counter.
    next_id: AtomicU64,
    /// Map of pending requests: id → oneshot sender.
    /// The background reader task sends responses through these channels.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    /// Server process handle — killed on drop.
    _child: Child,
}

impl McpClient {
    /// Spawn the MCP server process and start the background reader task.
    pub async fn spawn(config: McpServerConfig) -> Result<Self, String> {
        let config_name = config.name.clone();

        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::inherit());
        cmd.kill_on_drop(true);

        for env_pair in &config.env {
            if let Some((key, value)) = env_pair.split_once('=') {
                cmd.env(key, value);
            }
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn MCP server '{}': {}", config_name, e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("MCP server '{}' has no stdin", config_name))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("MCP server '{}' has no stdout", config_name))?;

        let stdin = Arc::new(Mutex::new(stdin));
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Spawn background reader task
        let pending_clone = pending.clone();
        let config_name_clone = config_name.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<JsonRpcResponse>(trimmed) {
                    Ok(resp) => {
                        if let Some(id) = resp.id {
                            let mut map = pending_clone.lock().await;
                            if let Some(sender) = map.remove(&id) {
                                let _ = sender.send(resp);
                            } else {
                                log::debug!(
                                    "[MCP] Response for unknown id={} from '{}'",
                                    id,
                                    config_name_clone
                                );
                            }
                        } else {
                            // Notification — silently consume
                            log::trace!("[MCP] Notification from '{}'", config_name_clone);
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "[MCP] Failed to parse response from '{}': {} (line: {})",
                            config_name_clone,
                            e,
                            &trimmed[..trimmed.len().min(200)]
                        );
                    }
                }
            }
            log::info!("[MCP] Server '{}' stdout closed", config_name_clone);
        });

        Ok(Self {
            config_name,
            stdin,
            next_id: AtomicU64::new(1),
            pending,
            _child: child,
        })
    }

    /// Perform the MCP initialize handshake.
    pub async fn initialize(&self) -> Result<(), String> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "NeoCoder",
                "version": "0.1.0"
            }
        });

        let response = self.send_request("initialize", Some(params)).await?;

        if let Some(ref err) = response.error {
            return Err(format!(
                "MCP initialize failed for '{}': {}",
                self.config_name, err
            ));
        }

        log::info!(
            "[MCP] Server '{}' initialized: {:?}",
            self.config_name,
            response
                .result
                .as_ref()
                .and_then(|r| r.get("serverInfo").and_then(|i| i.get("name")))
        );

        // Send initialized notification (no id)
        self.send_notification("notifications/initialized", None)
            .await?;

        Ok(())
    }

    /// List all tools exposed by this MCP server.
    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>, String> {
        let response = self.send_request("tools/list", None).await?;

        if let Some(ref err) = response.error {
            return Err(format!(
                "MCP tools/list failed for '{}': {}",
                self.config_name, err
            ));
        }

        let tools: Vec<McpToolDef> = response
            .result
            .as_ref()
            .and_then(|r| r.get("tools"))
            .and_then(|t| serde_json::from_value(t.clone()).ok())
            .unwrap_or_default();

        log::info!(
            "[MCP] Server '{}' reports {} tools",
            self.config_name,
            tools.len()
        );
        Ok(tools)
    }

    /// Call a specific tool exposed by this MCP server.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<String, String> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        });

        let response = self.send_request("tools/call", Some(params)).await?;

        if let Some(ref err) = response.error {
            return Err(format!(
                "MCP tool '{}' call failed on '{}': {}",
                tool_name, self.config_name, err
            ));
        }

        let content = response
            .result
            .as_ref()
            .and_then(|r| r.get("content"))
            .and_then(|c| c.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                            item.get("text")
                                .and_then(|t| t.as_str())
                                .map(|s| s.to_string())
                        } else {
                            Some(format!(
                                "[{}]",
                                item.get("type")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("unknown")
                            ))
                        }
                    })
                    .collect::<Vec<String>>()
                    .join("\n")
            })
            .unwrap_or_else(|| "[MCP] Tool returned empty response".to_string());

        Ok(content)
    }

    /// Send a JSON-RPC request and wait for the response via oneshot channel.
    async fn send_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: Some(id),
            method: method.to_string(),
            params,
        };

        let request_json =
            serde_json::to_string(&request).map_err(|e| format!("MCP serialize: {}", e))?;

        // Create oneshot channel BEFORE writing to stdin (prevent race)
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        // Write request to stdin
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(request_json.as_bytes())
                .await
                .map_err(|e| format!("MCP write to '{}': {}", self.config_name, e))?;
            stdin
                .write_all(b"\n")
                .await
                .map_err(|e| format!("MCP write newline to '{}': {}", self.config_name, e))?;
            stdin
                .flush()
                .await
                .map_err(|e| format!("MCP flush '{}': {}", self.config_name, e))?;
        }

        // Wait for response with timeout
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_recv_err)) => {
                // Sender dropped — clean up our pending entry
                let mut map = self.pending.lock().await;
                map.remove(&id);
                Err(format!(
                    "MCP server '{}' closed connection during request '{}'",
                    self.config_name, method
                ))
            }
            Err(_timeout) => {
                // Clean up pending entry
                let mut map = self.pending.lock().await;
                map.remove(&id);
                Err(format!(
                    "MCP request '{}' to '{}' timed out after 30s",
                    method, self.config_name
                ))
            }
        }
    }

    /// Send a notification (no id, no response expected).
    async fn send_notification(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: None,
            method: method.to_string(),
            params,
        };

        let request_json =
            serde_json::to_string(&request).map_err(|e| format!("MCP serialize: {}", e))?;

        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(request_json.as_bytes())
            .await
            .map_err(|e| format!("MCP write: {}", e))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| format!("MCP write newline: {}", e))?;
        stdin
            .flush()
            .await
            .map_err(|e| format!("MCP flush: {}", e))?;

        Ok(())
    }
}

// ── MCP Registry ────────────────────────────────────────────────────────────

/// Manages multiple MCP clients, discovers tools, and provides a unified registry.
pub struct McpRegistry {
    clients: Mutex<HashMap<String, Arc<McpClient>>>,
    /// All discovered tools: prefixed_name → (server_name, McpToolDef)
    tools: Mutex<HashMap<String, (String, McpToolDef)>>,
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
            tools: Mutex::new(HashMap::new()),
        }
    }

    /// Connect to an MCP server, initialize, and discover its tools.
    /// Returns the number of tools discovered.
    pub async fn connect(&self, config: McpServerConfig) -> Result<usize, String> {
        let server_name = config.name.clone();

        if !config.enabled {
            log::info!("[MCP] Server '{}' is disabled, skipping", server_name);
            return Ok(0);
        }

        log::info!(
            "[MCP] Connecting to MCP server '{}': {} {}",
            server_name,
            config.command,
            config.args.join(" ")
        );

        let client = McpClient::spawn(config).await?;
        client.initialize().await?;

        let tools = client.list_tools().await?;
        let tool_count = tools.len();
        let client = Arc::new(client);

        // Store client
        {
            let mut clients = self.clients.lock().await;
            clients.insert(server_name.clone(), client);
        }

        // Register tools with prefixed names to avoid collisions
        {
            let mut tool_map = self.tools.lock().await;
            for tool in tools {
                let prefixed_name = format!("mcp_{}__{}", server_name, tool.name);
                log::info!(
                    "[MCP] Registered tool: {} (server '{}')",
                    prefixed_name,
                    server_name
                );
                tool_map.insert(prefixed_name, (server_name.clone(), tool));
            }
        }

        Ok(tool_count)
    }

    /// Call an MCP tool by its prefixed name.
    pub async fn call_tool(
        &self,
        prefixed_name: &str,
        arguments: serde_json::Value,
    ) -> Result<String, String> {
        let (server_name, tool_def) = {
            let tools = self.tools.lock().await;
            tools
                .get(prefixed_name)
                .cloned()
                .ok_or_else(|| format!("MCP tool '{}' not found", prefixed_name))?
        };

        let client = {
            let clients = self.clients.lock().await;
            clients
                .get(&server_name)
                .cloned()
                .ok_or_else(|| format!("MCP server '{}' not connected", server_name))?
        };

        client.call_tool(&tool_def.name, arguments).await
    }

    /// Get all discovered tool definitions as OpenAI-compatible tool JSONs.
    pub async fn get_tool_schemas(&self) -> Vec<serde_json::Value> {
        let tools = self.tools.lock().await;
        tools
            .iter()
            .map(|(prefixed_name, (_, tool_def))| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": prefixed_name,
                        "description": tool_def.description,
                        "parameters": tool_def.input_schema,
                    }
                })
            })
            .collect()
    }

    /// Check if a server is currently connected.
    pub async fn is_connected(&self, server_name: &str) -> bool {
        let clients = self.clients.lock().await;
        clients.contains_key(server_name)
    }

    /// Get list of connected server names.
    pub async fn list_connected_servers(&self) -> Vec<String> {
        let clients = self.clients.lock().await;
        clients.keys().cloned().collect()
    }

    /// Get the number of tools registered by a specific server.
    pub async fn tool_count_for_server(&self, server_name: &str) -> usize {
        let tools = self.tools.lock().await;
        tools
            .iter()
            .filter(|(_, (srv, _))| srv == server_name)
            .count()
    }

    /// Disconnect from a server: remove its client (process killed on drop)
    /// and remove all its tools. Returns the number of tools removed.
    pub async fn disconnect(&self, server_name: &str) -> Result<usize, String> {
        // Remove client first (Arc drop will kill child process via kill_on_drop)
        {
            let mut clients = self.clients.lock().await;
            if clients.remove(server_name).is_none() {
                return Err(format!("MCP server '{}' not connected", server_name));
            }
        }

        // Remove all tools belonging to this server
        let removed = {
            let mut tools = self.tools.lock().await;
            let to_remove: Vec<String> = tools
                .iter()
                .filter(|(_, (srv, _))| srv == server_name)
                .map(|(name, _)| name.clone())
                .collect();
            for key in &to_remove {
                tools.remove(key);
            }
            to_remove.len()
        };

        log::info!(
            "[MCP] Disconnected from '{}', removed {} tools",
            server_name,
            removed
        );
        Ok(removed)
    }
}
