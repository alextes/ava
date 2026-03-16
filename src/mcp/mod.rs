pub mod config;
pub mod manager;
mod transport;
pub mod types;

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::Error;

use transport::StdioTransport;
use types::{
    CallToolParams, CallToolResult, ClientCapabilities, Implementation, InitializeParams,
    InitializeResult, ListToolsResult, McpTool,
};

/// an MCP client connected to a single server subprocess.
pub struct McpClient {
    transport: Arc<StdioTransport>,
    pub server_name: String,
}

impl McpClient {
    /// spawn an MCP server and complete the initialize handshake.
    pub async fn start(
        server_name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self, Error> {
        let transport = StdioTransport::start(command, args, env)?;

        let client = Self {
            transport,
            server_name: server_name.to_string(),
        };

        client.initialize().await?;

        Ok(client)
    }

    /// perform the MCP initialize handshake.
    async fn initialize(&self) -> Result<InitializeResult, Error> {
        let params = InitializeParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities {},
            client_info: Implementation {
                name: "ava".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        let params_value = serde_json::to_value(&params).map_err(|e| Error::Mcp(e.to_string()))?;
        let resp = self
            .transport
            .request("initialize", Some(params_value))
            .await?;

        if let Some(err) = resp.error {
            return Err(Error::Mcp(format!(
                "initialize failed: {} ({})",
                err.message, err.code
            )));
        }

        let result: InitializeResult = serde_json::from_value(
            resp.result
                .ok_or_else(|| Error::Mcp("initialize: empty result".into()))?,
        )
        .map_err(|e| Error::Mcp(format!("initialize: bad result: {e}")))?;

        tracing::info!(
            server = %result.server_info.name,
            protocol = %result.protocol_version,
            "MCP server initialized"
        );

        // send initialized notification
        self.transport
            .notify("notifications/initialized", None)
            .await?;

        Ok(result)
    }

    /// discover tools from the server.
    pub async fn list_tools(&self) -> Result<Vec<McpTool>, Error> {
        let resp = self.transport.request("tools/list", None).await?;

        if let Some(err) = resp.error {
            return Err(Error::Mcp(format!(
                "tools/list failed: {} ({})",
                err.message, err.code
            )));
        }

        let result: ListToolsResult = serde_json::from_value(
            resp.result
                .ok_or_else(|| Error::Mcp("tools/list: empty result".into()))?,
        )
        .map_err(|e| Error::Mcp(format!("tools/list: bad result: {e}")))?;

        tracing::info!(
            server = %self.server_name,
            count = result.tools.len(),
            "discovered MCP tools"
        );

        Ok(result.tools)
    }

    /// call a tool on the server.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<CallToolResult, Error> {
        let params = CallToolParams {
            name: name.to_string(),
            arguments,
        };
        let params_value = serde_json::to_value(&params).map_err(|e| Error::Mcp(e.to_string()))?;

        let resp = self
            .transport
            .request("tools/call", Some(params_value))
            .await?;

        if let Some(err) = resp.error {
            return Err(Error::Mcp(format!(
                "tools/call failed: {} ({})",
                err.message, err.code
            )));
        }

        let result: CallToolResult = serde_json::from_value(
            resp.result
                .ok_or_else(|| Error::Mcp("tools/call: empty result".into()))?,
        )
        .map_err(|e| Error::Mcp(format!("tools/call: bad result: {e}")))?;

        Ok(result)
    }

    /// shut down the server process.
    pub async fn shutdown(&self) {
        self.transport.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a minimal MCP server implemented as a shell script.
    /// reads JSON-RPC requests line by line, responds to initialize, tools/list, and tools/call.
    fn mock_server_script() -> String {
        r#"
import sys, json

def respond(id, result):
    msg = {"jsonrpc": "2.0", "id": id, "result": result}
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    method = req.get("method", "")
    rid = req.get("id")

    if rid is None:
        # notification, ignore
        continue

    if method == "initialize":
        respond(rid, {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {"listChanged": False}},
            "serverInfo": {"name": "mock-server", "version": "0.1.0"}
        })
    elif method == "tools/list":
        respond(rid, {
            "tools": [
                {
                    "name": "echo",
                    "description": "echoes the input back",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"message": {"type": "string"}},
                        "required": ["message"]
                    }
                }
            ]
        })
    elif method == "tools/call":
        args = req.get("params", {}).get("arguments", {})
        msg = args.get("message", "")
        respond(rid, {
            "content": [{"type": "text", "text": msg}],
            "isError": False
        })
    else:
        respond(rid, {"error": {"code": -32601, "message": "method not found"}})
"#
        .to_string()
    }

    #[tokio::test]
    async fn test_mcp_client_initialize_and_list_tools() {
        let script = mock_server_script();
        let client = McpClient::start(
            "mock",
            "python3",
            &["-c".to_string(), script],
            &HashMap::new(),
        )
        .await
        .expect("failed to start mock MCP server");

        let tools = client.list_tools().await.expect("failed to list tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(
            tools[0].description.as_deref(),
            Some("echoes the input back")
        );

        client.shutdown().await;
    }

    #[tokio::test]
    async fn test_mcp_client_call_tool() {
        let script = mock_server_script();
        let client = McpClient::start(
            "mock",
            "python3",
            &["-c".to_string(), script],
            &HashMap::new(),
        )
        .await
        .expect("failed to start mock MCP server");

        let result = client
            .call_tool("echo", serde_json::json!({"message": "hello world"}))
            .await
            .expect("failed to call tool");

        assert_eq!(result.content.len(), 1);
        assert_eq!(result.content[0].content_type, "text");
        assert_eq!(result.content[0].text.as_deref(), Some("hello world"));
        assert_eq!(result.is_error, Some(false));

        client.shutdown().await;
    }
}
