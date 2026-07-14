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
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    next_id: AtomicU64,
    child: Mutex<Child>,
}

struct PendingRequestGuard {
    id: Option<u64>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
}

impl PendingRequestGuard {
    fn disarm(&mut self) {
        self.id = None;
    }
}

impl Drop for PendingRequestGuard {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        let pending = Arc::clone(&self.pending);
        let stdin = Arc::clone(&self.stdin);
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            if pending.lock().await.remove(&id).is_none() {
                return;
            }

            let notification = JsonRpcNotification::new(
                "notifications/cancelled",
                Some(serde_json::json!({
                    "requestId": id,
                    "reason": "active turn stopped"
                })),
            );
            let Ok(mut line) = serde_json::to_string(&notification) else {
                return;
            };
            line.push('\n');
            let mut stdin = stdin.lock().await;
            if let Err(e) = stdin.write_all(line.as_bytes()).await {
                tracing::debug!(%e, request_id = id, "failed to notify MCP server of cancellation");
                return;
            }
            if let Err(e) = stdin.flush().await {
                tracing::debug!(%e, request_id = id, "failed to flush MCP cancellation");
            }
        });
    }
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
            stdin: Arc::new(Mutex::new(stdin)),
            pending: Arc::new(Mutex::new(HashMap::new())),
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
        let mut cancellation_guard = PendingRequestGuard {
            id: Some(id),
            pending: Arc::clone(&self.pending),
            stdin: Arc::clone(&self.stdin),
        };

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

        cancellation_guard.disarm();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_request_removes_pending_entry() {
        let transport = StdioTransport::start(
            "sh",
            &["-c".into(), "while read line; do sleep 30; done".into()],
            &HashMap::new(),
        )
        .unwrap();
        let request_transport = Arc::clone(&transport);
        let (abort_handle, registration) = futures::future::AbortHandle::new_pair();
        let task = tokio::spawn(async move {
            futures::future::Abortable::new(
                request_transport.request("tools/call", None),
                registration,
            )
            .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if !transport.pending.lock().await.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("request should become pending");

        abort_handle.abort();
        assert!(task.await.unwrap().is_err());
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if transport.pending.lock().await.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled MCP request should be removed");
        transport.shutdown().await;
    }
}
