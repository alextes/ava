use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::page::ScreenshotParams;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;

use super::exec::truncate_output;
use super::{ToolCall, ToolDefinition};
use crate::message::{ContentBlock, ImageSource, MessageContent};

pub const BROWSER_TOOL_NAME: &str = "browser";

const ACTION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TEXT_LEN: usize = 4000;

// --- global browser state ---

struct BrowserState {
    browser: Browser,
    /// handle for the handler task — kept alive so the CDP connection stays open
    _handler_handle: tokio::task::JoinHandle<()>,
}

static BROWSER_STATE: OnceLock<Mutex<BrowserState>> = OnceLock::new();

/// find the system chrome binary. returns None if not found.
fn find_chrome_executable() -> Option<&'static str> {
    let candidates = if cfg!(target_os = "macos") {
        vec!["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]
    } else {
        vec![
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
        ]
    };

    candidates
        .into_iter()
        .find(|&path| std::path::Path::new(path).exists())
}

async fn get_or_init_browser() -> Result<&'static Mutex<BrowserState>, String> {
    if let Some(state) = BROWSER_STATE.get() {
        return Ok(state);
    }

    let chrome_path = find_chrome_executable().ok_or("chrome not found — install google chrome")?;

    let headless = std::env::var("AVA_BROWSER_VISIBLE").is_err();

    let mut builder = BrowserConfig::builder()
        .chrome_executable(chrome_path)
        .window_size(1280, 720);

    if !headless {
        builder = builder.with_head();
    }

    let config = builder
        .build()
        .map_err(|e| format!("browser config error: {e}"))?;

    let (browser, mut handler) = Browser::launch(config)
        .await
        .map_err(|e| format!("failed to launch chrome: {e}"))?;

    let handle = tokio::spawn(async move {
        while let Some(h) = handler.next().await {
            if h.is_err() {
                break;
            }
        }
    });

    let state = BrowserState {
        browser,
        _handler_handle: handle,
    };

    // if another task raced us, just use whichever won
    let _ = BROWSER_STATE.set(Mutex::new(state));
    Ok(BROWSER_STATE.get().unwrap())
}

// --- tool definition ---

pub(super) fn browser_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: BROWSER_TOOL_NAME,
        description: "control a headless chrome browser. actions: navigate, click, type, screenshot, get_text.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["navigate", "click", "type", "screenshot", "get_text"],
                    "description": "the browser action to perform"
                },
                "url": {
                    "type": "string",
                    "description": "URL to navigate to (required for navigate)"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector for the target element (required for click, type; optional for get_text)"
                },
                "text": {
                    "type": "string",
                    "description": "text to type into the element (required for type)"
                }
            },
            "required": ["action"]
        }),
    }
}

// --- input ---

#[derive(Debug, Deserialize)]
struct BrowserInput {
    action: String,
    url: Option<String>,
    selector: Option<String>,
    text: Option<String>,
}

// --- handler ---

pub(super) async fn handle_browser(call: &ToolCall) -> MessageContent {
    let input: BrowserInput = match serde_json::from_value(call.input.clone()) {
        Ok(i) => i,
        Err(err) => return MessageContent::tool_result(&call.id, format!("invalid input: {err}")),
    };

    let result = tokio::time::timeout(ACTION_TIMEOUT, execute_action(&input)).await;

    match result {
        Ok(Ok(content)) => content.into_message_content(&call.id),
        Ok(Err(e)) => MessageContent::tool_result(&call.id, format!("error: {e}")),
        Err(_) => MessageContent::tool_result(&call.id, "action timed out (30s)"),
    }
}

#[derive(Debug)]
enum ActionResult {
    Text(String),
    Screenshot(Vec<u8>),
}

