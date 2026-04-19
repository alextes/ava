use serde::Deserialize;
use serde_json::json;

use super::exec::truncate_output;
use super::{ToolCall, ToolDefinition};

pub const WEB_SEARCH_TOOL_NAME: &str = "web_search";
pub const WEB_FETCH_TOOL_NAME: &str = "web_fetch";

const BRAVE_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";
const DEFAULT_MAX_RESULTS: u64 = 5;
const MAX_MAX_RESULTS: u64 = 20;
const JINA_READER_BASE: &str = "https://r.jina.ai/";
const DEFAULT_FETCH_MAX_CHARS: u64 = 4000;
const FETCH_TIMEOUT_SECS: u64 = 30;

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

pub(super) async fn handle_web_search(client: &reqwest::Client, call: &ToolCall) -> String {
    match serde_json::from_value::<WebSearchInput>(call.input.clone()) {
        Ok(input) => web_search(client, &input.query, input.max_results).await,
        Err(err) => format!("invalid input: {err}"),
    }
}

pub(super) async fn handle_web_fetch(client: &reqwest::Client, call: &ToolCall) -> String {
    match serde_json::from_value::<WebFetchInput>(call.input.clone()) {
        Ok(input) => web_fetch(client, &input.url, input.max_chars).await,
        Err(err) => format!("invalid input: {err}"),
    }
}

async fn web_search(client: &reqwest::Client, query: &str, max_results: Option<u64>) -> String {
    let api_key = match std::env::var("BRAVE_SEARCH_API_KEY") {
        Ok(key) if !key.is_empty() => key,
        _ => return "web search unavailable: BRAVE_SEARCH_API_KEY not set".to_string(),
    };

    let count = max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .min(MAX_MAX_RESULTS);

    tracing::info!(query, count, "searching web");

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

async fn web_fetch(client: &reqwest::Client, url: &str, max_chars: Option<u64>) -> String {
    if let Err(reason) = validate_fetch_url(url) {
        return format!("invalid URL: {reason}");
    }

    let max = max_chars.unwrap_or(DEFAULT_FETCH_MAX_CHARS) as usize;
    let jina_url = format!("{JINA_READER_BASE}{url}");

    tracing::info!(url, "fetching web page");

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

pub(super) fn web_search_definition() -> ToolDefinition {
    ToolDefinition::Custom {
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

pub(super) fn web_fetch_definition() -> ToolDefinition {
    ToolDefinition::Custom {
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
    use crate::tool::{ToolCall, requires_approval};
    use serde_json::json;

    #[test]
    fn test_requires_approval_web_search() {
        let call = ToolCall {
            id: "test".into(),
            name: WEB_SEARCH_TOOL_NAME.into(),
            input: json!({"query": "rust lang"}),
        };
        assert!(!requires_approval(&call));
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

    // hold the env lock across .await — acceptable in a single-threaded
    // test where the lock just serializes against other env-touching tests.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn test_web_search_missing_api_key() {
        let _guard = crate::config::ENV_TEST_LOCK.lock().unwrap();

        // ensure the env var is not set for this test
        let _original = std::env::var("BRAVE_SEARCH_API_KEY").ok();
        unsafe {
            std::env::remove_var("BRAVE_SEARCH_API_KEY");
        }
        let result = web_search(&reqwest::Client::new(), "test query", None).await;
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
