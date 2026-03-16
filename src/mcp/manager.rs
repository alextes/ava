use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::error::Error;

use super::McpClient;
use super::config::{McpServerConfig, load_mcp_config};
use super::types::McpTool;

/// manages the lifecycle of multiple MCP server connections.
pub struct McpManager {
    servers: RwLock<HashMap<String, Arc<McpClient>>>,
    configs: Vec<McpServerConfig>,
}

/// a tool definition paired with the server that provides it.
#[derive(Debug, Clone)]
pub struct McpToolEntry {
    pub server_name: String,
    pub tool: McpTool,
}

impl McpManager {
    /// load config and start all enabled MCP servers.
    /// servers that fail to start are logged and skipped.
    pub async fn start() -> Result<Arc<Self>, Error> {
        let config = load_mcp_config()?;
        let enabled: Vec<McpServerConfig> = config
            .mcp_servers
            .into_iter()
            .filter(|s| s.enabled)
            .collect();

        let mut servers = HashMap::new();

        for cfg in &enabled {
            match McpClient::start(&cfg.name, &cfg.command, &cfg.args, &cfg.env).await {
                Ok(client) => {
                    tracing::info!(server = %cfg.name, "MCP server started");
                    servers.insert(cfg.name.clone(), Arc::new(client));
                }
                Err(e) => {
                    tracing::error!(server = %cfg.name, error = %e, "failed to start MCP server");
                }
            }
        }

        Ok(Arc::new(Self {
            servers: RwLock::new(servers),
            configs: enabled,
        }))
    }

    /// discover all tools from all running servers.
    pub async fn list_all_tools(&self) -> Vec<McpToolEntry> {
        let servers = self.servers.read().await;
        let mut all_tools = Vec::new();

        for (name, client) in servers.iter() {
            match client.list_tools().await {
                Ok(tools) => {
                    for tool in tools {
                        all_tools.push(McpToolEntry {
                            server_name: name.clone(),
                            tool,
                        });
                    }
                }
                Err(e) => {
                    tracing::error!(server = %name, error = %e, "failed to list tools");
                }
            }
        }

        all_tools
    }

    /// call a tool on a specific server.
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<super::types::CallToolResult, Error> {
        let servers = self.servers.read().await;
        let client = servers
            .get(server_name)
            .ok_or_else(|| Error::Mcp(format!("server not found: {server_name}")))?;

        match client.call_tool(tool_name, arguments.clone()).await {
            Ok(result) => Ok(result),
            Err(e) => {
                tracing::warn!(
                    server = %server_name,
                    tool = %tool_name,
                    error = %e,
                    "tool call failed, attempting restart"
                );
                // drop the read lock before trying to restart
                drop(servers);
                self.try_restart(server_name).await;

                // retry once after restart
                let servers = self.servers.read().await;
                let client = servers.get(server_name).ok_or_else(|| {
                    Error::Mcp(format!("server not available after restart: {server_name}"))
                })?;
                client.call_tool(tool_name, arguments).await
            }
        }
    }

    /// attempt to restart a server using its original config.
    async fn try_restart(&self, server_name: &str) {
        let Some(cfg) = self.configs.iter().find(|c| c.name == server_name) else {
            tracing::error!(server = %server_name, "no config found for restart");
            return;
        };

        // shut down existing client
        {
            let servers = self.servers.read().await;
            if let Some(client) = servers.get(server_name) {
                client.shutdown().await;
            }
        }

        // try to start a new one
        match McpClient::start(&cfg.name, &cfg.command, &cfg.args, &cfg.env).await {
            Ok(client) => {
                let mut servers = self.servers.write().await;
                servers.insert(cfg.name.clone(), Arc::new(client));
                tracing::info!(server = %server_name, "MCP server restarted");
            }
            Err(e) => {
                let mut servers = self.servers.write().await;
                servers.remove(server_name);
                tracing::error!(
                    server = %server_name,
                    error = %e,
                    "failed to restart MCP server"
                );
            }
        }
    }

    /// shut down all servers gracefully.
    pub async fn shutdown(&self) {
        let servers = self.servers.read().await;
        for (name, client) in servers.iter() {
            tracing::info!(server = %name, "shutting down MCP server");
            client.shutdown().await;
        }
    }

    /// returns true if there are any running servers.
    pub async fn has_servers(&self) -> bool {
        !self.servers.read().await.is_empty()
    }
}
