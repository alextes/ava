use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Error;
use crate::message::{Message, MessageContent, Role};
use crate::provider::{Provider, ProviderResponse, StopReason, ToolCall, Usage};
use crate::tool::{ToolDefinition, tool_definitions};

const API_URL: &str = "https://api.openai.com/v1/chat/completions";
const DEFAULT_MODEL: &str = "gpt-5.2";
pub const ALLOWED_MODELS: &[&str] = &["gpt-5.2", "gpt-5-mini"];
const DEFAULT_MAX_TOKENS: u32 = 8192;

pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    model: String,
    max_tokens: u32,
}

impl OpenAiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model: DEFAULT_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    pub fn from_env() -> Result<Self, Error> {
        let api_key =
            std::env::var("OPENAI_API_KEY").map_err(|_| Error::MissingApiKey("OPENAI_API_KEY"))?;
        Ok(Self::new(api_key))
    }

    pub fn model_name(&self) -> &str {
        &self.model
    }

    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    pub fn context_window(&self) -> u32 {
        400_000
    }
}

// -- request types --

#[derive(Debug, Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    max_completion_tokens: u32,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatTool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role")]
enum ChatMessage {
    #[serde(rename = "system")]
    System { content: String },
    #[serde(rename = "user")]
    User { content: String },
    #[serde(rename = "assistant")]
    Assistant {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ChatToolCall>,
    },
    #[serde(rename = "tool")]
    Tool {
        content: String,
        tool_call_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: ChatFunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct ChatTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: ChatFunction,
}

#[derive(Debug, Serialize)]
struct ChatFunction {
    name: &'static str,
    description: &'static str,
    parameters: Value,
}

// -- response types --

#[derive(Debug, Deserialize)]
struct ApiResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<ApiUsage>,
}

#[derive(Debug, Deserialize)]
struct ApiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
    finish_reason: String,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ChatToolCall>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    error: ApiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    message: String,
}

// -- conversion helpers --

fn convert_messages(system_prompt: &str, messages: &[Message]) -> Vec<ChatMessage> {
    let mut out = vec![ChatMessage::System {
        content: system_prompt.to_string(),
    }];

    for msg in messages {
        match msg.role {
            Role::User => {
                // user messages can contain text and tool results
                // tool results become separate "tool" messages in openai format
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
                                out.push(ChatMessage::User {
                                    content: text_parts.join("\n"),
                                });
                                text_parts.clear();
                            }
                            out.push(ChatMessage::Tool {
                                content: content.clone(),
                                tool_call_id: tool_use_id.clone(),
                            });
                        }
                        _ => {}
                    }
                }
                if !text_parts.is_empty() {
                    out.push(ChatMessage::User {
                        content: text_parts.join("\n"),
                    });
                }
            }
            Role::Assistant => {
                let mut text_content = None;
                let mut tool_calls = Vec::new();

                for block in &msg.content {
                    match block {
                        MessageContent::Text { text } => {
                            text_content = Some(text.clone());
                        }
                        MessageContent::ToolUse { id, name, input } => {
                            tool_calls.push(ChatToolCall {
                                id: id.clone(),
                                call_type: "function".to_string(),
                                function: ChatFunctionCall {
                                    name: name.clone(),
                                    arguments: serde_json::to_string(input).unwrap_or_default(),
                                },
                            });
                        }
                        _ => {}
                    }
                }

                out.push(ChatMessage::Assistant {
                    content: text_content,
                    tool_calls,
                });
            }
        }
    }

    out
}

fn convert_tools(definitions: &[ToolDefinition]) -> Vec<ChatTool> {
    definitions
        .iter()
        .filter_map(|def| match def {
            ToolDefinition::Custom {
                name,
                description,
                input_schema,
            } => Some(ChatTool {
                tool_type: "function".to_string(),
                function: ChatFunction {
                    name,
                    description,
                    parameters: input_schema.clone(),
                },
            }),
            ToolDefinition::BuiltIn { .. } => None,
        })
        .collect()
}

fn parse_finish_reason(reason: &str) -> StopReason {
    match reason {
        "stop" => StopReason::EndTurn,
        "length" => StopReason::MaxTokens,
        "tool_calls" => StopReason::ToolUse,
        _ => StopReason::EndTurn,
    }
}

