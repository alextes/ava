use std::future::Future;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::Database;
use crate::error::Error;
use crate::message::MessageContent;

pub const REMEMBER_FACT_TOOL_NAME: &str = "remember_fact";
pub const EXEC_TOOL_NAME: &str = "exec";

const MAX_OUTPUT_CHARS: usize = 4000;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 300;

// --- tool call types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: serde_json::Value,
}

// --- approver trait ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    AllowOnce,
    AllowAlways { pattern: String },
    Deny,
    AutoApproved,
}

pub trait Approver: Send + Sync {
    fn request_approval(
        &self,
        tool_call: &ToolCall,
    ) -> impl Future<Output = Result<ApprovalDecision, Error>> + Send;
}

/// auto-approves all tool calls (used for CLI)
pub struct CliApprover;

impl Approver for CliApprover {
    async fn request_approval(&self, _tool_call: &ToolCall) -> Result<ApprovalDecision, Error> {
        Ok(ApprovalDecision::AutoApproved)
    }
}

/// returns true if this tool call requires approval
pub fn requires_approval(tool_call: &ToolCall) -> bool {
    tool_call.name == EXEC_TOOL_NAME
}

// --- security filter ---

const BLOCKED_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    "mkfs",
    "dd if=",
    "> /dev/sd",
    ":(){ :|:& };:", // fork bomb
    ".fork",         // another fork bomb pattern
];

/// returns Some(reason) if the command is blocked by the safety filter
fn check_safety_filter(command: &str) -> Option<&'static str> {
    let trimmed = command.trim();
    for pattern in BLOCKED_PATTERNS {
        if trimmed.contains(pattern) {
            return Some("command blocked: matches safety filter");
        }
    }
    None
}

/// returns true if the command references sensitive env vars
pub fn references_sensitive_env(command: &str) -> bool {
    const SENSITIVE_VARS: &[&str] = &["ANTHROPIC_API_KEY", "TELOXIDE_TOKEN"];
    SENSITIVE_VARS.iter().any(|var| command.contains(var))
}

// --- tool definitions ---

pub fn tool_definitions() -> Vec<ToolDefinition> {
    vec![remember_fact_definition(), exec_definition()]
}

// --- tool dispatch ---

#[derive(Debug, Deserialize)]
struct RememberFactInput {
    category: String,
    key: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct ExecInput {
    command: String,
    timeout_secs: Option<u64>,
}

pub async fn handle_tool_call(db: &Database, call: &ToolCall) -> Result<MessageContent, Error> {
    tracing::info!(tool = %call.name, "handling tool call");
    match call.name.as_str() {
        REMEMBER_FACT_TOOL_NAME => {
            match serde_json::from_value::<RememberFactInput>(call.input.clone()) {
                Ok(input) => {
                    db.remember_fact(&input.category, &input.key, &input.value)?;
                    Ok(MessageContent::tool_result(&call.id, "ok"))
                }
                Err(err) => Ok(MessageContent::tool_result(
                    &call.id,
                    format!("invalid input: {err}"),
                )),
            }
        }
        EXEC_TOOL_NAME => match serde_json::from_value::<ExecInput>(call.input.clone()) {
            Ok(input) => {
                let result = execute_command(&input.command, input.timeout_secs).await;
                Ok(MessageContent::tool_result(&call.id, result))
            }
            Err(err) => Ok(MessageContent::tool_result(
                &call.id,
                format!("invalid input: {err}"),
            )),
        },
        _ => {
            tracing::warn!(tool = %call.name, "unknown tool");
            Ok(MessageContent::tool_result(
                &call.id,
                format!("unknown tool: {}", call.name),
            ))
        }
    }
}

// --- exec implementation ---

async fn execute_command(command: &str, timeout_secs: Option<u64>) -> String {
    // safety filter
    if let Some(reason) = check_safety_filter(command) {
        return reason.to_string();
    }

    let timeout = timeout_secs
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .min(MAX_TIMEOUT_SECS);

    tracing::info!(command, timeout, "executing command");

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout),
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output(),
    )
    .await;

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let code = output.status.code().unwrap_or(-1);

            let mut result = format!("exit code: {code}");

            if !stdout.is_empty() {
                result.push_str("\nstdout:\n");
                result.push_str(&stdout);
            }

            if !stderr.is_empty() {
                result.push_str("\nstderr:\n");
                result.push_str(&stderr);
            }

            if stdout.is_empty() && stderr.is_empty() {
                result.push_str("\n(no output)");
            }

            truncate_output(&result)
        }
        Ok(Err(e)) => format!("failed to execute command: {e}"),
        Err(_) => format!("command timed out after {timeout}s"),
    }
}

