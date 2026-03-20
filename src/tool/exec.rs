use std::collections::HashMap;

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
    const SENSITIVE_VARS: &[&str] = &["ANTHROPIC_API_KEY", "TELEGRAM_BOT_TOKEN"];
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
    cmd.arg("-c").arg(command);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let result = tokio::time::timeout(std::time::Duration::from_secs(timeout), cmd.output()).await;

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

/// resolve a vault:// secret source by reading from ~/.ava/vault/<name>.
/// returns the secret value or an error message.
pub(crate) fn resolve_vault_secret(source: &str) -> Result<String, String> {
    let name = source
        .strip_prefix("vault://")
        .ok_or_else(|| format!("not a vault source: {source}"))?;

    let path = crate::config::vault_dir().join(name);
    std::fs::read_to_string(&path)
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("failed to read vault secret '{}': {e}", path.display()))
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

/// replace all occurrences of secret values in text with [REDACTED].
/// handles multi-line secrets by scrubbing each line independently.
pub(crate) fn scrub_secrets(text: &str, secrets: &HashMap<String, String>) -> String {
    let mut result = text.to_string();
    for value in secrets.values() {
        if value.is_empty() {
            continue;
        }
        // scrub the full value
        result = result.replace(value.as_str(), "[REDACTED]");
        // also scrub individual lines for multi-line secrets (e.g. PEM keys)
        for line in value.lines() {
            let line = line.trim();
            if line.len() >= 8 {
                result = result.replace(line, "[REDACTED]");
            }
        }
    }
    result
}

/// execute a command with secrets injected as env vars.
/// secret values are scrubbed from the output before returning.
/// the agent never sees the raw secret values.
pub(crate) async fn sealed_exec(
    command: &str,
    secrets: &HashMap<String, String>,
    timeout_secs: Option<u64>,
    cwd: Option<&str>,
) -> String {
    if let Some(reason) = check_safety_filter(command) {
        return reason.to_string();
    }

    let timeout = timeout_secs
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .min(MAX_TIMEOUT_SECS);

    tracing::info!(
        command,
        timeout,
        ?cwd,
        secret_count = secrets.len(),
        "sealed execution"
    );

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    // inject secrets as env vars — only for this command's process
    for (name, value) in secrets {
        cmd.env(name, value);
    }

    let result = tokio::time::timeout(std::time::Duration::from_secs(timeout), cmd.output()).await;

    let raw_output = match result {
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

            result
        }
        Ok(Err(e)) => format!("failed to execute command: {e}"),
        Err(_) => format!("command timed out after {timeout}s"),
    };

    // scrub secrets from output before it reaches the agent
    let scrubbed = scrub_secrets(&raw_output, secrets);
    truncate_output(&scrubbed)
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
        assert!(result.contains("failed to execute command"));
    }

    // --- load_vault_secrets tests ---

    #[test]
    fn test_load_vault_secrets_from_dir() {
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

    // --- scrub_secrets tests ---

    #[test]
    fn test_scrub_secrets_basic() {
        let mut secrets = HashMap::new();
        secrets.insert("DB_URL".into(), "postgres://admin:s3cret@db:5432".into());

        let output = "connected to postgres://admin:s3cret@db:5432 successfully";
        let scrubbed = scrub_secrets(output, &secrets);
        assert_eq!(scrubbed, "connected to [REDACTED] successfully");
        assert!(!scrubbed.contains("s3cret"));
    }

    #[test]
    fn test_scrub_secrets_multiline() {
        let mut secrets = HashMap::new();
        secrets.insert(
            "KEY".into(),
            "-----BEGIN KEY-----\nMIIBogIBAAJBALK\n-----END KEY-----".into(),
        );

        let output = "loaded key: MIIBogIBAAJBALK";
        let scrubbed = scrub_secrets(output, &secrets);
        assert_eq!(scrubbed, "loaded key: [REDACTED]");
    }

    #[test]
    fn test_scrub_secrets_multiple() {
        let mut secrets = HashMap::new();
        secrets.insert("USER".into(), "admin".into());
        secrets.insert("PASS".into(), "hunter2".into());

        let output = "login: admin / hunter2";
        let scrubbed = scrub_secrets(output, &secrets);
        assert!(!scrubbed.contains("admin"));
        assert!(!scrubbed.contains("hunter2"));
    }

    #[test]
    fn test_scrub_secrets_empty_value_skipped() {
        let mut secrets = HashMap::new();
        secrets.insert("EMPTY".into(), "".into());

        let output = "nothing to scrub";
        let scrubbed = scrub_secrets(output, &secrets);
        assert_eq!(scrubbed, "nothing to scrub");
    }

    #[test]
    fn test_scrub_secrets_short_lines_skipped() {
        let mut secrets = HashMap::new();
        secrets.insert("KEY".into(), "line1\nab\nlong_enough_line".into());

        // "ab" is too short (< 8 chars) to scrub individually, avoids false positives
        let output = "found: ab and long_enough_line";
        let scrubbed = scrub_secrets(output, &secrets);
        assert!(scrubbed.contains("ab"));
        assert!(!scrubbed.contains("long_enough_line"));
    }

    // --- sealed_exec tests ---

    #[tokio::test]
    async fn test_sealed_exec_injects_env() {
        let mut secrets = HashMap::new();
        secrets.insert("MY_SECRET".into(), "supersecret123".into());

        let result = sealed_exec("echo $MY_SECRET", &secrets, None, None).await;
        // the output should have the secret scrubbed
        assert!(result.contains("[REDACTED]"));
        assert!(!result.contains("supersecret123"));
    }

    #[tokio::test]
    async fn test_sealed_exec_scrubs_stderr() {
        let mut secrets = HashMap::new();
        secrets.insert("TOKEN".into(), "tok_abc123xyz".into());

        let result = sealed_exec("echo tok_abc123xyz >&2", &secrets, None, None).await;
        assert!(result.contains("[REDACTED]"));
        assert!(!result.contains("tok_abc123xyz"));
    }

    #[tokio::test]
    async fn test_sealed_exec_no_secrets_works_normally() {
        let secrets = HashMap::new();
        let result = sealed_exec("echo hello", &secrets, None, None).await;
        assert!(result.contains("hello"));
        assert!(result.contains("exit code: 0"));
    }

    // --- resolve_vault_secret tests ---

    #[test]
    fn test_resolve_vault_secret_not_vault_source() {
        let result = resolve_vault_secret("op://Private/key");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a vault source"));
    }

    #[test]
    fn test_resolve_vault_secret_missing_file() {
        let result = resolve_vault_secret("vault://nonexistent-secret-xyz");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed to read"));
    }

    #[test]
    fn test_resolve_vault_secret_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir(&vault).unwrap();
        std::fs::write(vault.join("test-secret"), "my-secret-value\n").unwrap();

        // temporarily override AVA_HOME
        // SAFETY: test-only, single-threaded context
        unsafe {
            std::env::set_var("AVA_HOME", dir.path().to_str().unwrap());
        }

        let result = resolve_vault_secret("vault://test-secret");
        assert_eq!(result.unwrap(), "my-secret-value");

        unsafe {
            std::env::remove_var("AVA_HOME");
        }
    }
}
