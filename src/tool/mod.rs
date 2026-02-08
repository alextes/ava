use std::future::Future;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::{Database, MemoryKind};
use crate::error::Error;
use crate::message::MessageContent;
use crate::provider::AnyProvider;

pub const REMEMBER_TOOL_NAME: &str = "remember";
pub const FORGET_TOOL_NAME: &str = "forget";
pub const RECALL_TOOL_NAME: &str = "recall";
pub const EXEC_TOOL_NAME: &str = "exec";
pub const WEB_SEARCH_TOOL_NAME: &str = "web_search";
pub const WEB_FETCH_TOOL_NAME: &str = "web_fetch";
pub const SWITCH_MODEL_TOOL_NAME: &str = "switch_model";
pub const MANAGE_RULES_TOOL_NAME: &str = "manage_rules";

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

pub struct ToolCallResult {
    pub content: MessageContent,
    pub switch_provider: Option<AnyProvider>,
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
        remember_definition(),
        forget_definition(),
        recall_definition(),
        exec_definition(),
        web_search_definition(),
        web_fetch_definition(),
        switch_model_definition(),
        manage_rules_definition(),
    ]
}

// --- tool dispatch ---

#[derive(Debug, Deserialize)]
struct RememberInput {
    content: String,
    kind: String,
    category: Option<String>,
    key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ForgetInput {
    kind: String,
    category: Option<String>,
    key: Option<String>,
    id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RecallInput {
    query: String,
    limit: Option<u32>,
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

#[derive(Debug, Deserialize)]
struct SwitchModelInput {
    provider: String,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManageRulesInput {
    action: String,
    id: Option<i64>,
}

pub async fn handle_tool_call(db: &Database, call: &ToolCall) -> Result<ToolCallResult, Error> {
    tracing::info!(tool = %call.name, "handling tool call");
    match call.name.as_str() {
        REMEMBER_TOOL_NAME => match serde_json::from_value::<RememberInput>(call.input.clone()) {
            Ok(input) => {
                let kind = match MemoryKind::from_str(&input.kind) {
                    Some(k) => k,
                    None => {
                        return Ok(ToolCallResult {
                            content: MessageContent::tool_result(
                                &call.id,
                                format!("invalid kind: {}", input.kind),
                            ),
                            switch_provider: None,
                        });
                    }
                };
                let id = db.remember(
                    kind,
                    &input.content,
                    input.category.as_deref(),
                    input.key.as_deref(),
                )?;
                Ok(ToolCallResult {
                    content: MessageContent::tool_result(&call.id, format!("ok (id={id})")),
                    switch_provider: None,
                })
            }
            Err(err) => Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, format!("invalid input: {err}")),
                switch_provider: None,
            }),
        },
        FORGET_TOOL_NAME => match serde_json::from_value::<ForgetInput>(call.input.clone()) {
            Ok(input) => {
                let deleted = match input.kind.as_str() {
                    "fact" => {
                        let cat = input.category.as_deref().unwrap_or("");
                        let key = input.key.as_deref().unwrap_or("");
                        db.forget_fact(cat, key)?
                    }
                    "character" => {
                        let key = input.key.as_deref().unwrap_or("");
                        db.forget_character(key)?
                    }
                    "episode" => match input.id {
                        Some(id) => db.forget_memory(id)?,
                        None => {
                            return Ok(ToolCallResult {
                                content: MessageContent::tool_result(
                                    &call.id,
                                    "episode forget requires id",
                                ),
                                switch_provider: None,
                            });
                        }
                    },
                    other => {
                        return Ok(ToolCallResult {
                            content: MessageContent::tool_result(
                                &call.id,
                                format!("invalid kind: {other}"),
                            ),
                            switch_provider: None,
                        });
                    }
                };
                let msg = if deleted { "deleted" } else { "not found" };
                Ok(ToolCallResult {
                    content: MessageContent::tool_result(&call.id, msg),
                    switch_provider: None,
                })
            }
            Err(err) => Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, format!("invalid input: {err}")),
                switch_provider: None,
            }),
        },
        RECALL_TOOL_NAME => match serde_json::from_value::<RecallInput>(call.input.clone()) {
            Ok(input) => {
                let limit = input.limit.unwrap_or(10).min(50);
                let memories = db.search_memories(&input.query, limit)?;
                if memories.is_empty() {
                    return Ok(ToolCallResult {
                        content: MessageContent::tool_result(&call.id, "no memories found"),
                        switch_provider: None,
                    });
                }
                let mut output = String::new();
                for (i, m) in memories.iter().enumerate() {
                    if i > 0 {
                        output.push('\n');
                    }
                    match m.kind {
                        MemoryKind::Fact => {
                            let cat = m.category.as_deref().unwrap_or("?");
                            let key = m.key.as_deref().unwrap_or("?");
                            output.push_str(&format!("[fact] {cat}/{key}: {}", m.content));
                        }
                        MemoryKind::Episode => {
                            let date = m.created_at.split(' ').next().unwrap_or(&m.created_at);
                            output.push_str(&format!("[episode] {date}: {}", m.content));
                        }
                        MemoryKind::Character => {
                            let key = m.key.as_deref().unwrap_or("?");
                            output.push_str(&format!("[character] {key}: {}", m.content));
                        }
                    }
                }
                Ok(ToolCallResult {
                    content: MessageContent::tool_result(&call.id, output),
                    switch_provider: None,
                })
            }
            Err(err) => Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, format!("invalid input: {err}")),
                switch_provider: None,
            }),
        },
        EXEC_TOOL_NAME => match serde_json::from_value::<ExecInput>(call.input.clone()) {
            Ok(input) => {
                let result = execute_command(&input.command, input.timeout_secs).await;
                Ok(ToolCallResult {
                    content: MessageContent::tool_result(&call.id, result),
                    switch_provider: None,
                })
            }
            Err(err) => Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, format!("invalid input: {err}")),
                switch_provider: None,
            }),
        },
        WEB_SEARCH_TOOL_NAME => {
            match serde_json::from_value::<WebSearchInput>(call.input.clone()) {
                Ok(input) => {
                    let result = web_search(&input.query, input.max_results).await;
                    Ok(ToolCallResult {
                        content: MessageContent::tool_result(&call.id, result),
                        switch_provider: None,
                    })
                }
                Err(err) => Ok(ToolCallResult {
                    content: MessageContent::tool_result(&call.id, format!("invalid input: {err}")),
                    switch_provider: None,
                }),
            }
        }
        WEB_FETCH_TOOL_NAME => match serde_json::from_value::<WebFetchInput>(call.input.clone()) {
            Ok(input) => {
                let result = web_fetch(&input.url, input.max_chars).await;
                Ok(ToolCallResult {
                    content: MessageContent::tool_result(&call.id, result),
                    switch_provider: None,
                })
            }
            Err(err) => Ok(ToolCallResult {
                content: MessageContent::tool_result(&call.id, format!("invalid input: {err}")),
                switch_provider: None,
            }),
        },
        SWITCH_MODEL_TOOL_NAME => {
            match serde_json::from_value::<SwitchModelInput>(call.input.clone()) {
                Ok(input) => {
                    match AnyProvider::from_name(&input.provider, input.model.as_deref()) {
                        Ok(provider) => {
                            let model_info = input.model.as_deref().unwrap_or("default");
                            Ok(ToolCallResult {
                                content: MessageContent::tool_result(
                                    &call.id,
                                    format!(
                                        "switched to provider: {}, model: {model_info}",
                                        input.provider
                                    ),
                                ),
                                switch_provider: Some(provider),
                            })
                        }
                        Err(err) => Ok(ToolCallResult {
                            content: MessageContent::tool_result(
                                &call.id,
                                format!("failed to switch: {err}"),
                            ),
                            switch_provider: None,
                        }),
                    }
                }
                Err(err) => Ok(ToolCallResult {
                    content: MessageContent::tool_result(&call.id, format!("invalid input: {err}")),
                    switch_provider: None,
                }),
            }
        }
        MANAGE_RULES_TOOL_NAME => {
            match serde_json::from_value::<ManageRulesInput>(call.input.clone()) {
                Ok(input) => match input.action.as_str() {
                    "list" => {
                        let rules = db.list_approval_rules()?;
                        if rules.is_empty() {
                            return Ok(ToolCallResult {
                                content: MessageContent::tool_result(
                                    &call.id,
                                    "no approval rules saved",
                                ),
                                switch_provider: None,
                            });
                        }
                        let mut output = String::new();
                        for (i, rule) in rules.iter().enumerate() {
                            if i > 0 {
                                output.push('\n');
                            }
                            output.push_str(&format!("id={}: {}", rule.id, rule.pattern));
                        }
                        Ok(ToolCallResult {
                            content: MessageContent::tool_result(&call.id, output),
                            switch_provider: None,
                        })
                    }
                    "delete" => match input.id {
                        Some(id) => {
                            let deleted = db.delete_approval_rule(id)?;
                            let msg = if deleted { "deleted" } else { "not found" };
                            Ok(ToolCallResult {
                                content: MessageContent::tool_result(&call.id, msg),
                                switch_provider: None,
                            })
                        }
                        None => Ok(ToolCallResult {
                            content: MessageContent::tool_result(&call.id, "delete requires id"),
                            switch_provider: None,
                        }),
                    },
                    other => Ok(ToolCallResult {
                        content: MessageContent::tool_result(
                            &call.id,
                            format!("invalid action: {other}"),
                        ),
                        switch_provider: None,
                    }),
                },
                Err(err) => Ok(ToolCallResult {
                    content: MessageContent::tool_result(&call.id, format!("invalid input: {err}")),
                    switch_provider: None,
                }),
            }
        }
        _ => {
            tracing::warn!(tool = %call.name, "unknown tool");
            Ok(ToolCallResult {
                content: MessageContent::tool_result(
                    &call.id,
                    format!("unknown tool: {}", call.name),
                ),
                switch_provider: None,
            })
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

fn remember_definition() -> ToolDefinition {
    ToolDefinition {
        name: REMEMBER_TOOL_NAME,
        description: "store something in long-term memory. kind=fact: structured knowledge (requires category + key, e.g. user/name: alex). kind=episode: events, decisions, context worth preserving. kind=character: persona traits that shape behavior (requires key).",
        input_schema: json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "the value or text to remember"
                },
                "kind": {
                    "type": "string",
                    "enum": ["fact", "episode", "character"],
                    "description": "memory type"
                },
                "category": {
                    "type": "string",
                    "description": "fact namespace (required for kind=fact)"
                },
                "key": {
                    "type": "string",
                    "description": "key within category (required for kind=fact and kind=character)"
                }
            },
            "required": ["content", "kind"]
        }),
    }
}