fn truncate_output(output: &str) -> String {
    if output.len() <= MAX_OUTPUT_CHARS {
        return output.to_string();
    }
    let mut truncated: String = output.chars().take(MAX_OUTPUT_CHARS).collect();
    truncated.push_str("\n... (output truncated)");
    truncated
}

// --- tool definition builders ---

fn remember_fact_definition() -> ToolDefinition {
    ToolDefinition {
        name: REMEMBER_FACT_TOOL_NAME,
        description: "store a user fact for future conversations",
        input_schema: json!({
            "type": "object",
            "properties": {
                "category": {
                    "type": "string",
                    "description": "fact namespace, such as user or preferences"
                },
                "key": {
                    "type": "string",
                    "description": "fact key within the category"
                },
                "value": {
                    "type": "string",
                    "description": "fact value to store"
                }
            },
            "required": ["category", "key", "value"]
        }),
    }
}

fn exec_definition() -> ToolDefinition {
    ToolDefinition {
        name: EXEC_TOOL_NAME,
        description: "execute a shell command via sh -c. use this to run commands on the host system. the user may need to approve the command before it runs.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "shell command to run via sh -c"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "timeout in seconds (default 30, max 300)"
                }
            },
            "required": ["command"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safety_filter_blocks_rm_rf_root() {
        assert!(check_safety_filter("rm -rf /").is_some());
        assert!(check_safety_filter("rm -rf /*").is_some());
    }

    #[test]
    fn test_safety_filter_blocks_fork_bomb() {
        assert!(check_safety_filter(":(){ :|:& };:").is_some());
    }

    #[test]
    fn test_safety_filter_blocks_mkfs() {
        assert!(check_safety_filter("mkfs.ext4 /dev/sda1").is_some());
    }

    #[test]
    fn test_safety_filter_allows_normal_commands() {
        assert!(check_safety_filter("ls -la").is_none());
        assert!(check_safety_filter("cargo test").is_none());
        assert!(check_safety_filter("echo hello").is_none());
    }

    #[test]
    fn test_references_sensitive_env() {
        assert!(references_sensitive_env("echo $ANTHROPIC_API_KEY"));
        assert!(references_sensitive_env("echo $TELOXIDE_TOKEN"));
        assert!(!references_sensitive_env("echo hello"));
    }

    #[test]
    fn test_truncate_output_short() {
        let short = "hello world";
        assert_eq!(truncate_output(short), short);
    }

    #[test]
    fn test_truncate_output_long() {
        let long = "x".repeat(MAX_OUTPUT_CHARS + 100);
        let result = truncate_output(&long);
        assert!(result.len() < long.len());
        assert!(result.ends_with("... (output truncated)"));
    }

    #[test]
    fn test_requires_approval_exec() {
        let call = ToolCall {
            id: "test".into(),
            name: EXEC_TOOL_NAME.into(),
            input: json!({"command": "ls"}),
        };
        assert!(requires_approval(&call));
    }

    #[test]
    fn test_requires_approval_remember_fact() {
        let call = ToolCall {
            id: "test".into(),
            name: REMEMBER_FACT_TOOL_NAME.into(),
            input: json!({"category": "user", "key": "name", "value": "alex"}),
        };
        assert!(!requires_approval(&call));
    }

    #[tokio::test]
    async fn test_execute_command_ls() {
        let result = execute_command("echo hello", None).await;
        assert!(result.contains("exit code: 0"));
        assert!(result.contains("hello"));
    }

    #[tokio::test]
    async fn test_execute_command_timeout() {
        let result = execute_command("sleep 10", Some(1)).await;
        assert!(result.contains("timed out"));
    }

    #[tokio::test]
    async fn test_execute_command_safety_filter() {
        let result = execute_command("rm -rf /", None).await;
        assert!(result.contains("blocked"));
    }

    #[tokio::test]
    async fn test_cli_approver_auto_approves() {
        let approver = CliApprover;
        let call = ToolCall {
            id: "test".into(),
            name: EXEC_TOOL_NAME.into(),
            input: json!({"command": "ls"}),
        };
        let decision = approver.request_approval(&call).await.unwrap();
        assert_eq!(decision, ApprovalDecision::AutoApproved);
    }
}
