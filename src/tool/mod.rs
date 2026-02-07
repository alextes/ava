use std::future::Future;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::Database;
use crate::error::Error;
use crate::message::MessageContent;

pub const REMEMBER_FACT_TOOL_NAME: &str = "remember_fact";
pub const EXEC_TOOL_NAME: &str = "exec";
pub const WEB_SEARCH_TOOL_NAME: &str = "web_search";
pub const WEB_FETCH_TOOL_NAME: &str = "web_fetch";

const MAX_OUTPUT_CHARS: usize = 4000;
const BRAVE_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";
const DEFAULT_MAX_RESULTS: u64 = 5;
const MAX_MAX_RESULTS: u64 = 20;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 300;
const JINA_READER_BASE: &str = "https://r.jina.ai/";
const DEFAULT_FETCH_MAX_CHARS: u64 = 4000;
const FETCH_TIMEOUT_SECS: u64 = 30;

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
    vec![
        remember_fact_definition(),
        exec_definition(),
        web_search_definition(),
        web_fetch_definition(),
    ]
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

#[derive(Debug, Deserialize)]
struct WebSearchInput {
    query: String,
    max_results: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct WebFetchInput {
    url: String,
    max_chars: Option<u64>,
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
        WEB_SEARCH_TOOL_NAME => {
            match serde_json::from_value::<WebSearchInput>(call.input.clone()) {
                Ok(input) => {
                    let result = web_search(&input.query, input.max_results).await;
                    Ok(MessageContent::tool_result(&call.id, result))
                }
                Err(err) => Ok(MessageContent::tool_result(
                    &call.id,
                    format!("invalid input: {err}"),
                )),
            }
        }
        WEB_FETCH_TOOL_NAME => match serde_json::from_value::<WebFetchInput>(call.input.clone()) {
            Ok(input) => {
                let result = web_fetch(&input.url, input.max_chars).await;
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

// --- web search implementation ---

/// brave search API response types
#[derive(Debug, Deserialize)]
struct BraveSearchResponse {
    web: Option<BraveWebResults>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResults {
    results: Vec<BraveWebResult>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResult {
    title: String,
    url: String,
    description: Option<String>,
}

async fn web_search(query: &str, max_results: Option<u64>) -> String {
    let api_key = match std::env::var("BRAVE_SEARCH_API_KEY") {
        Ok(key) if !key.is_empty() => key,
        _ => return "web search unavailable: BRAVE_SEARCH_API_KEY not set".to_string(),
    };

    let count = max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .min(MAX_MAX_RESULTS);

    tracing::info!(query, count, "searching web");

    let client = reqwest::Client::new();
    let response = client
        .get(BRAVE_SEARCH_URL)
        .header("X-Subscription-Token", &api_key)
        .header("Accept", "application/json")
        .query(&[("q", query), ("count", &count.to_string())])
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => return format!("web search failed: {e}"),
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return format!("web search failed (HTTP {status}): {body}");
    }

    let parsed: BraveSearchResponse = match response.json().await {
        Ok(r) => r,
        Err(e) => return format!("failed to parse search results: {e}"),
    };

    let results = match parsed.web {
        Some(web) if !web.results.is_empty() => web.results,
        _ => return format!("no results found for: {query}"),
    };

    let mut output = String::new();
    for (i, result) in results.iter().enumerate() {
        if i > 0 {
            output.push('\n');
        }
        output.push_str(&format!("{}. {}\n   {}", i + 1, result.title, result.url));
        if let Some(desc) = &result.description
            && !desc.is_empty()
        {
            output.push_str(&format!("\n   {desc}"));
        }
    }

    truncate_output(&output)
}

// --- web fetch implementation ---

/// checks if a URL is safe to fetch (rejects local/internal targets)
fn validate_fetch_url(url: &str) -> Result<(), &'static str> {
    let lower = url.to_lowercase();

    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err("only http and https URLs are supported");
    }

    // extract host portion
    let after_scheme = if let Some(rest) = lower.strip_prefix("https://") {
        rest
    } else if let Some(rest) = lower.strip_prefix("http://") {
        rest
    } else {
        // unreachable due to the check above, but be safe
        return Err("only http and https URLs are supported");
    };
    let host = after_scheme.split('/').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or(host);

    if host == "localhost"
        || host == "127.0.0.1"
        || host == "[::1]"
        || host.ends_with(".local")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.starts_with("172.16.")
        || host.starts_with("169.254.")
    {
        return Err("fetching local/internal URLs is not allowed");
    }

    Ok(())
}

async fn web_fetch(url: &str, max_chars: Option<u64>) -> String {
    if let Err(reason) = validate_fetch_url(url) {
        return format!("invalid URL: {reason}");
    }

    let max = max_chars.unwrap_or(DEFAULT_FETCH_MAX_CHARS) as usize;
    let jina_url = format!("{JINA_READER_BASE}{url}");

    tracing::info!(url, "fetching web page");

    let client = reqwest::Client::new();
    let mut request = client
        .get(&jina_url)
        .header("Accept", "text/plain")
        .header("User-Agent", "ava/0.1");

    if let Ok(key) = std::env::var("JINA_API_KEY")
        && !key.is_empty()
    {
        request = request.header("Authorization", format!("Bearer {key}"));
    }

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(FETCH_TIMEOUT_SECS),
        request.send(),
    )
    .await;

    let response = match result {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return format!("failed to fetch URL: {e}"),
        Err(_) => return format!("fetch timed out after {FETCH_TIMEOUT_SECS}s"),
    };

    if !response.status().is_success() {
        let status = response.status();
        return format!("failed to fetch URL (HTTP {status})");
    }

    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => return format!("failed to read response: {e}"),
    };

    if body.trim().is_empty() {
        return "(no content)".to_string();
    }

    truncate_to_chars(&body, max)
}

fn truncate_to_chars(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max).collect();
    truncated.push_str("\n... (content truncated)");
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

fn web_search_definition() -> ToolDefinition {
    ToolDefinition {
        name: WEB_SEARCH_TOOL_NAME,
        description: "search the web using brave search. use this to find current information, look up documentation, or answer questions that require up-to-date knowledge.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "search query"
                },
                "max_results": {
                    "type": "integer",
                    "description": "maximum number of results to return (default 5, max 20)"
                }
            },
            "required": ["query"]
        }),
    }
}

