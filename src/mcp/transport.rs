use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};

use crate::error::Error;

use super::types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

/// low-level JSON-RPC 2.0 transport over a child process's stdin/stdout.
pub struct StdioTransport {
    stdin: Mutex<tokio::process::ChildStdin>,
    pending: Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>,
    next_id: AtomicU64,
    child: Mutex<Child>,
}

impl StdioTransport {
    /// start a transport, returning it wrapped in Arc for safe concurrent use.
    /// spawns a reader task onto the tokio runtime to dispatch responses.
    pub fn start(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Arc<Self>, Error> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");

        let transport = Arc::new(Self {
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            child: Mutex::new(child),
        });

        // spawn reader task
        let weak = Arc::downgrade(&transport);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let Some(transport) = weak.upgrade() else {
                            break;
                        };
                        match serde_json::from_str::<JsonRpcResponse>(trimmed) {
                            Ok(resp) => {
                                if let Some(id) = resp.id {
                                    let mut pending = transport.pending.lock().await;
                                    if let Some(tx) = pending.remove(&id) {
                                        let _ = tx.send(resp);
                                    }
                                }
                                // notifications (no id) are ignored for now
                            }
                            Err(e) => {
                                tracing::debug!(
                                    line = trimmed,
                                    error = %e,
                                    "ignoring unparseable line from MCP server"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "MCP server stdout read error");
                        break;
                    }
                }
            }
        });

        Ok(transport)
    }

    /// send a request and wait for the response.
    pub async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse, Error> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = JsonRpcRequest::new(id, method, params);

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        let mut line = serde_json::to_string(&req).map_err(|e| Error::Mcp(e.to_string()))?;
        line.push('\n');

        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;
        }

        let resp = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| Error::Mcp(format!("request timed out: {method}")))?
            .map_err(|_| Error::Mcp("server closed connection".into()))?;

        Ok(resp)
    }

    /// send a notification (no response expected).
    pub async fn notify(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), Error> {
        let notif = JsonRpcNotification::new(method, params);
        let mut line = serde_json::to_string(&notif).map_err(|e| Error::Mcp(e.to_string()))?;
        line.push('\n');

        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;

        Ok(())
    }

    /// kill the child process.
    pub async fn shutdown(&self) {
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
    }
}
