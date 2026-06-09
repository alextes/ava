use serde::Deserialize;
use serde_json::json;

use super::{ToolCall, ToolDefinition};

pub const EXEC_TOOL_NAME: &str = "exec";

const MAX_OUTPUT_CHARS: usize = 4000;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 300;

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
    const SENSITIVE_VARS: &[&str] = &[
        "ANTHROPIC_API_KEY",
        "DEEPSEEK_API_KEY",
        "GEMINI_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "TELEGRAM_BOT_TOKEN",
    ];
    SENSITIVE_VARS.iter().any(|var| command.contains(var))
}

#[derive(Debug, Deserialize)]
struct ExecInput {
    command: String,
    timeout_secs: Option<u64>,
    cwd: Option<String>,
}

pub(super) async fn handle_exec(call: &ToolCall) -> String {
    match serde_json::from_value::<ExecInput>(call.input.clone()) {
        Ok(input) => {
            execute_command(&input.command, input.timeout_secs, input.cwd.as_deref()).await
        }
        Err(err) => format!("invalid input: {err}"),
    }
}

async fn execute_command(command: &str, timeout_secs: Option<u64>, cwd: Option<&str>) -> String {
    // safety filter
    if let Some(reason) = check_safety_filter(command) {
        return reason.to_string();
    }

    let timeout = timeout_secs
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .min(MAX_TIMEOUT_SECS);

    tracing::info!(command, timeout, ?cwd, "executing command");

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // start in its own process group so we can kill the whole tree
        .process_group(0);
    if let Some(dir) = cwd {
        let path = std::path::Path::new(dir);
        if !path.is_dir() {
            return format!(
                "error: cwd '{dir}' does not exist on this host. \
                 use \".\" for the current working directory or provide a valid path."
            );
        }
        cmd.current_dir(dir);
    }

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return format!("failed to execute command: {e}"),
    };

    // save the process group id before wait_with_output consumes the child
    let pgid = child.id();

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout),
        child.wait_with_output(),
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
        Err(_) => {
            // kill the entire process group to clean up child processes silently
            if let Some(pid) = pgid {
                unsafe {
                    libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                }
            }
            format!("command timed out after {timeout}s")
        }
    }
}

pub(crate) fn truncate_output(output: &str) -> String {
    if output.len() <= MAX_OUTPUT_CHARS {
        return output.to_string();
    }
    let mut truncated: String = output.chars().take(MAX_OUTPUT_CHARS).collect();
    truncated.push_str("\n... (output truncated)");
    truncated
}

pub(super) fn exec_definition() -> ToolDefinition {
    ToolDefinition::Custom {
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
                },
                "cwd": {
                    "type": "string",
                    "description": "working directory for the command (default: process working directory)"
                }
            },
            "required": ["command"]
        }),
    }
}

/// load all vault file contents into memory for output scrubbing.
/// reads every file in ~/.ava/vault/, returns trimmed non-empty contents.
pub(crate) fn load_vault_secrets() -> Vec<String> {
    let vault = crate::config::vault_dir();
    let entries = match std::fs::read_dir(&vault) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut secrets = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let trimmed = content.trim().to_string();
                if !trimmed.is_empty() {
                    secrets.push(trimmed);
                }
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), %e, "failed to read vault file");
            }
        }
    }
    secrets
}

