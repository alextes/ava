use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::accessibility::{
    AxNode, AxPropertyName, GetFullAxTreeParams, GetFullAxTreeReturns,
};
use chromiumoxide::cdp::browser_protocol::dom::{
    BackendNodeId, ResolveNodeParams, ResolveNodeReturns,
};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnParams;
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
const AX_TREE_DEPTH: i64 = 10;
const MAX_AX_NODES: usize = 500;

// --- global browser state ---

struct BrowserState {
    browser: Browser,
    /// handle for the handler task — kept alive so the CDP connection stays open
    _handler_handle: tokio::task::JoinHandle<()>,
    /// ref map from last snapshot: ref number → backend DOM node ID
    ref_map: HashMap<u32, BackendNodeId>,
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
        ref_map: HashMap::new(),
    };

    // if another task raced us, just use whichever won
    let _ = BROWSER_STATE.set(Mutex::new(state));
    Ok(BROWSER_STATE.get().unwrap())
}

// --- tool definition ---

pub(super) fn browser_definition() -> ToolDefinition {
    ToolDefinition::Custom {
        name: BROWSER_TOOL_NAME,
        description: "control a headless chrome browser. actions: navigate, click, type, screenshot, get_text, snapshot. use snapshot to get the accessibility tree, then click/type by ref number.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["navigate", "click", "type", "screenshot", "get_text", "snapshot"],
                    "description": "the browser action to perform"
                },
                "url": {
                    "type": "string",
                    "description": "URL to navigate to (required for navigate)"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector for the target element (for click, type, get_text)"
                },
                "ref": {
                    "type": "integer",
                    "description": "ref number from snapshot output (alternative to selector for click, type)"
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
    #[serde(rename = "ref")]
    ref_num: Option<u32>,
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
        "snapshot" => action_snapshot().await,
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
    let mut guard = state.lock().await;

    // clear ref map on navigation — refs are invalidated
    guard.ref_map.clear();

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

    Ok(ActionResult::Text(format!(
        "navigated to {url}\ntitle: {title}"
    )))
}

async fn action_click(input: &BrowserInput) -> Result<ActionResult, String> {
    if input.ref_num.is_none() && input.selector.is_none() {
        return Err("click requires selector or ref".into());
    }

    let state = get_or_init_browser().await?;
    let guard = state.lock().await;

    let pages = guard
        .browser
        .pages()
        .await
        .map_err(|e| format!("failed to get pages: {e}"))?;
    let page = pages.last().ok_or("no page open — use navigate first")?;

    if let Some(ref_num) = input.ref_num {
        let backend_id = guard
            .ref_map
            .get(&ref_num)
            .ok_or_else(|| format!("ref {ref_num} not found — run snapshot first"))?;
        let object_id = resolve_backend_node(page, *backend_id).await?;
        click_by_object_id(page, &object_id).await?;
        Ok(ActionResult::Text(format!("clicked: ref {ref_num}")))
    } else if let Some(selector) = input.selector.as_deref() {
        page.find_element(selector)
            .await
            .map_err(|e| format!("element not found: {e}"))?
            .click()
            .await
            .map_err(|e| format!("click failed: {e}"))?;
        Ok(ActionResult::Text(format!("clicked: {selector}")))
    } else {
        unreachable!("validated above")
    }
}

async fn action_type(input: &BrowserInput) -> Result<ActionResult, String> {
    let text = input.text.as_deref().ok_or("type requires text")?;
    if input.ref_num.is_none() && input.selector.is_none() {
        return Err("type requires selector or ref".into());
    }

    let state = get_or_init_browser().await?;
    let guard = state.lock().await;

    let pages = guard
        .browser
        .pages()
        .await
        .map_err(|e| format!("failed to get pages: {e}"))?;
    let page = pages.last().ok_or("no page open — use navigate first")?;

    let target_label = if let Some(ref_num) = input.ref_num {
        let backend_id = guard
            .ref_map
            .get(&ref_num)
            .ok_or_else(|| format!("ref {ref_num} not found — run snapshot first"))?;
        let object_id = resolve_backend_node(page, *backend_id).await?;
        // focus then type via CDP
        click_by_object_id(page, &object_id).await?;
        format!("ref {ref_num}")
    } else if let Some(selector) = input.selector.as_deref() {
        page.find_element(selector)
            .await
            .map_err(|e| format!("element not found: {e}"))?
            .click()
            .await
            .map_err(|e| format!("focus failed: {e}"))?;
        selector.to_string()
    } else {
        unreachable!("validated above")
    };

    // type the text via CDP Input.insertText
    use chromiumoxide::cdp::browser_protocol::input::InsertTextParams;
    let insert_params = InsertTextParams::new(text.to_string());
    let _: chromiumoxide::cdp::browser_protocol::input::InsertTextReturns = page
        .execute(insert_params)
        .await
        .map_err(|e| format!("type failed: {e}"))?
        .result;

    Ok(ActionResult::Text(format!(
        "typed {} chars into: {target_label}",
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

// --- snapshot action ---

async fn action_snapshot() -> Result<ActionResult, String> {
    let state = get_or_init_browser().await?;
    let mut guard = state.lock().await;

    let pages = guard
        .browser
        .pages()
        .await
        .map_err(|e| format!("failed to get pages: {e}"))?;
    let page = pages.last().ok_or("no page open — use navigate first")?;

    let params = GetFullAxTreeParams::builder().depth(AX_TREE_DEPTH).build();
    let result: GetFullAxTreeReturns = page
        .execute(params)
        .await
        .map_err(|e| format!("failed to get accessibility tree: {e}"))?
        .result;

    let nodes = result.nodes;
    let (output, ref_map) = format_ax_tree(&nodes);
    guard.ref_map = ref_map;

    Ok(ActionResult::Text(truncate_output(&output)))
}

// --- accessibility tree formatting ---

/// roles that get a ref number (interactive elements the agent can target)
const INTERACTIVE_ROLES: &[&str] = &[
    "button",
    "link",
    "textbox",
    "combobox",
    "checkbox",
    "radio",
    "slider",
    "switch",
    "menuitem",
    "tab",
    "treeitem",
    "option",
    "searchbox",
    "spinbutton",
];

/// structural roles to collapse (promote children) when they have no name
const STRUCTURAL_ROLES: &[&str] = &["generic", "none", "presentation", "group"];

fn is_interactive(role: &str) -> bool {
    INTERACTIVE_ROLES
        .iter()
        .any(|r| r.eq_ignore_ascii_case(role))
}

fn is_structural(role: &str) -> bool {
    STRUCTURAL_ROLES
        .iter()
        .any(|r| r.eq_ignore_ascii_case(role))
}

/// extract the string value from an AxValue
fn ax_value_str(val: &chromiumoxide::cdp::browser_protocol::accessibility::AxValue) -> String {
    val.value
        .as_ref()
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// format the flat AxNode list into an indented tree with refs.
/// returns (formatted text, ref_map).
fn format_ax_tree(nodes: &[AxNode]) -> (String, HashMap<u32, BackendNodeId>) {
    if nodes.is_empty() {
        return ("(empty accessibility tree)".to_string(), HashMap::new());
    }

    // build a lookup: node_id → index
    let id_to_idx: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.node_id.inner().to_string(), i))
        .collect();

    // build children map: node_id → vec of child indices
    let mut children_map: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        if let Some(parent_id) = &node.parent_id {
            children_map
                .entry(parent_id.inner().to_string())
                .or_default()
                .push(i);
        }
        // also respect child_ids for nodes that declare them
        if let Some(child_ids) = &node.child_ids {
            let entry = children_map
                .entry(node.node_id.inner().to_string())
                .or_default();
            for cid in child_ids {
                if let Some(&idx) = id_to_idx.get(cid.inner())
                    && !entry.contains(&idx)
                {
                    entry.push(idx);
                }
            }
        }
    }

    let mut output = String::new();
    let mut ref_map = HashMap::new();
    let mut next_ref: u32 = 1;
    let mut node_count: usize = 0;

    // find root — first node with no parent or the first node
    let root_idx = nodes
        .iter()
        .position(|n| n.parent_id.is_none())
        .unwrap_or(0);

    format_node_recursive(
        nodes,
        &children_map,
        root_idx,
        0,
        &mut output,
        &mut ref_map,
        &mut next_ref,
        &mut node_count,
    );

    if node_count >= MAX_AX_NODES {
        output.push_str(&format!(
            "\n... (truncated at {} nodes, tree may have more)\n",
            MAX_AX_NODES
        ));
    }

    (output, ref_map)
}

#[allow(clippy::too_many_arguments)]
fn format_node_recursive(
    nodes: &[AxNode],
    children_map: &HashMap<String, Vec<usize>>,
    idx: usize,
    depth: usize,
    output: &mut String,
    ref_map: &mut HashMap<u32, BackendNodeId>,
    next_ref: &mut u32,
    node_count: &mut usize,
) {
    if *node_count >= MAX_AX_NODES {
        return;
    }

    let node = &nodes[idx];

    // skip ignored nodes
    if node.ignored {
        // still recurse into children — some ignored containers have visible children
        if let Some(children) = children_map.get(node.node_id.inner()) {
            for &child_idx in children {
                format_node_recursive(
                    nodes,
                    children_map,
                    child_idx,
                    depth,
                    output,
                    ref_map,
                    next_ref,
                    node_count,
                );
            }
        }
        return;
    }

    let role = node.role.as_ref().map(ax_value_str).unwrap_or_default();
    let name = node.name.as_ref().map(ax_value_str).unwrap_or_default();

    // collapse structural nodes with no name — promote their children
    if is_structural(&role) && name.is_empty() {
        if let Some(children) = children_map.get(node.node_id.inner()) {
            for &child_idx in children {
                format_node_recursive(
                    nodes,
                    children_map,
                    child_idx,
                    depth,
                    output,
                    ref_map,
                    next_ref,
                    node_count,
                );
            }
        }
        return;
    }

    // skip nodes with no role and no name (pure structural noise)
    if role.is_empty() && name.is_empty() {
        return;
    }

    *node_count += 1;

    // build the line
    let indent = "  ".repeat(depth);
    let interactive = is_interactive(&role);

    // assign ref for interactive elements
    let ref_prefix = if interactive {
        if let Some(backend_id) = node.backend_dom_node_id {
            let r = *next_ref;
            ref_map.insert(r, backend_id);
            *next_ref += 1;
            format!("[{r}] ")
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // format: [ref] role "name" [attrs]
    let mut line = format!("{indent}{ref_prefix}{role}");
    if !name.is_empty() {
        line.push_str(&format!(" \"{}\"", truncate_name(&name, 80)));
    }

    // collect relevant properties
    let attrs = format_properties(node);
    if !attrs.is_empty() {
        line.push_str(&format!(" [{attrs}]"));
    }

    // add value for inputs
    if let Some(value) = &node.value {
        let val_str = ax_value_str(value);
        if !val_str.is_empty() {
            line.push_str(&format!(" value=\"{}\"", truncate_name(&val_str, 40)));
        }
    }

    output.push_str(&line);
    output.push('\n');

    // recurse children
    if let Some(children) = children_map.get(node.node_id.inner()) {
        for &child_idx in children {
            format_node_recursive(
                nodes,
                children_map,
                child_idx,
                depth + 1,
                output,
                ref_map,
                next_ref,
                node_count,
            );
        }
    }
}

fn format_properties(node: &AxNode) -> String {
    let Some(props) = &node.properties else {
        return String::new();
    };

    let mut parts = Vec::new();
    for prop in props {
        let val_str = ax_value_str(&prop.value);
        match prop.name {
            AxPropertyName::Focused if val_str == "true" => parts.push("focused".to_string()),
            AxPropertyName::Disabled if val_str == "true" => parts.push("disabled".to_string()),
            AxPropertyName::Checked if val_str != "false" => {
                parts.push(format!("checked={val_str}"))
            }
            AxPropertyName::Pressed if val_str != "false" => {
                parts.push(format!("pressed={val_str}"))
            }
            AxPropertyName::Expanded => parts.push(format!("expanded={val_str}")),
            AxPropertyName::Selected if val_str == "true" => parts.push("selected".to_string()),
            AxPropertyName::Level => parts.push(format!("level={val_str}")),
            _ => {}
        }
    }
    parts.join(", ")
}

fn truncate_name(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

// --- ref-based interaction helpers ---

async fn resolve_backend_node(
    page: &chromiumoxide::Page,
    backend_id: BackendNodeId,
) -> Result<String, String> {
    let params = ResolveNodeParams::builder()
        .backend_node_id(backend_id)
        .build();
    let result: ResolveNodeReturns = page
        .execute(params)
        .await
        .map_err(|e| format!("failed to resolve node: {e}"))?
        .result;
    result
        .object
        .object_id
        .ok_or_else(|| "resolved node has no object ID".to_string())
        .map(|id| id.inner().to_string())
}

async fn click_by_object_id(page: &chromiumoxide::Page, object_id: &str) -> Result<(), String> {
    // use callFunctionOn to scroll into view and click
    let params = CallFunctionOnParams::builder()
        .object_id(object_id.to_string())
        .function_declaration(
            "function() { this.scrollIntoViewIfNeeded(); this.focus(); this.click(); }",
        )
        .build()
        .map_err(|e| format!("failed to build callFunctionOn params: {e}"))?;
    let _: chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnReturns = page
        .execute(params)
        .await
        .map_err(|e| format!("click via ref failed: {e}"))?
        .result;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chromiumoxide::cdp::browser_protocol::accessibility::{AxNodeId, AxValue, AxValueType};

    fn make_ax_value(s: &str) -> AxValue {
        AxValue::builder()
            .r#type(AxValueType::String)
            .value(serde_json::Value::String(s.to_string()))
            .build()
            .unwrap()
    }

    fn make_ax_node(
        node_id: &str,
        role: &str,
        name: &str,
        parent_id: Option<&str>,
        ignored: bool,
        backend_id: Option<i64>,
    ) -> AxNode {
        let mut builder = AxNode::builder()
            .node_id(AxNodeId::new(node_id.to_string()))
            .ignored(ignored)
            .role(make_ax_value(role))
            .name(make_ax_value(name));
        if let Some(pid) = parent_id {
            builder = builder.parent_id(AxNodeId::new(pid.to_string()));
        }
        if let Some(bid) = backend_id {
            builder = builder.backend_dom_node_id(BackendNodeId::new(bid));
        }
        builder.build().unwrap()
    }

    #[test]
    fn test_find_chrome_executable() {
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
            ref_num: None,
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
            ref_num: None,
            text: None,
        };
        let result = action_navigate(&input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires url"));
    }

    #[tokio::test]
    async fn test_click_requires_selector_or_ref() {
        let input = BrowserInput {
            action: "click".into(),
            url: None,
            selector: None,
            ref_num: None,
            text: None,
        };
        let result = action_click(&input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires selector or ref"));
    }

    #[tokio::test]
    async fn test_type_requires_text() {
        let input = BrowserInput {
            action: "type".into(),
            url: None,
            selector: Some("input".into()),
            ref_num: None,
            text: None,
        };
        let result = action_type(&input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires text"));
    }

    #[tokio::test]
    async fn test_type_requires_selector_or_ref() {
        let input = BrowserInput {
            action: "type".into(),
            url: None,
            selector: None,
            ref_num: None,
            text: Some("hello".into()),
        };
        let result = action_type(&input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires selector or ref"));
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let input = BrowserInput {
            action: "dance".into(),
            url: None,
            selector: None,
            ref_num: None,
            text: None,
        };
        let result = execute_action(&input).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown action"));
    }

    // --- accessibility tree formatting tests ---

    #[test]
    fn test_format_empty_tree() {
        let (output, ref_map) = format_ax_tree(&[]);
        assert_eq!(output, "(empty accessibility tree)");
        assert!(ref_map.is_empty());
    }

    #[test]
    fn test_format_simple_tree() {
        let nodes = vec![
            make_ax_node("1", "WebArea", "Test Page", None, false, None),
            make_ax_node("2", "heading", "Welcome", Some("1"), false, None),
            make_ax_node("3", "button", "Submit", Some("1"), false, Some(100)),
        ];
        let (output, ref_map) = format_ax_tree(&nodes);
        assert!(output.contains("WebArea \"Test Page\""));
        assert!(output.contains("heading \"Welcome\""));
        assert!(output.contains("[1] button \"Submit\""));
        assert_eq!(ref_map.len(), 1);
        assert_eq!(ref_map[&1], BackendNodeId::new(100));
    }

    #[test]
    fn test_format_interactive_roles_get_refs() {
        let nodes = vec![
            make_ax_node("1", "WebArea", "Page", None, false, None),
            make_ax_node("2", "link", "Home", Some("1"), false, Some(10)),
            make_ax_node("3", "textbox", "Search", Some("1"), false, Some(11)),
            make_ax_node("4", "heading", "Title", Some("1"), false, Some(12)),
            make_ax_node("5", "checkbox", "Agree", Some("1"), false, Some(13)),
        ];
        let (output, ref_map) = format_ax_tree(&nodes);
        // interactive elements get refs
        assert!(output.contains("[1] link \"Home\""));
        assert!(output.contains("[2] textbox \"Search\""));
        assert!(output.contains("[3] checkbox \"Agree\""));
        // non-interactive heading does NOT get a ref
        assert!(output.contains("heading \"Title\""));
        assert!(!output.contains("] heading"));
        assert_eq!(ref_map.len(), 3);
    }

    #[test]
    fn test_format_skips_ignored_nodes() {
        let nodes = vec![
            make_ax_node("1", "WebArea", "Page", None, false, None),
            make_ax_node("2", "generic", "", Some("1"), true, None),
            make_ax_node("3", "button", "OK", Some("2"), false, Some(10)),
        ];
        let (output, ref_map) = format_ax_tree(&nodes);
        // ignored node is skipped but its child "OK" button is promoted
        assert!(!output.contains("generic"));
        assert!(output.contains("[1] button \"OK\""));
        assert_eq!(ref_map.len(), 1);
    }

    #[test]
    fn test_format_collapses_structural_nodes() {
        let nodes = vec![
            make_ax_node("1", "WebArea", "Page", None, false, None),
            make_ax_node("2", "generic", "", Some("1"), false, None),
            make_ax_node("3", "button", "Save", Some("2"), false, Some(10)),
        ];
        let (output, ref_map) = format_ax_tree(&nodes);
        // unnamed generic node is collapsed, button is promoted
        assert!(!output.contains("generic"));
        assert!(output.contains("[1] button \"Save\""));
        assert_eq!(ref_map.len(), 1);
    }

    #[test]
    fn test_format_keeps_named_structural_nodes() {
        let nodes = vec![
            make_ax_node("1", "WebArea", "Page", None, false, None),
            make_ax_node("2", "group", "Settings", Some("1"), false, None),
            make_ax_node("3", "button", "Save", Some("2"), false, Some(10)),
        ];
        let (output, _) = format_ax_tree(&nodes);
        // named group is kept
        assert!(output.contains("group \"Settings\""));
        assert!(output.contains("[1] button \"Save\""));
    }

    #[test]
    fn test_format_indentation() {
        let nodes = vec![
            make_ax_node("1", "WebArea", "Page", None, false, None),
            make_ax_node("2", "navigation", "Nav", Some("1"), false, None),
            make_ax_node("3", "link", "Home", Some("2"), false, Some(10)),
        ];
        let (output, _) = format_ax_tree(&nodes);
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[0].starts_with("WebArea"));
        assert!(lines[1].starts_with("  navigation"));
        assert!(lines[2].starts_with("    [1] link"));
    }

    #[test]
    fn test_format_truncates_long_names() {
        let long_name = "a".repeat(200);
        let nodes = vec![make_ax_node("1", "heading", &long_name, None, false, None)];
        let (output, _) = format_ax_tree(&nodes);
        assert!(output.contains("..."));
        assert!(output.len() < 200);
    }

    #[test]
    fn test_is_interactive() {
        assert!(is_interactive("button"));
        assert!(is_interactive("link"));
        assert!(is_interactive("textbox"));
        assert!(is_interactive("Button")); // case insensitive
        assert!(!is_interactive("heading"));
        assert!(!is_interactive("paragraph"));
        assert!(!is_interactive("generic"));
    }

    #[test]
    fn test_is_structural() {
        assert!(is_structural("generic"));
        assert!(is_structural("none"));
        assert!(is_structural("presentation"));
        assert!(is_structural("group"));
        assert!(!is_structural("button"));
        assert!(!is_structural("heading"));
    }

    #[test]
    fn test_truncate_name() {
        assert_eq!(truncate_name("short", 10), "short");
        assert_eq!(truncate_name("a very long name", 5), "a ver...");
    }
}