fn forget_definition() -> ToolDefinition {
    ToolDefinition {
        name: FORGET_TOOL_NAME,
        description: "delete a memory. for facts: provide kind+category+key. for character traits: provide kind+key. for episodes: provide kind+id.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["fact", "episode", "character"],
                    "description": "memory type to delete"
                },
                "category": {
                    "type": "string",
                    "description": "fact category (for kind=fact)"
                },
                "key": {
                    "type": "string",
                    "description": "key (for kind=fact or kind=character)"
                },
                "id": {
                    "type": "integer",
                    "description": "memory id (for kind=episode)"
                }
            },
            "required": ["kind"]
        }),
    }
}

fn recall_definition() -> ToolDefinition {
    ToolDefinition {
        name: RECALL_TOOL_NAME,
        description: "search stored memories by keyword or phrase. use this proactively to look up past context when relevant.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "search query"
                },
                "limit": {
                    "type": "integer",
                    "description": "max results to return (default 10, max 50)"
                }
            },
            "required": ["query"]
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

fn switch_model_definition() -> ToolDefinition {
    ToolDefinition {
        name: SWITCH_MODEL_TOOL_NAME,
        description: "switch the ai provider and model for the remainder of this conversation. use this to delegate to a different model (e.g. a cheaper one for simple tasks, or a more capable one for hard tasks).",
        input_schema: json!({
            "type": "object",
            "properties": {
                "provider": {
                    "type": "string",
                    "enum": ["anthropic", "openai"],
                    "description": "the provider to switch to"
                },
                "model": {
                    "type": "string",
                    "enum": ["claude-opus-4-6", "claude-sonnet-4-5", "claude-haiku-4-5", "gpt-5.2", "gpt-5-mini"],
                    "description": "model name. must match the chosen provider. anthropic: claude-opus-4-6, claude-sonnet-4-5, claude-haiku-4-5. openai: gpt-5.2, gpt-5-mini. if omitted, uses the provider's default."
                }
            },
            "required": ["provider"]
        }),
    }
}