fn web_fetch_definition() -> ToolDefinition {
    ToolDefinition {
        name: WEB_FETCH_TOOL_NAME,
        description: "fetch a web page and return its content as plain text. use this to read the full content of a URL found via web_search or provided by the user.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to fetch (must be http or https)"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "maximum number of characters to return (default 4000)"
                }
            },
            "required": ["url"]
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

    #[test]
    fn test_requires_approval_web_search() {
        let call = ToolCall {
            id: "test".into(),
            name: WEB_SEARCH_TOOL_NAME.into(),
            input: json!({"query": "rust lang"}),
        };
        assert!(!requires_approval(&call));
    }

    #[tokio::test]
    async fn test_web_search_missing_api_key() {
        // ensure the env var is not set for this test
        let _original = std::env::var("BRAVE_SEARCH_API_KEY").ok();
        unsafe {
            std::env::remove_var("BRAVE_SEARCH_API_KEY");
        }
        let result = web_search("test query", None).await;
        assert!(result.contains("BRAVE_SEARCH_API_KEY not set"));
        // restore if it was set
        if let Some(val) = _original {
            unsafe {
                std::env::set_var("BRAVE_SEARCH_API_KEY", val);
            }
        }
    }

    #[test]
    fn test_format_search_results() {
        let results = vec![
            BraveWebResult {
                title: "Rust Programming Language".into(),
                url: "https://www.rust-lang.org/".into(),
                description: Some(
                    "A language empowering everyone to build reliable software.".into(),
                ),
            },
            BraveWebResult {
                title: "Rust (programming language) - Wikipedia".into(),
                url: "https://en.wikipedia.org/wiki/Rust_(programming_language)".into(),
                description: None,
            },
        ];

        let mut output = String::new();
        for (i, result) in results.iter().enumerate() {
            if i > 0 {
                output.push('\n');
            }
            output.push_str(&format!("{}. {}\n   {}", i + 1, result.title, result.url));
            if let Some(desc) = &result.description {
                if !desc.is_empty() {
                    output.push_str(&format!("\n   {desc}"));
                }
            }
        }

        assert!(output.contains("1. Rust Programming Language"));
        assert!(output.contains("https://www.rust-lang.org/"));
        assert!(output.contains("A language empowering everyone"));
        assert!(output.contains("2. Rust (programming language) - Wikipedia"));
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

    #[test]
    fn test_requires_approval_web_fetch() {
        let call = ToolCall {
            id: "test".into(),
            name: WEB_FETCH_TOOL_NAME.into(),
            input: json!({"url": "https://example.com"}),
        };
        assert!(!requires_approval(&call));
    }

    #[test]
    fn test_validate_fetch_url_valid() {
        assert!(validate_fetch_url("https://example.com").is_ok());
        assert!(validate_fetch_url("http://example.com/page").is_ok());
        assert!(validate_fetch_url("https://docs.rs/reqwest/latest").is_ok());
    }

    #[test]
    fn test_validate_fetch_url_rejects_non_http() {
        assert!(validate_fetch_url("ftp://example.com").is_err());
        assert!(validate_fetch_url("file:///etc/passwd").is_err());
        assert!(validate_fetch_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn test_validate_fetch_url_rejects_internal() {
        assert!(validate_fetch_url("http://localhost/admin").is_err());
        assert!(validate_fetch_url("http://127.0.0.1:8080").is_err());
        assert!(validate_fetch_url("http://192.168.1.1").is_err());
        assert!(validate_fetch_url("http://10.0.0.1").is_err());
        assert!(validate_fetch_url("http://172.16.0.1").is_err());
    }

    #[test]
    fn test_truncate_to_chars_short() {
        let short = "hello world";
        assert_eq!(truncate_to_chars(short, 100), short);
    }

    #[test]
    fn test_truncate_to_chars_long() {
        let long = "x".repeat(5000);
        let result = truncate_to_chars(&long, 100);
        assert!(result.starts_with("xxxx"));
        assert!(result.ends_with("... (content truncated)"));
    }
}