impl Provider for OpenAiProvider {
    #[tracing::instrument(skip_all, fields(model = %self.model))]
    async fn complete(
        &self,
        system_prompt: &str,
        messages: &[Message],
    ) -> Result<ProviderResponse, Error> {
        let tools = tool_definitions();
        let chat_messages = convert_messages(system_prompt, messages);
        let chat_tools = convert_tools(&tools);

        let request = ApiRequest {
            model: &self.model,
            max_completion_tokens: self.max_tokens,
            messages: chat_messages,
            tools: chat_tools,
        };

        let response = self
            .client
            .post(API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error: ApiError = response.json().await?;
            let msg = &error.error.message;
            if msg.contains("maximum context length") || msg.contains("too many tokens") {
                return Err(Error::ContextOverflow);
            }
            return Err(Error::Provider(error.error.message));
        }

        let api_response: ApiResponse = response.json().await?;

        let choice = api_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| Error::Provider("empty response from openai".into()))?;

        let content = choice.message.content.unwrap_or_default();
        let stop_reason = parse_finish_reason(&choice.finish_reason);

        let mut tool_calls = Vec::new();
        for tc in choice.message.tool_calls {
            let input: Value = serde_json::from_str(&tc.function.arguments)
                .unwrap_or(Value::Object(serde_json::Map::new()));
            tool_calls.push(ToolCall {
                id: tc.id,
                name: tc.function.name,
                input,
            });
        }

        let usage = api_response
            .usage
            .map(|u| Usage {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
                ..Default::default()
            })
            .unwrap_or_default();

        Ok(ProviderResponse {
            content,
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
        let result = convert_messages("system", &messages);

        assert_eq!(result.len(), 2);
        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["role"], "system");
        assert_eq!(json["content"], "system");

        let json = serde_json::to_value(&result[1]).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "hello");
    }

    #[test]
    fn test_convert_assistant_with_tool_use() {
        let messages = vec![Message::assistant_with_content(vec![
            MessageContent::text("thinking..."),
            MessageContent::tool_use("call_123", "remember_fact", serde_json::json!({"key": "v"})),
        ])];

        let result = convert_messages("sys", &messages);
        assert_eq!(result.len(), 2); // system + assistant

        let json = serde_json::to_value(&result[1]).unwrap();
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["content"], "thinking...");
        assert_eq!(json["tool_calls"][0]["id"], "call_123");
        assert_eq!(json["tool_calls"][0]["type"], "function");
        assert_eq!(json["tool_calls"][0]["function"]["name"], "remember_fact");
    }

    #[test]
    fn test_convert_tool_result_message() {
        let messages = vec![Message::user_with_content(vec![
            MessageContent::tool_result("call_123", "ok"),
        ])];

        let result = convert_messages("sys", &messages);
        assert_eq!(result.len(), 2); // system + tool

        let json = serde_json::to_value(&result[1]).unwrap();
        assert_eq!(json["role"], "tool");
        assert_eq!(json["content"], "ok");
        assert_eq!(json["tool_call_id"], "call_123");
    }

    #[test]
    fn test_convert_tools() {
        let definitions = tool_definitions();
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
        assert!(json["function"]["name"].is_string());
        assert!(json["function"]["parameters"].is_object());

        // verify no tool is named after the built-in text editor
        for tool in &tools {
            assert_ne!(tool.function.name, "str_replace_based_edit_tool");
        }
    }

    #[test]
    fn test_parse_response() {
        let json = r#"{
            "choices": [{
                "message": {
                    "content": "hello there",
                    "tool_calls": []
                },
                "finish_reason": "stop"
            }]
        }"#;

        let response: ApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.choices.len(), 1);
        assert_eq!(
            response.choices[0].message.content.as_deref(),
            Some("hello there")
        );
        assert_eq!(response.choices[0].finish_reason, "stop");
    }

    #[test]
    fn test_parse_tool_call_response() {
        let json = r#"{
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "remember_fact",
                            "arguments": "{\"category\":\"user\",\"key\":\"name\",\"value\":\"alex\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }"#;

        let response: ApiResponse = serde_json::from_str(json).unwrap();
        let tc = &response.choices[0].message.tool_calls[0];
        assert_eq!(tc.id, "call_abc");
        assert_eq!(tc.function.name, "remember_fact");

        let args: Value = serde_json::from_str(&tc.function.arguments).unwrap();
        assert_eq!(args["category"], "user");
    }

    #[test]
    fn test_parse_finish_reasons() {
        assert_eq!(parse_finish_reason("stop"), StopReason::EndTurn);
        assert_eq!(parse_finish_reason("length"), StopReason::MaxTokens);
        assert_eq!(parse_finish_reason("tool_calls"), StopReason::ToolUse);
        assert_eq!(parse_finish_reason("unknown"), StopReason::EndTurn);
    }

    #[test]
    fn test_parse_api_error() {
        let json = r#"{"error":{"message":"invalid api key"}}"#;
        let error: ApiError = serde_json::from_str(json).unwrap();
        assert_eq!(error.error.message, "invalid api key");
    }
}
