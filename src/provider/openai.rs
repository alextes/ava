use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Error;
use crate::message::{ContentBlock, Message, MessageContent, Role, ToolResultContent};
use crate::provider::{Provider, ProviderResponse, StopReason, ToolCall, Usage};
use crate::tool::ToolDefinition;

const API_URL: &str = "https://api.openai.com/v1/responses";
const DEFAULT_MODEL: &str = "gpt-5.4";
pub const ALLOWED_MODELS: &[&str] = &["gpt-5.4", "gpt-5-mini"];
const DEFAULT_MAX_TOKENS: u32 = 8192;

pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    model: String,
    max_tokens: u32,
}

impl OpenAiProvider {
    pub fn new(client: Client, api_key: String) -> Self {
        Self {
            client,
            api_key,
            model: DEFAULT_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
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

    pub fn context_window(&self) -> u32 {
        match self.model.as_str() {
            "gpt-5.4" => 1_050_000,
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
    tools: Vec<FunctionTool>,
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
}

#[derive(Debug, Serialize)]
struct FunctionTool {
    #[serde(rename = "type")]
    tool_type: String,
    name: String,
    description: String,
    parameters: Value,
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
    output_tokens_details: Option<OutputTokensDetails>,
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
    #[serde(other)]
    Other,
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

    for msg in messages {
        match msg.role {
            Role::User | Role::System => {
                let mut text_parts = Vec::new();
                for block in &msg.content {
                    match block {
                        MessageContent::Text { text } => text_parts.push(text.clone()),
                        MessageContent::ToolResult {
                            tool_use_id,
                            content,
                        } => {
                            // flush any accumulated text first
                            if !text_parts.is_empty() {
                                out.push(InputItem::Message {
                                    role: "user".to_string(),
                                    content: Value::String(text_parts.join("\n")),
                                });
                                text_parts.clear();
                            }
                            let output = tool_result_content_to_value(content);
                            out.push(InputItem::FunctionCallOutput {
                                call_id: tool_use_id.clone(),
                                output,
                            });
                        }
                        _ => {}
                    }
                }
                if !text_parts.is_empty() {
                    out.push(InputItem::Message {
                        role: "user".to_string(),
                        content: Value::String(text_parts.join("\n")),
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
                            out.push(InputItem::FunctionCall {
                                call_id: id.clone(),
                                name: name.clone(),
                                arguments: serde_json::to_string(input).unwrap_or_default(),
                            });
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

fn convert_tools(definitions: &[ToolDefinition]) -> Vec<FunctionTool> {
    definitions
        .iter()
        .filter_map(|def| match def {
            ToolDefinition::Custom {
                name,
                description,
                input_schema,
            } => Some(FunctionTool {
                tool_type: "function".to_string(),
                name: (*name).to_string(),
                description: (*description).to_string(),
                parameters: input_schema.clone(),
            }),
            ToolDefinition::Dynamic {
                name,
                description,
                input_schema,
            } => Some(FunctionTool {
                tool_type: "function".to_string(),
                name: name.clone(),
                description: description.clone(),
                parameters: input_schema.clone(),
            }),
            ToolDefinition::BuiltIn { .. } => None,
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
                let reasoning_tokens = u.output_tokens_details.and_then(|d| d.reasoning_tokens);
                Usage {
                    input_tokens: u.input_tokens,
                    output_tokens: u.output_tokens,
                    reasoning_tokens,
                    ..Default::default()
                }
            })
            .unwrap_or_default();

        Ok(ProviderResponse {
            content: content_parts.join(""),
            stop_reason,
            tool_calls,
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let definitions = crate::tool::tool_definitions();
        let custom_count = definitions
            .iter()
            .filter(|d| matches!(d, ToolDefinition::Custom { .. }))
            .count();
        let tools = convert_tools(&definitions);

        // built-in tools should be filtered out
        assert_eq!(tools.len(), custom_count);
        assert!(tools.len() < definitions.len());

        let json = serde_json::to_value(&tools[0]).unwrap();
        assert_eq!(json["type"], "function");
        assert!(json["name"].is_string());
        assert!(json["parameters"].is_object());

        // verify no tool is named after the built-in text editor
        for tool in &tools {
            assert_ne!(tool.name, "str_replace_based_edit_tool");
        }
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
    fn test_parse_api_error() {
        let json = r#"{"error":{"message":"invalid api key"}}"#;
        let error: ApiError = serde_json::from_str(json).unwrap();
        assert_eq!(error.error.message, "invalid api key");
    }
}