/// replace all occurrences of vault secret values in text with [REDACTED].
/// handles multi-line secrets by scrubbing each line independently.
pub(crate) fn scrub_vault_secrets(text: &str, secrets: &[String]) -> String {
    let mut result = text.to_string();
    for value in secrets {
        if value.is_empty() {
            continue;
        }
        result = result.replace(value.as_str(), "[REDACTED]");
        for line in value.lines() {
            let line = line.trim();
            if line.len() >= 8 {
                result = result.replace(line, "[REDACTED]");
            }
        }
    }
    result
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
        assert!(references_sensitive_env("echo $DEEPSEEK_API_KEY"));
        assert!(references_sensitive_env("echo $GEMINI_API_KEY"));
        assert!(references_sensitive_env("echo $OPENAI_API_KEY"));
        assert!(references_sensitive_env("echo $OPENROUTER_API_KEY"));
        assert!(references_sensitive_env("echo $TELEGRAM_BOT_TOKEN"));
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

    #[tokio::test]
    async fn test_execute_command_ls() {
        let result = execute_command("echo hello", None, None).await;
        assert!(result.contains("exit code: 0"));
        assert!(result.contains("hello"));
    }

    #[tokio::test]
    async fn test_execute_command_timeout() {
        let result = execute_command("sleep 10", Some(1), None).await;
        assert!(result.contains("timed out"));
    }

    #[tokio::test]
    async fn test_execute_command_safety_filter() {
        let result = execute_command("rm -rf /", None, None).await;
        assert!(result.contains("blocked"));
    }

    #[tokio::test]
    async fn test_execute_command_with_cwd() {
        let result = execute_command("pwd", None, Some("/tmp")).await;
        assert!(result.contains("exit code: 0"));
        assert!(result.contains("/tmp") || result.contains("/private/tmp"));
    }

    #[tokio::test]
    async fn test_execute_command_with_nonexistent_cwd() {
        let result = execute_command(
            "echo hi",
            None,
            Some("/nonexistent_dir_that_does_not_exist"),
        )
        .await;
        assert!(result.contains("does not exist on this host"));
    }

    // --- load_vault_secrets tests ---

    #[test]
    fn test_load_vault_secrets_from_dir() {
        let _guard = crate::config::ENV_TEST_LOCK.lock().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir(&vault).unwrap();
        std::fs::write(vault.join("db-url"), "postgres://admin:secret@db\n").unwrap();
        std::fs::write(vault.join("api-key"), "sk-abc123\n").unwrap();
        std::fs::write(vault.join("empty"), "").unwrap();

        unsafe { std::env::set_var("AVA_HOME", dir.path().to_str().unwrap()) };
        let secrets = load_vault_secrets();
        unsafe { std::env::remove_var("AVA_HOME") };

        assert_eq!(secrets.len(), 2);
        assert!(secrets.contains(&"postgres://admin:secret@db".to_string()));
        assert!(secrets.contains(&"sk-abc123".to_string()));
    }

    #[test]
    fn test_load_vault_secrets_no_dir() {
        let _guard = crate::config::ENV_TEST_LOCK.lock().unwrap();

        unsafe { std::env::set_var("AVA_HOME", "/nonexistent_ava_test_xyz") };
        let secrets = load_vault_secrets();
        unsafe { std::env::remove_var("AVA_HOME") };
        assert!(secrets.is_empty());
    }

    // --- scrub_vault_secrets tests ---

    #[test]
    fn test_scrub_vault_secrets_basic() {
        let secrets = vec!["postgres://admin:s3cret@db:5432".to_string()];
        let output = "connected to postgres://admin:s3cret@db:5432 ok";
        let scrubbed = scrub_vault_secrets(output, &secrets);
        assert_eq!(scrubbed, "connected to [REDACTED] ok");
    }

    #[test]
    fn test_scrub_vault_secrets_multiline() {
        let secrets = vec!["-----BEGIN KEY-----\nMIIBogIBAAJBALK\n-----END KEY-----".to_string()];
        let output = "key line: MIIBogIBAAJBALK";
        let scrubbed = scrub_vault_secrets(output, &secrets);
        assert_eq!(scrubbed, "key line: [REDACTED]");
    }

    #[test]
    fn test_scrub_vault_secrets_multiple() {
        let secrets = vec!["secretA123".to_string(), "secretB456".to_string()];
        let output = "found secretA123 and secretB456";
        let scrubbed = scrub_vault_secrets(output, &secrets);
        assert!(!scrubbed.contains("secretA123"));
        assert!(!scrubbed.contains("secretB456"));
    }

    #[test]
    fn test_scrub_vault_secrets_empty_list() {
        let output = "nothing to scrub";
        assert_eq!(scrub_vault_secrets(output, &[]), "nothing to scrub");
    }
}
