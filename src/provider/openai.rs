use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Error;
use crate::message::{ContentBlock, Message, MessageContent, Role, ToolResultContent};
use crate::provider::{Provider, ProviderResponse, ReasoningEffort, StopReason, ToolCall, Usage};
use crate::tool::{APPLY_PATCH_TOOL_NAME, BuiltInKind, ToolDefinition};

const API_URL: &str = "https://api.openai.com/v1/responses";
const DEFAULT_MODEL: &str = "gpt-5.6-luna";
pub const ALLOWED_MODELS: &[&str] = &["gpt-5.6-luna", "gpt-5.6-sol"];
const DEFAULT_MAX_TOKENS: u32 = 8192;

pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    model: String,
    max_tokens: u32,
    reasoning_effort: ReasoningEffort,
}

impl OpenAiProvider {
    pub fn new(client: Client, api_key: String) -> Self {
        Self {
            client,
            api_key,
            model: DEFAULT_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            reasoning_effort: ReasoningEffort::Medium,
        }
    }

    pub fn from_env(client: Client) -> Result<Self, Error> {
        let api_key =
            std::env::var("OPENAI_API_KEY").map_err(|_| Error::MissingApiKey("OPENAI_API_KEY"))?;
        Ok(Self::new(client, api_key))
    }

    pub fn model_name(&self) -> &str {
        &self.model
    }

    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    pub fn reasoning_effort(&self) -> ReasoningEffort {
        self.reasoning_effort
    }

    pub fn set_reasoning_effort(&mut self, effort: ReasoningEffort) {
        self.reasoning_effort = effort;
    }

    pub fn context_window(&self) -> u32 {
        match self.model.as_str() {
            "gpt-5.6-luna" | "gpt-5.6-sol" => 1_050_000,
            _ => 400_000,
        }
    }
}

// -- request types --

#[derive(Debug, Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    max_output_tokens: u32,
    instructions: &'a str,
    input: Vec<InputItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    prompt_cache_retention: &'a str,
    reasoning: OpenAiReasoning,
}