fn manage_rules_definition() -> ToolDefinition {
    ToolDefinition {
        name: MANAGE_RULES_TOOL_NAME,
        description: "manage approval rules for command execution. action=list: show all saved rules. action=delete: remove a rule by id.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "delete"],
                    "description": "action to perform"
                },
                "id": {
                    "type": "integer",
                    "description": "rule id to delete (required for action=delete)"
                }
            },
            "required": ["action"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approver::CliApprover;
    use crate::db::Database;

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
    fn test_requires_approval_remember() {
        let call = ToolCall {
            id: "test".into(),
            name: REMEMBER_TOOL_NAME.into(),
            input: json!({"content": "alex", "kind": "fact", "category": "user", "key": "name"}),
        };
        assert!(!requires_approval(&call));
    }

    #[test]
    fn test_requires_approval_forget() {
        let call = ToolCall {
            id: "test".into(),
            name: FORGET_TOOL_NAME.into(),
            input: json!({"kind": "fact", "category": "user", "key": "name"}),
        };
        assert!(!requires_approval(&call));
    }

    #[test]
    fn test_requires_approval_recall() {
        let call = ToolCall {
            id: "test".into(),
            name: RECALL_TOOL_NAME.into(),
            input: json!({"query": "rust"}),
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
        let results = [
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
            if let Some(desc) = &result.description
                && !desc.is_empty()
            {
                output.push_str(&format!("\n   {desc}"));
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

    // --- handle_tool_call integration tests ---

    fn make_call(name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "test_id".into(),
            name: name.into(),
            input,
        }
    }

    fn extract_tool_result_text(content: &MessageContent) -> &str {
        match content {
            MessageContent::ToolResult { content, .. } => content.as_str(),
            _ => panic!("expected ToolResult"),
        }
    }

    // remember tool

    #[tokio::test]
    async fn test_handle_remember_fact() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call(
            "remember",
            json!({"content": "alex", "kind": "fact", "category": "user", "key": "name"}),
        );
        let result = handle_tool_call(&db, &call).await.unwrap();
        let text = extract_tool_result_text(&result.content);
        assert!(text.starts_with("ok (id="));
        assert!(result.switch_provider.is_none());

        // verify persisted
        let facts = db.recent_facts().unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "alex");
    }

    #[tokio::test]
    async fn test_handle_remember_episode() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call(
            "remember",
            json!({"content": "discussed migration plan", "kind": "episode"}),
        );
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert!(extract_tool_result_text(&result.content).starts_with("ok (id="));

        let episodes = db.recent_episodes().unwrap();
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].content, "discussed migration plan");
    }

    #[tokio::test]
    async fn test_handle_remember_character() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call(
            "remember",
            json!({"content": "formal and precise", "kind": "character", "key": "tone"}),
        );
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert!(extract_tool_result_text(&result.content).starts_with("ok (id="));

        let traits = db.character_traits().unwrap();
        assert_eq!(traits.len(), 1);
        assert_eq!(traits[0].content, "formal and precise");
    }

    #[tokio::test]
    async fn test_handle_remember_invalid_kind() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("remember", json!({"content": "test", "kind": "bogus"}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "invalid kind: bogus"
        );
    }

    #[tokio::test]
    async fn test_handle_remember_missing_fields() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("remember", json!({"kind": "fact"}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        let text = extract_tool_result_text(&result.content);
        assert!(text.starts_with("invalid input:"));
    }

    #[tokio::test]
    async fn test_handle_remember_fact_upserts() {
        let db = Database::open_in_memory().unwrap();
        let call1 = make_call(
            "remember",
            json!({"content": "v1", "kind": "fact", "category": "user", "key": "name"}),
        );
        let call2 = make_call(
            "remember",
            json!({"content": "v2", "kind": "fact", "category": "user", "key": "name"}),
        );
        handle_tool_call(&db, &call1).await.unwrap();
        handle_tool_call(&db, &call2).await.unwrap();

        let facts = db.recent_facts().unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "v2");
    }

    // forget tool

    #[tokio::test]
    async fn test_handle_forget_fact() {
        let db = Database::open_in_memory().unwrap();
        db.remember(MemoryKind::Fact, "alex", Some("user"), Some("name"))
            .unwrap();

        let call = make_call(
            "forget",
            json!({"kind": "fact", "category": "user", "key": "name"}),
        );
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert_eq!(extract_tool_result_text(&result.content), "deleted");
        assert!(db.recent_facts().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_handle_forget_character() {
        let db = Database::open_in_memory().unwrap();
        db.remember(MemoryKind::Character, "formal", None, Some("tone"))
            .unwrap();

        let call = make_call("forget", json!({"kind": "character", "key": "tone"}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert_eq!(extract_tool_result_text(&result.content), "deleted");
        assert!(db.character_traits().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_handle_forget_episode_by_id() {
        let db = Database::open_in_memory().unwrap();
        let id = db
            .remember(MemoryKind::Episode, "some event", None, None)
            .unwrap();

        let call = make_call("forget", json!({"kind": "episode", "id": id}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert_eq!(extract_tool_result_text(&result.content), "deleted");
    }

    #[tokio::test]
    async fn test_handle_forget_episode_missing_id() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("forget", json!({"kind": "episode"}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "episode forget requires id"
        );
    }

    #[tokio::test]
    async fn test_handle_forget_not_found() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call(
            "forget",
            json!({"kind": "fact", "category": "user", "key": "nonexistent"}),
        );
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert_eq!(extract_tool_result_text(&result.content), "not found");
    }

    #[tokio::test]
    async fn test_handle_forget_invalid_kind() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("forget", json!({"kind": "bogus"}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "invalid kind: bogus"
        );
    }

    // recall tool

    #[tokio::test]
    async fn test_handle_recall_finds_results() {
        let db = Database::open_in_memory().unwrap();
        db.remember(
            MemoryKind::Fact,
            "loves rust programming",
            Some("user"),
            Some("hobby"),
        )
        .unwrap();
        db.remember(
            MemoryKind::Episode,
            "discussed python migration",
            None,
            None,
        )
        .unwrap();
        db.remember(MemoryKind::Character, "formal", None, Some("tone"))
            .unwrap();

        let call = make_call("recall", json!({"query": "rust"}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        let text = extract_tool_result_text(&result.content);
        assert!(text.contains("[fact] user/hobby: loves rust programming"));
    }

    #[tokio::test]
    async fn test_handle_recall_no_results() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("recall", json!({"query": "nonexistent"}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "no memories found"
        );
    }

    #[tokio::test]
    async fn test_handle_recall_formats_all_kinds() {
        let db = Database::open_in_memory().unwrap();
        db.remember(
            MemoryKind::Fact,
            "rust developer",
            Some("user"),
            Some("role"),
        )
        .unwrap();
        db.remember(MemoryKind::Episode, "discussed rust project", None, None)
            .unwrap();
        db.remember(
            MemoryKind::Character,
            "rust enthusiast",
            None,
            Some("personality"),
        )
        .unwrap();

        let call = make_call("recall", json!({"query": "rust"}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        let text = extract_tool_result_text(&result.content);
        assert!(text.contains("[fact]"));
        assert!(text.contains("[episode]"));
        assert!(text.contains("[character]"));
    }

    // manage_rules tool

    #[tokio::test]
    async fn test_handle_manage_rules_list_empty() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("manage_rules", json!({"action": "list"}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "no approval rules saved"
        );
    }

    #[tokio::test]
    async fn test_handle_manage_rules_list() {
        let db = Database::open_in_memory().unwrap();
        db.save_approval_rule("ls *").unwrap();
        db.save_approval_rule("cargo *").unwrap();

        let call = make_call("manage_rules", json!({"action": "list"}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        let text = extract_tool_result_text(&result.content);
        assert!(text.contains("ls *"));
        assert!(text.contains("cargo *"));
    }

    #[tokio::test]
    async fn test_handle_manage_rules_delete() {
        let db = Database::open_in_memory().unwrap();
        db.save_approval_rule("ls *").unwrap();
        let rules = db.list_approval_rules().unwrap();

        let call = make_call(
            "manage_rules",
            json!({"action": "delete", "id": rules[0].id}),
        );
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert_eq!(extract_tool_result_text(&result.content), "deleted");
        assert!(db.list_approval_rules().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_handle_manage_rules_delete_not_found() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("manage_rules", json!({"action": "delete", "id": 999}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert_eq!(extract_tool_result_text(&result.content), "not found");
    }

    #[tokio::test]
    async fn test_handle_manage_rules_delete_missing_id() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("manage_rules", json!({"action": "delete"}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "delete requires id"
        );
    }

    #[tokio::test]
    async fn test_handle_manage_rules_invalid_action() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("manage_rules", json!({"action": "update"}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        assert_eq!(
            extract_tool_result_text(&result.content),
            "invalid action: update"
        );
    }

    // unknown tool

    #[tokio::test]
    async fn test_handle_unknown_tool() {
        let db = Database::open_in_memory().unwrap();
        let call = make_call("nonexistent_tool", json!({}));
        let result = handle_tool_call(&db, &call).await.unwrap();
        let text = extract_tool_result_text(&result.content);
        assert!(text.contains("unknown tool: nonexistent_tool"));
    }
}