impl ActionResult {
    fn into_message_content(self, tool_use_id: &str) -> MessageContent {
        match self {
            Self::Text(text) => MessageContent::tool_result(tool_use_id, text),
            Self::Screenshot(bytes) => {
                let b64 = BASE64.encode(&bytes);
                let blocks = vec![ContentBlock::Image {
                    source: ImageSource {
                        source_type: "base64".to_string(),
                        media_type: "image/png".to_string(),
                        data: b64,
                    },
                }];
                MessageContent::tool_result_with_blocks(tool_use_id, blocks)
            }
        }
    }
}

async fn execute_action(input: &BrowserInput) -> Result<ActionResult, String> {
    match input.action.as_str() {
        "navigate" => action_navigate(input).await,
        "click" => action_click(input).await,
        "type" => action_type(input).await,
        "screenshot" => action_screenshot().await,
        "get_text" => action_get_text(input).await,
        other => Err(format!("unknown action: {other}")),
    }
}

async fn action_navigate(input: &BrowserInput) -> Result<ActionResult, String> {
    let url = input.url.as_deref().ok_or("navigate requires url")?;

    // basic URL validation
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("url must start with http:// or https://".into());
    }

    let state = get_or_init_browser().await?;
    let guard = state.lock().await;

    let page = guard
        .browser
        .new_page(url)
        .await
        .map_err(|e| format!("navigation failed: {e}"))?;

    let title = page
        .get_title()
        .await
        .map_err(|e| format!("failed to get title: {e}"))?
        .unwrap_or_else(|| "(no title)".to_string());

    // close the page to free resources (single-tab model)
    // we don't close here — keep the page alive for subsequent actions
    // the next navigate will open a new page

    Ok(ActionResult::Text(format!(
        "navigated to {url}\ntitle: {title}"
    )))
}

async fn action_click(input: &BrowserInput) -> Result<ActionResult, String> {
    let selector = input.selector.as_deref().ok_or("click requires selector")?;

    let state = get_or_init_browser().await?;
    let guard = state.lock().await;

    let pages = guard
        .browser
        .pages()
        .await
        .map_err(|e| format!("failed to get pages: {e}"))?;
    let page = pages.last().ok_or("no page open — use navigate first")?;

    page.find_element(selector)
        .await
        .map_err(|e| format!("element not found: {e}"))?
        .click()
        .await
        .map_err(|e| format!("click failed: {e}"))?;

    Ok(ActionResult::Text(format!("clicked: {selector}")))
}

async fn action_type(input: &BrowserInput) -> Result<ActionResult, String> {
    let selector = input.selector.as_deref().ok_or("type requires selector")?;
    let text = input.text.as_deref().ok_or("type requires text")?;

    let state = get_or_init_browser().await?;
    let guard = state.lock().await;

    let pages = guard
        .browser
        .pages()
        .await
        .map_err(|e| format!("failed to get pages: {e}"))?;
    let page = pages.last().ok_or("no page open — use navigate first")?;

    page.find_element(selector)
        .await
        .map_err(|e| format!("element not found: {e}"))?
        .click()
        .await
        .map_err(|e| format!("focus failed: {e}"))?
        .type_str(text)
        .await
        .map_err(|e| format!("type failed: {e}"))?;

    Ok(ActionResult::Text(format!(
        "typed {} chars into: {selector}",
        text.len()
    )))
}

async fn action_screenshot() -> Result<ActionResult, String> {
    let state = get_or_init_browser().await?;
    let guard = state.lock().await;

    let pages = guard
        .browser
        .pages()
        .await
        .map_err(|e| format!("failed to get pages: {e}"))?;
    let page = pages.last().ok_or("no page open — use navigate first")?;

    let bytes = page
        .screenshot(
            ScreenshotParams::builder()
                .format(CaptureScreenshotFormat::Png)
                .full_page(true)
                .build(),
        )
        .await
        .map_err(|e| format!("screenshot failed: {e}"))?;

    Ok(ActionResult::Screenshot(bytes))
}