#[derive(Debug, Serialize)]
struct OpenAiReasoning {
    effort: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum InputItem {
    #[serde(rename = "message")]
    Message { role: String, content: Value },
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput { call_id: String, output: Value },
    #[serde(rename = "apply_patch_call")]
    ApplyPatchCall {
        id: String,
        call_id: String,
        status: String,
        operation: Value,
    },
    #[serde(rename = "apply_patch_call_output")]
    ApplyPatchCallOutput {
        call_id: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
}

// -- response types --

#[derive(Debug, Deserialize)]
struct ApiResponse {
    output: Vec<OutputItem>,
    #[serde(default)]
    usage: Option<ApiUsage>,
    status: String,
}

#[derive(Debug, Deserialize)]
struct ApiUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    input_tokens_details: Option<InputTokensDetails>,
    #[serde(default)]
    output_tokens_details: Option<OutputTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct InputTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OutputTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum OutputItem {
    #[serde(rename = "message")]
    Message { content: Vec<OutputContent> },
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "apply_patch_call")]
    ApplyPatchCall {
        id: String,
        call_id: String,
        status: String,
        operation: ApplyPatchOperation,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct ApplyPatchOperation {
    #[serde(rename = "type")]
    op_type: String,
    path: String,
    #[serde(default)]
    diff: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum OutputContent {
    #[serde(rename = "output_text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    error: ApiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    message: String,
    #[serde(rename = "type")]
    error_type: Option<String>,
}

// -- conversion helpers --

fn convert_messages(messages: &[Message]) -> Vec<InputItem> {
    let mut out = Vec::new();
    // track which call IDs are for apply_patch so we can emit the right output type
    let mut apply_patch_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    for msg in messages {
        match msg.role {
            Role::User | Role::System => {
                let mut content_parts: Vec<Value> = Vec::new();
                for block in &msg.content {
                    match block {
                        MessageContent::Text { text } => {
                            content_parts
                                .push(serde_json::json!({"type": "input_text", "text": text}));
                        }
                        MessageContent::Image { source } => {
                            let data_uri =
                                format!("data:{};base64,{}", source.media_type, source.data);
                            content_parts.push(serde_json::json!({
                                "type": "input_image",
                                "image_url": data_uri,
                            }));
                        }
                        MessageContent::ToolResult {
                            tool_use_id,
                            content,
                        } => {
                            // flush any accumulated content parts first
                            if !content_parts.is_empty() {
                                out.push(InputItem::Message {
                                    role: "user".to_string(),
                                    content: Value::Array(std::mem::take(&mut content_parts)),
                                });
                            }
                            if apply_patch_ids.contains(tool_use_id) {
                                let output_text = match content {
                                    ToolResultContent::Text(s) => s.clone(),
                                    ToolResultContent::Blocks(blocks) => blocks
                                        .iter()
                                        .filter_map(|b| match b {
                                            ContentBlock::Text { text } => Some(text.as_str()),
                                            _ => None,
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                };
                                let succeeded = output_text == "ok";
                                out.push(InputItem::ApplyPatchCallOutput {
                                    call_id: tool_use_id.clone(),
                                    status: if succeeded {
                                        "completed".to_string()
                                    } else {
                                        "failed".to_string()
                                    },
                                    output: if succeeded { None } else { Some(output_text) },
                                });
                            } else {
                                let output = tool_result_content_to_value(content);
                                out.push(InputItem::FunctionCallOutput {
                                    call_id: tool_use_id.clone(),
                                    output,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                if !content_parts.is_empty() {
                    // optimize: single text block can be sent as plain string
                    let content = if content_parts.len() == 1
                        && content_parts[0].get("type") == Some(&Value::String("input_text".into()))
                    {
                        content_parts[0]["text"].clone()
                    } else {
                        Value::Array(content_parts)
                    };
                    out.push(InputItem::Message {
                        role: "user".to_string(),
                        content,
                    });
                }
            }
            Role::Assistant => {
                let mut text_parts = Vec::new();

                for block in &msg.content {
                    match block {
                        MessageContent::Text { text } => {
                            text_parts.push(text.clone());
                        }
                        MessageContent::ToolUse { id, name, input } => {
                            // flush any accumulated text first
                            if !text_parts.is_empty() {
                                out.push(InputItem::Message {
                                    role: "assistant".to_string(),
                                    content: Value::String(text_parts.join("\n")),
                                });
                                text_parts.clear();
                            }
                            if name == APPLY_PATCH_TOOL_NAME {
                                // re-send as apply_patch_call (not function_call) so the
                                // API has context for the corresponding apply_patch_call_output.
                                // we reconstruct from the stored input which includes apc_id
                                // and apc_status from the original response.
                                apply_patch_ids.insert(id.clone());
                                let apc_id = input
                                    .get("apc_id")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| format!("apc_{id}"));
                                let apc_status = input
                                    .get("apc_status")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("completed")
                                    .to_string();
                                let mut operation = serde_json::json!({
                                    "type": input.get("operation").and_then(|v| v.as_str()).unwrap_or("update_file"),
                                    "path": input.get("path").and_then(|v| v.as_str()).unwrap_or(""),
                                });
                                // only include diff when present — delete_file has no diff
                                // and the API rejects unknown fields
                                if let Some(diff) = input.get("diff").and_then(|v| v.as_str()) {
                                    operation["diff"] = serde_json::Value::String(diff.to_string());
                                }
                                out.push(InputItem::ApplyPatchCall {
                                    id: apc_id,
                                    call_id: id.clone(),
                                    status: apc_status,
                                    operation,
                                });
                            } else {
                                out.push(InputItem::FunctionCall {
                                    call_id: id.clone(),
                                    name: name.clone(),
                                    arguments: serde_json::to_string(input).unwrap_or_default(),
                                });
                            }
                        }
                        _ => {}
                    }
                }

                if !text_parts.is_empty() {
                    out.push(InputItem::Message {
                        role: "assistant".to_string(),
                        content: Value::String(text_parts.join("\n")),
                    });
                }
            }
        }
    }

    out
}

fn tool_result_content_to_value(content: &ToolResultContent) -> Value {
    match content {
        ToolResultContent::Text(s) => Value::String(s.clone()),
        ToolResultContent::Blocks(blocks) => {
            let parts: Vec<Value> = blocks
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => {
                        serde_json::json!({"type": "input_text", "text": text})
                    }
                    ContentBlock::Image { source } => {
                        let data_uri = format!("data:{};base64,{}", source.media_type, source.data);
                        serde_json::json!({
                            "type": "input_image",
                            "image_url": data_uri
                        })
                    }
                })
                .collect();
            Value::Array(parts)
        }
    }
}

fn convert_tools(definitions: &[ToolDefinition]) -> Vec<Value> {
    definitions
        .iter()
        .filter_map(|def| match def {
            ToolDefinition::Custom {
                name,
                description,
                input_schema,
            } => Some(serde_json::json!({
                "type": "function",
                "name": *name,
                "description": *description,
                "parameters": input_schema,
            })),
            ToolDefinition::Dynamic {
                name,
                description,
                input_schema,
            } => Some(serde_json::json!({
                "type": "function",
                "name": name,
                "description": description,
                "parameters": input_schema,
            })),
            ToolDefinition::BuiltIn { kind } => match kind {
                BuiltInKind::OpenAiApplyPatch => Some(serde_json::json!({
                    "type": kind.api_type(),
                })),
                // skip non-openai built-ins
                _ => None,
            },
        })
        .collect()
}

fn parse_status(status: &str) -> StopReason {
    match status {
        "completed" => StopReason::EndTurn,
        "incomplete" => StopReason::MaxTokens,
        _ => StopReason::EndTurn,
    }
}

impl Provider for OpenAiProvider {
    #[tracing::instrument(skip_all, fields(model = %self.model))]
    async fn complete(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ProviderResponse, Error> {
        let input = convert_messages(messages);
        let tools = convert_tools(tools);

        let request = ApiRequest {
            model: &self.model,
            max_output_tokens: self.max_tokens,
            instructions: system_prompt,
            input,
            tools,
            prompt_cache_retention: "24h",
            reasoning: OpenAiReasoning {
                effort: self.reasoning_effort.as_str(),
            },
        };

        let response = self
            .client
            .post(API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error: ApiError = response.json().await?;
            let msg = &error.error.message;
            if msg.contains("maximum context length") || msg.contains("too many tokens") {
                return Err(Error::ContextOverflow);
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let is_quota = error.error.error_type.as_deref() == Some("insufficient_quota");
                return if is_quota {
                    Err(Error::BudgetExhausted(error.error.message))
                } else {
                    Err(Error::RateLimited(error.error.message))
                };
            }
            if status == reqwest::StatusCode::PAYMENT_REQUIRED {
                return Err(Error::BudgetExhausted(error.error.message));
            }
            return Err(Error::Provider(error.error.message));
        }

        let api_response: ApiResponse = response.json().await?;

        let mut content_parts = Vec::new();
        let mut tool_calls = Vec::new();

        for item in api_response.output {
            match item {
                OutputItem::Message {
                    content: contents, ..
                } => {
                    for c in contents {
                        if let OutputContent::Text { text } = c {
                            content_parts.push(text);
                        }
                    }
                }
                OutputItem::FunctionCall {
                    call_id,
                    name,
                    arguments,
                } => {
                    let input: Value = serde_json::from_str(&arguments)
                        .unwrap_or(Value::Object(serde_json::Map::new()));
                    tool_calls.push(ToolCall {
                        id: call_id,
                        name,
                        input,
                    });
                }
                OutputItem::ApplyPatchCall {
                    id,
                    call_id,
                    status,
                    operation,
                } => {
                    let mut input = serde_json::json!({
                        "apc_id": id,
                        "apc_status": status,
                        "operation": operation.op_type,
                        "path": operation.path,
                    });
                    if let Some(diff) = operation.diff {
                        input["diff"] = serde_json::Value::String(diff);
                    }
                    tool_calls.push(ToolCall {
                        id: call_id,
                        name: APPLY_PATCH_TOOL_NAME.to_string(),
                        input,
                    });
                }
                OutputItem::Other => {}
            }
        }

        let stop_reason = if !tool_calls.is_empty() {
            StopReason::ToolUse
        } else {
            parse_status(&api_response.status)
        };

        let usage = api_response
            .usage
            .map(|u| {
                let cached_tokens = u.input_tokens_details.and_then(|d| d.cached_tokens);
                let reasoning_tokens = u.output_tokens_details.and_then(|d| d.reasoning_tokens);
                Usage {
                    input_tokens: u.input_tokens,
                    output_tokens: u.output_tokens,
                    cache_read_tokens: cached_tokens,
                    reasoning_tokens,
                    ..Default::default()
                }
            })
            .unwrap_or_default();

        Ok(ProviderResponse {
            content: content_parts.join(""),
            stop_reason,
            tool_calls,
            hidden_content: Vec::new(),
            usage,
        })
    }

    fn cache_ttl(&self) -> Duration {
        // `prompt_cache_retention: "24h"` — hint to openai to keep the cache
        // warm for up to 24 hours.
        Duration::from_secs(24 * 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> Client {
        Client::builder()
            .no_proxy()
            .build()
            .expect("test HTTP client")
    }

    #[test]
    fn test_allowed_models_use_luna_default_and_include_sol() {
        let provider = OpenAiProvider::new(test_client(), "test-key".into());

        assert_eq!(provider.model_name(), "gpt-5.6-luna");
        assert_eq!(ALLOWED_MODELS, &["gpt-5.6-luna", "gpt-5.6-sol"]);
    }

    #[test]
    fn test_gpt_5_6_context_windows() {
        let mut provider = OpenAiProvider::new(test_client(), "test-key".into());
        assert_eq!(provider.context_window(), 1_050_000);

        provider.set_model("gpt-5.6-sol".into());
        assert_eq!(provider.context_window(), 1_050_000);
    }

    #[test]
    fn test_convert_user_message() {
        let messages = vec![Message::user("hello")];
        let result = convert_messages(&messages);

        assert_eq!(result.len(), 1);
        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["type"], "message");
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "hello");
    }

    #[test]
    fn test_convert_assistant_with_tool_use() {
        let messages = vec![Message::assistant_with_content(vec![
            MessageContent::text("thinking..."),
            MessageContent::tool_use("call_123", "remember_fact", serde_json::json!({"key": "v"})),
        ])];

        let result = convert_messages(&messages);
        assert_eq!(result.len(), 2); // assistant message + function_call

        let msg_json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(msg_json["type"], "message");
        assert_eq!(msg_json["role"], "assistant");
        assert_eq!(msg_json["content"], "thinking...");

        let call_json = serde_json::to_value(&result[1]).unwrap();
        assert_eq!(call_json["type"], "function_call");
        assert_eq!(call_json["call_id"], "call_123");
        assert_eq!(call_json["name"], "remember_fact");
    }

    #[test]
    fn test_convert_tool_result_message() {
        let messages = vec![Message::user_with_content(vec![
            MessageContent::tool_result("call_123", "ok"),
        ])];

        let result = convert_messages(&messages);
        assert_eq!(result.len(), 1);

        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["type"], "function_call_output");
        assert_eq!(json["call_id"], "call_123");
        assert_eq!(json["output"], "ok");
    }

    #[test]
    fn test_convert_tool_result_with_image() {
        use crate::message::ImageSource;

        let blocks = vec![
            ContentBlock::Text {
                text: "screenshot".into(),
            },
            ContentBlock::Image {
                source: ImageSource {
                    source_type: "base64".into(),
                    media_type: "image/png".into(),
                    data: "iVBORw0KGgo=".into(),
                },
            },
        ];
        let messages = vec![Message::user_with_content(vec![
            MessageContent::tool_result_with_blocks("call_456", blocks),
        ])];

        let result = convert_messages(&messages);
        assert_eq!(result.len(), 1);

        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["type"], "function_call_output");
        assert_eq!(json["call_id"], "call_456");

        let output = &json["output"];
        assert!(output.is_array());
        let arr = output.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "input_text");
        assert_eq!(arr[0]["text"], "screenshot");
        assert_eq!(arr[1]["type"], "input_image");
        assert_eq!(arr[1]["image_url"], "data:image/png;base64,iVBORw0KGgo=");
    }

    #[test]
    fn test_convert_tools() {
        let definitions = crate::tool::tool_definitions(false);
        let custom_count = definitions
            .iter()
            .filter(|d| matches!(d, ToolDefinition::Custom { .. }))
            .count();
        let tools = convert_tools(&definitions);

        // custom tools + apply_patch built-in (anthropic text_editor is filtered out)
        assert_eq!(tools.len(), custom_count + 1);
        assert!(tools.len() < definitions.len());

        // first tool should be a function tool
        assert_eq!(tools[0]["type"], "function");
        assert!(tools[0]["name"].is_string());
        assert!(tools[0]["parameters"].is_object());

        // verify apply_patch is included as a native built-in
        let has_apply_patch = tools.iter().any(|t| t["type"] == "apply_patch");
        assert!(has_apply_patch, "apply_patch should be in the tool list");

        // verify no tool is named after the anthropic text editor
        for tool in &tools {
            if let Some(name) = tool.get("name").and_then(|n| n.as_str()) {
                assert_ne!(name, "str_replace_based_edit_tool");
            }
        }
    }

    #[test]
    fn test_request_serializes_reasoning_effort() {
        let request = ApiRequest {
            model: "gpt-5.6-luna",
            max_output_tokens: 1024,
            instructions: "be helpful",
            input: vec![],
            tools: vec![],
            prompt_cache_retention: "24h",
            reasoning: OpenAiReasoning { effort: "medium" },
        };

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["reasoning"]["effort"], "medium");
    }

    #[test]
    fn test_parse_response() {
        let json = r#"{
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": "hello there"
                }]
            }],
            "status": "completed"
        }"#;

        let response: ApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.output.len(), 1);
        if let OutputItem::Message { content, .. } = &response.output[0] {
            assert_eq!(content.len(), 1);
            if let OutputContent::Text { text } = &content[0] {
                assert_eq!(text, "hello there");
            } else {
                panic!("expected Text content");
            }
        } else {
            panic!("expected Message output");
        }
        assert_eq!(response.status, "completed");
    }

    #[test]
    fn test_parse_tool_call_response() {
        let json = r#"{
            "output": [{
                "type": "function_call",
                "call_id": "call_abc",
                "name": "remember_fact",
                "arguments": "{\"category\":\"user\",\"key\":\"name\",\"value\":\"alex\"}"
            }],
            "status": "completed"
        }"#;

        let response: ApiResponse = serde_json::from_str(json).unwrap();
        if let OutputItem::FunctionCall {
            call_id,
            name,
            arguments,
        } = &response.output[0]
        {
            assert_eq!(call_id, "call_abc");
            assert_eq!(name, "remember_fact");
            let args: Value = serde_json::from_str(arguments).unwrap();
            assert_eq!(args["category"], "user");
        } else {
            panic!("expected FunctionCall output");
        }
    }

    #[test]
    fn test_parse_status() {
        assert_eq!(parse_status("completed"), StopReason::EndTurn);
        assert_eq!(parse_status("incomplete"), StopReason::MaxTokens);
        assert_eq!(parse_status("unknown"), StopReason::EndTurn);
    }

    #[test]
    fn test_parse_apply_patch_call_response() {
        let json = r#"{
            "output": [{
                "type": "apply_patch_call",
                "id": "apc_123",
                "call_id": "call_xyz",
                "status": "completed",
                "operation": {
                    "type": "update_file",
                    "path": "src/main.rs",
                    "diff": "*** Begin Patch\n*** Update File: src/main.rs\n@@\n context\n-old\n+new\n*** End Patch"
                }
            }],
            "status": "completed"
        }"#;

        let response: ApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.output.len(), 1);
        if let OutputItem::ApplyPatchCall {
            id,
            call_id,
            status,
            operation,
        } = &response.output[0]
        {
            assert_eq!(id, "apc_123");
            assert_eq!(call_id, "call_xyz");
            assert_eq!(status, "completed");
            assert_eq!(operation.op_type, "update_file");
            assert_eq!(operation.path, "src/main.rs");
            assert!(operation.diff.as_ref().unwrap().contains("Begin Patch"));
        } else {
            panic!("expected ApplyPatchCall output");
        }
    }

    #[test]
    fn test_apply_patch_message_roundtrip() {
        // simulate: assistant emits apply_patch tool use, user returns tool result.
        // input includes apc_id/apc_status from the original API response.
        let messages = vec![
            Message::assistant_with_content(vec![MessageContent::tool_use(
                "call_xyz",
                APPLY_PATCH_TOOL_NAME,
                serde_json::json!({
                    "apc_id": "apc_123",
                    "apc_status": "completed",
                    "operation": "update_file",
                    "path": "src/main.rs"
                }),
            )]),
            Message::user_with_content(vec![MessageContent::tool_result("call_xyz", "ok")]),
        ];

        let result = convert_messages(&messages);

        // apply_patch tool use should NOT appear as a function_call
        assert!(
            !result
                .iter()
                .any(|item| matches!(item, InputItem::FunctionCall { .. })),
            "apply_patch should not be serialized as function_call"
        );

        // apply_patch tool use should appear as an apply_patch_call input item
        let call_item = result
            .iter()
            .find(|item| matches!(item, InputItem::ApplyPatchCall { .. }))
            .expect("should have ApplyPatchCall input item");
        let call_json = serde_json::to_value(call_item).unwrap();
        assert_eq!(call_json["type"], "apply_patch_call");
        assert_eq!(call_json["id"], "apc_123");
        assert_eq!(call_json["call_id"], "call_xyz");
        assert_eq!(call_json["status"], "completed");
        assert_eq!(call_json["operation"]["type"], "update_file");
        assert_eq!(call_json["operation"]["path"], "src/main.rs");

        // the tool result should be an ApplyPatchCallOutput
        let output_item = result
            .iter()
            .find(|item| matches!(item, InputItem::ApplyPatchCallOutput { .. }))
            .expect("should have ApplyPatchCallOutput");
        let json = serde_json::to_value(output_item).unwrap();
        assert_eq!(json["type"], "apply_patch_call_output");
        assert_eq!(json["call_id"], "call_xyz");
        assert_eq!(json["status"], "completed");
        assert!(json.get("output").is_none() || json["output"].is_null());
    }

    #[test]
    fn test_apply_patch_failed_result() {
        let messages = vec![
            Message::assistant_with_content(vec![MessageContent::tool_use(
                "call_fail",
                APPLY_PATCH_TOOL_NAME,
                serde_json::json!({"apc_id": "apc_f", "apc_status": "completed", "operation": "update_file", "path": "x.rs"}),
            )]),
            Message::user_with_content(vec![MessageContent::tool_result(
                "call_fail",
                "could not find matching location",
            )]),
        ];

        let result = convert_messages(&messages);
        let output_item = result
            .iter()
            .find(|item| matches!(item, InputItem::ApplyPatchCallOutput { .. }))
            .expect("should have ApplyPatchCallOutput");
        let json = serde_json::to_value(output_item).unwrap();
        assert_eq!(json["status"], "failed");
        assert_eq!(json["output"], "could not find matching location");
    }

    #[test]
    fn test_apply_patch_replay_without_apc_fields() {
        // regression test: ToolUse input stored before the apc_id/apc_status fix
        // should still produce valid apply_patch_call items with defaults
        let messages = vec![
            Message::assistant_with_content(vec![MessageContent::tool_use(
                "call_old",
                APPLY_PATCH_TOOL_NAME,
                // no apc_id or apc_status — old format
                serde_json::json!({"operation": "update_file", "path": "old.rs", "diff": "@@\n-a\n+b"}),
            )]),
            Message::user_with_content(vec![MessageContent::tool_result("call_old", "ok")]),
        ];

        let result = convert_messages(&messages);

        let call_item = result
            .iter()
            .find(|item| matches!(item, InputItem::ApplyPatchCall { .. }))
            .expect("should have ApplyPatchCall");
        let json = serde_json::to_value(call_item).unwrap();
        // id falls back to "apc_" + call_id (API requires apc_ prefix), status to "completed"
        assert_eq!(json["id"], "apc_call_old");
        assert_eq!(json["status"], "completed");
        assert_eq!(json["call_id"], "call_old");
        assert_eq!(json["operation"]["type"], "update_file");
    }

    #[test]
    fn test_parse_mixed_response_function_and_apply_patch() {
        let json = r#"{
            "output": [
                {
                    "type": "message",
                    "content": [{"type": "output_text", "text": "making changes"}]
                },
                {
                    "type": "function_call",
                    "call_id": "call_fn1",
                    "name": "exec",
                    "arguments": "{\"command\":\"ls\"}"
                },
                {
                    "type": "apply_patch_call",
                    "id": "apc_1",
                    "call_id": "call_ap1",
                    "status": "completed",
                    "operation": {
                        "type": "create_file",
                        "path": "new.txt",
                        "diff": "+hello"
                    }
                }
            ],
            "status": "completed"
        }"#;

        let response: ApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.output.len(), 3);

        // check each item type was parsed correctly
        assert!(matches!(response.output[0], OutputItem::Message { .. }));
        assert!(matches!(
            response.output[1],
            OutputItem::FunctionCall { .. }
        ));
        assert!(matches!(
            response.output[2],
            OutputItem::ApplyPatchCall { .. }
        ));
    }

    #[test]
    fn test_parse_multiple_apply_patch_calls() {
        let json = r#"{
            "output": [
                {
                    "type": "apply_patch_call",
                    "id": "apc_1",
                    "call_id": "call_1",
                    "status": "completed",
                    "operation": {"type": "update_file", "path": "a.rs", "diff": "@@\n-old\n+new"}
                },
                {
                    "type": "apply_patch_call",
                    "id": "apc_2",
                    "call_id": "call_2",
                    "status": "completed",
                    "operation": {"type": "create_file", "path": "b.rs", "diff": "+content"}
                },
                {
                    "type": "apply_patch_call",
                    "id": "apc_3",
                    "call_id": "call_3",
                    "status": "completed",
                    "operation": {"type": "delete_file", "path": "c.rs"}
                }
            ],
            "status": "completed"
        }"#;

        let response: ApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.output.len(), 3);

        // all three should be ApplyPatchCall with correct operations
        if let OutputItem::ApplyPatchCall { operation, .. } = &response.output[0] {
            assert_eq!(operation.op_type, "update_file");
        } else {
            panic!("expected ApplyPatchCall");
        }
        if let OutputItem::ApplyPatchCall { operation, .. } = &response.output[1] {
            assert_eq!(operation.op_type, "create_file");
        } else {
            panic!("expected ApplyPatchCall");
        }
        if let OutputItem::ApplyPatchCall { operation, .. } = &response.output[2] {
            assert_eq!(operation.op_type, "delete_file");
            assert!(operation.diff.is_none());
        } else {
            panic!("expected ApplyPatchCall");
        }
    }

    #[test]
    fn test_multiple_apply_patch_results_roundtrip() {
        // two apply_patch calls followed by their results
        let messages = vec![
            Message::assistant_with_content(vec![
                MessageContent::text("editing two files"),
                MessageContent::tool_use(
                    "call_1",
                    APPLY_PATCH_TOOL_NAME,
                    serde_json::json!({"apc_id": "apc_1", "apc_status": "completed", "operation": "update_file", "path": "a.rs", "diff": "@@\n-old\n+new"}),
                ),
                MessageContent::tool_use(
                    "call_2",
                    APPLY_PATCH_TOOL_NAME,
                    serde_json::json!({"apc_id": "apc_2", "apc_status": "completed", "operation": "create_file", "path": "b.rs", "diff": "+content"}),
                ),
            ]),
            Message::user_with_content(vec![
                MessageContent::tool_result("call_1", "ok"),
                MessageContent::tool_result("call_2", "ok"),
            ]),
        ];

        let result = convert_messages(&messages);

        // should have: assistant text, apply_patch_call x2, apply_patch_call_output x2
        let call_count = result
            .iter()
            .filter(|i| matches!(i, InputItem::ApplyPatchCall { .. }))
            .count();
        let output_count = result
            .iter()
            .filter(|i| matches!(i, InputItem::ApplyPatchCallOutput { .. }))
            .count();
        assert_eq!(call_count, 2, "should have 2 apply_patch_call items");
        assert_eq!(
            output_count, 2,
            "should have 2 apply_patch_call_output items"
        );

        // verify no function_call items leaked in
        let fn_count = result
            .iter()
            .filter(|i| matches!(i, InputItem::FunctionCall { .. }))
            .count();
        assert_eq!(fn_count, 0, "apply_patch should not produce function_call");
    }

    /// end-to-end wire format test: simulate a realistic API response containing
    /// all three apply_patch operation types, run them through the full pipeline
    /// (parse response → build ToolCalls → simulate results → convert to input
    /// items → serialize to JSON), then validate the serialized JSON matches what
    /// the OpenAI API actually accepts.
    ///
    /// this catches wire-format issues like missing required fields, wrong field
    /// names, unexpected null values, and id prefix constraints that unit tests
    /// on internal types miss.
    #[test]
    fn test_apply_patch_wire_format_full_roundtrip() {
        // step 1: parse a realistic API response with all three operation types
        let api_json = r#"{
            "output": [
                {
                    "type": "message",
                    "content": [{"type": "output_text", "text": "making changes"}]
                },
                {
                    "type": "apply_patch_call",
                    "id": "apc_create_abc",
                    "call_id": "call_create_1",
                    "status": "completed",
                    "operation": {
                        "type": "create_file",
                        "path": "new.txt",
                        "diff": "+line one\n+line two\n"
                    }
                },
                {
                    "type": "apply_patch_call",
                    "id": "apc_update_def",
                    "call_id": "call_update_2",
                    "status": "completed",
                    "operation": {
                        "type": "update_file",
                        "path": "existing.rs",
                        "diff": "@@\n context\n-old line\n+new line\n"
                    }
                },
                {
                    "type": "apply_patch_call",
                    "id": "apc_delete_ghi",
                    "call_id": "call_delete_3",
                    "status": "completed",
                    "operation": {
                        "type": "delete_file",
                        "path": "obsolete.txt"
                    }
                }
            ],
            "status": "completed"
        }"#;

        let api_response: ApiResponse = serde_json::from_str(api_json).unwrap();

        // step 2: extract ToolCalls (same as Provider::complete does)
        let mut tool_calls = Vec::new();
        for item in api_response.output {
            if let OutputItem::ApplyPatchCall {
                id,
                call_id,
                status,
                operation,
            } = item
            {
                let mut input = serde_json::json!({
                    "apc_id": id,
                    "apc_status": status,
                    "operation": operation.op_type,
                    "path": operation.path,
                });
                if let Some(diff) = operation.diff {
                    input["diff"] = serde_json::Value::String(diff);
                }
                tool_calls.push(ToolCall {
                    id: call_id,
                    name: APPLY_PATCH_TOOL_NAME.to_string(),
                    input,
                });
            }
        }
        assert_eq!(tool_calls.len(), 3);

        // step 3: simulate tool results and build messages (same as agent loop)
        let mut assistant_content: Vec<MessageContent> =
            vec![MessageContent::text("making changes")];
        for tc in &tool_calls {
            assistant_content.push(MessageContent::tool_use(&tc.id, &tc.name, tc.input.clone()));
        }
        let user_content = vec![
            MessageContent::tool_result("call_create_1", "ok"),
            MessageContent::tool_result("call_update_2", "ok"),
            MessageContent::tool_result("call_delete_3", "ok"),
        ];

        let messages = vec![
            Message::assistant_with_content(assistant_content),
            Message::user_with_content(user_content),
        ];

        // step 4: convert to input items (same as convert_messages in complete())
        let input_items = convert_messages(&messages);

        // step 5: serialize each item to JSON and validate the wire format

        // collect by type for easier assertion
        let mut apc_calls: Vec<Value> = Vec::new();
        let mut apc_outputs: Vec<Value> = Vec::new();
        for item in &input_items {
            let json = serde_json::to_value(item).unwrap();
            match json["type"].as_str() {
                Some("apply_patch_call") => apc_calls.push(json),
                Some("apply_patch_call_output") => apc_outputs.push(json),
                _ => {}
            }
        }
        assert_eq!(apc_calls.len(), 3, "should have 3 apply_patch_call items");
        assert_eq!(
            apc_outputs.len(),
            3,
            "should have 3 apply_patch_call_output items"
        );

        // --- validate apply_patch_call wire format ---

        // create_file call
        let create_call = &apc_calls[0];
        assert_eq!(create_call["type"], "apply_patch_call");
        assert_eq!(create_call["id"], "apc_create_abc", "id must be preserved");
        assert!(
            create_call["id"].as_str().unwrap().starts_with("apc_"),
            "id must start with apc_"
        );
        assert_eq!(create_call["call_id"], "call_create_1");
        assert_eq!(
            create_call["status"], "completed",
            "status is required on apply_patch_call"
        );
        assert_eq!(create_call["operation"]["type"], "create_file");
        assert_eq!(create_call["operation"]["path"], "new.txt");
        assert!(
            create_call["operation"]["diff"].is_string(),
            "create_file should have diff"
        );

        // update_file call
        let update_call = &apc_calls[1];
        assert_eq!(update_call["id"], "apc_update_def");
        assert_eq!(update_call["status"], "completed");
        assert_eq!(update_call["operation"]["type"], "update_file");
        assert!(
            update_call["operation"]["diff"].is_string(),
            "update_file should have diff"
        );

        // delete_file call — must NOT have a diff field
        let delete_call = &apc_calls[2];
        assert_eq!(delete_call["id"], "apc_delete_ghi");
        assert_eq!(delete_call["status"], "completed");
        assert_eq!(delete_call["operation"]["type"], "delete_file");
        assert_eq!(delete_call["operation"]["path"], "obsolete.txt");
        assert!(
            delete_call["operation"].get("diff").is_none(),
            "delete_file must NOT have a diff field — API rejects it as unknown parameter"
        );

        // verify no unexpected fields leak into any call
        for call in &apc_calls {
            let obj = call.as_object().unwrap();
            let allowed_top = ["type", "id", "call_id", "status", "operation"];
            for key in obj.keys() {
                assert!(
                    allowed_top.contains(&key.as_str()),
                    "unexpected top-level field in apply_patch_call: {key}"
                );
            }
        }

        // --- validate apply_patch_call_output wire format ---

        for output in &apc_outputs {
            assert_eq!(output["type"], "apply_patch_call_output");
            assert!(
                output["call_id"].is_string(),
                "call_id is required on output"
            );
            assert!(
                output["status"].as_str() == Some("completed")
                    || output["status"].as_str() == Some("failed"),
                "status must be completed or failed"
            );

            let obj = output.as_object().unwrap();
            let allowed_top = ["type", "call_id", "status", "output"];
            for key in obj.keys() {
                assert!(
                    allowed_top.contains(&key.as_str()),
                    "unexpected field in apply_patch_call_output: {key}"
                );
            }
        }

        // successful results should not have output field (or it's null)
        assert!(
            apc_outputs[0].get("output").is_none() || apc_outputs[0]["output"].is_null(),
            "successful result should omit output"
        );
    }

    #[test]
    fn test_parse_api_error() {
        let json = r#"{"error":{"message":"invalid api key"}}"#;
        let error: ApiError = serde_json::from_str(json).unwrap();
        assert_eq!(error.error.message, "invalid api key");
    }
}