async fn action_get_text(input: &BrowserInput) -> Result<ActionResult, String> {
    let state = get_or_init_browser().await?;
    let guard = state.lock().await;

    let pages = guard
        .browser
        .pages()
        .await
        .map_err(|e| format!("failed to get pages: {e}"))?;
    let page = pages.last().ok_or("no page open — use navigate first")?;

    let text = if let Some(selector) = input.selector.as_deref() {
        page.find_element(selector)
            .await
            .map_err(|e| format!("element not found: {e}"))?
            .inner_text()
            .await
            .map_err(|e| format!("failed to get text: {e}"))?
            .unwrap_or_default()
    } else {
        // get full page text via JS
        page.evaluate("document.body.innerText")
            .await
            .map_err(|e| format!("failed to get page text: {e}"))?
            .into_value::<String>()
            .map_err(|e| format!("failed to parse page text: {e}"))?
    };

    let truncated = if text.len() > MAX_TEXT_LEN {
        format!(
            "{}\n... (truncated, {} total chars)",
            &text[..MAX_TEXT_LEN],
            text.len()
        )
    } else {
        text
    };

    Ok(ActionResult::Text(truncate_output(&truncated)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_chrome_executable() {
        // just verify it doesn't panic — may or may not find chrome
        let _result = find_chrome_executable();
    }

    #[test]
    fn test_action_result_text() {
        let result = ActionResult::Text("hello".into());
        let content = result.into_message_content("test-id");
        match content {
            MessageContent::ToolResult { tool_use_id, .. } => {
                assert_eq!(tool_use_id, "test-id");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_action_result_screenshot() {
        let result = ActionResult::Screenshot(vec![0x89, 0x50, 0x4e, 0x47]);
        let content = result.into_message_content("test-id");
        match content {
            MessageContent::ToolResult {
                tool_use_id,
                content,
            } => {
                assert_eq!(tool_use_id, "test-id");
                match content {
                    crate::message::ToolResultContent::Blocks(blocks) => {
                        assert_eq!(blocks.len(), 1);
                        match &blocks[0] {
                            ContentBlock::Image { source } => {
                                assert_eq!(source.media_type, "image/png");
                                assert_eq!(source.source_type, "base64");
                                assert!(!source.data.is_empty());
                            }
                            _ => panic!("expected Image block"),
                        }
                    }
                    _ => panic!("expected Blocks content"),
                }
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn test_browser_definition_schema() {
        let def = browser_definition();
        assert_eq!(def.name(), BROWSER_TOOL_NAME);
    }

    #[tokio::test]
    async fn test_navigate_rejects_non_http() {
        let input = BrowserInput {
            action: "navigate".into(),
            url: Some("file:///etc/passwd".into()),
            selector: None,
            text: None,
        };
        let result = action_navigate(&input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("http"));
    }

    #[tokio::test]
    async fn test_navigate_requires_url() {
        let input = BrowserInput {
            action: "navigate".into(),
            url: None,
            selector: None,
            text: None,
        };
        let result = action_navigate(&input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires url"));
    }

    #[tokio::test]
    async fn test_click_requires_selector() {
        let input = BrowserInput {
            action: "click".into(),
            url: None,
            selector: None,
            text: None,
        };
        let result = action_click(&input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires selector"));
    }

    #[tokio::test]
    async fn test_type_requires_selector_and_text() {
        let input = BrowserInput {
            action: "type".into(),
            url: None,
            selector: None,
            text: None,
        };
        let result = action_type(&input).await;
        assert!(result.is_err());

        let input2 = BrowserInput {
            action: "type".into(),
            url: None,
            selector: Some("input".into()),
            text: None,
        };
        let result2 = action_type(&input2).await;
        assert!(result2.is_err());
        assert!(result2.unwrap_err().contains("requires text"));
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let input = BrowserInput {
            action: "dance".into(),
            url: None,
            selector: None,
            text: None,
        };
        let result = execute_action(&input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown action"));
    }
}
