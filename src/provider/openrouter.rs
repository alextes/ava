use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::Error;
use crate::message::{ContentBlock, Message, MessageContent, Role, ToolResultContent};
use crate::provider::{Provider, ProviderResponse, StopReason, ToolCall, Usage};
use crate::tool::{BuiltInKind, ToolDefinition, text_editor_function_schema};

const API_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const DEFAULT_MODEL: &str = "google/gemini-2.5-flash";
const DEFAULT_MAX_TOKENS: u32 = 8192;

pub struct OpenRouterProvider {
    client: Client,
    api_key: String,
    model: String,
    max_tokens: u32,
}

impl OpenRouterProvider {
    pub fn new(client: Client, api_key: String) -> Self {
        Self {
            client,
            api_key,
            model: DEFAULT_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    pub fn from_env(client: Client) -> Result<Self, Error> {
        let api_key = std::env::var("OPENROUTER_API_KEY")
            .map_err(|_| Error::MissingApiKey("OPENROUTER_API_KEY"))?;
        Ok(Self::new(client, api_key))
    }

    pub fn model_name(&self) -> &str {
        &self.model
    }

    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    pub fn context_window(&self) -> u32 {
        // known context windows for popular models
        match self.model.as_str() {
            "google/gemini-2.5-flash" | "google/gemini-2.5-pro" => 1_048_576,
            "anthropic/claude-sonnet-4" | "anthropic/claude-opus-4" => 200_000,
            "anthropic/claude-sonnet-4-6" | "anthropic/claude-opus-4-6" => 1_000_000,
            "openai/gpt-5.5" | "openai/gpt-5.4" => 1_050_000,
            "deepseek/deepseek-v4-pro" | "deepseek/deepseek-v4-flash" => 1_048_576,
            "deepseek/deepseek-chat-v3-0324" => 128_000,
            "meta-llama/llama-4-maverick" => 1_048_576,
            _ => 128_000, // conservative default
        }
    }
}

// -- request types (chat completions format) --

#[derive(Debug, Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatTool>,
    max_tokens: u32,
    provider: ProviderPrefs,
}

/// per-request privacy preferences. OR'd with the account-level setting, so
/// this only tightens — it cannot relax — the account default.
#[derive(Debug, Serialize)]
struct ProviderPrefs {
    /// only route to endpoints with a zero-data-retention policy.
    zdr: bool,
    /// `"deny"` skips providers that store inputs/outputs non-transiently or
    /// may train on them. side effect: some models (notably hosted deepseek)
    /// may have no eligible endpoint and the request will fail — surface that
    /// to the user rather than silently relaxing the policy.
    data_collection: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct ChatMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ChatToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: ChatFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct ChatTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: ChatToolDef,
}

#[derive(Debug, Serialize)]
struct ChatToolDef {
    name: String,
    description: String,
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
struct Choice {
    message: ChoiceMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChoiceMessage {
    #[allow(dead_code)]
    role: Option<String>,
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ApiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    error: ApiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    message: String,
    #[serde(default)]
    #[allow(dead_code)]
    code: Option<Value>,
}

// -- conversion helpers --

fn convert_messages(system_prompt: &str, messages: &[Message]) -> Vec<ChatMessage> {
    let mut out = Vec::new();

    // system message with cache_control (openrouter passes through to anthropic)
    out.push(ChatMessage {
        role: "system".into(),
        content: Some(json!([{
            "type": "text",
            "text": system_prompt,
            "cache_control": {"type": "ephemeral"}
        }])),
        tool_calls: None,
        tool_call_id: None,
    });

    for (msg_idx, msg) in messages.iter().enumerate() {
        let is_last = msg_idx == messages.len() - 1;

        match msg.role {
            Role::User | Role::System => {
                let mut content_parts: Vec<Value> = Vec::new();
                for block in &msg.content {
                    match block {
                        MessageContent::Text { text } => {
                            content_parts.push(json!({"type": "text", "text": text}));
                        }
                        MessageContent::Image { source } => {
                            content_parts.push(json!({
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": source.media_type,
                                    "data": source.data,
                                }
                            }));
                        }
                        MessageContent::ToolResult {
                            tool_use_id,
                            content,
                        } => {
                            // flush content parts first
                            if !content_parts.is_empty() {
                                out.push(ChatMessage {
                                    role: "user".into(),
                                    content: Some(Value::Array(std::mem::take(&mut content_parts))),
                                    tool_calls: None,
                                    tool_call_id: None,
                                });
                            }
                            out.push(ChatMessage {
                                role: "tool".into(),
                                content: Some(tool_result_to_value(content)),
                                tool_calls: None,
                                tool_call_id: Some(tool_use_id.clone()),
                            });
                        }
                        _ => {}
                    }
                }
                if !content_parts.is_empty() {
                    let content = if is_last {
                        // add cache_control on last block for anthropic models
                        if let Some(last) = content_parts.last_mut() {
                            last["cache_control"] = json!({"type": "ephemeral"});
                        }
                        Value::Array(content_parts)
                    } else if content_parts.len() == 1
                        && content_parts[0].get("type") == Some(&Value::String("text".into()))
                        && content_parts[0].get("cache_control").is_none()
                    {
                        // optimize: single text block without cache_control as plain string
                        content_parts[0]["text"].clone()
                    } else {
                        Value::Array(content_parts)
                    };
                    out.push(ChatMessage {
                        role: "user".into(),
                        content: Some(content),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
            }
            Role::Assistant => {
                let mut text_parts = Vec::new();
                let mut tool_calls = Vec::new();

                for block in &msg.content {
                    match block {
                        MessageContent::Text { text } => text_parts.push(text.clone()),
                        MessageContent::ToolUse { id, name, input } => {
                            tool_calls.push(ChatToolCall {
                                id: id.clone(),
                                call_type: "function".into(),
                                function: ChatFunction {
                                    name: name.clone(),
                                    arguments: serde_json::to_string(input).unwrap_or_default(),
                                },
                            });
                        }
                        _ => {}
                    }
                }

                let content = if text_parts.is_empty() {
                    None
                } else {
                    Some(Value::String(text_parts.join("\n")))
                };
                let tool_calls_opt = if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                };

                out.push(ChatMessage {
                    role: "assistant".into(),
                    content,
                    tool_calls: tool_calls_opt,
                    tool_call_id: None,
                });
            }
        }
    }

    out
}

fn tool_result_to_value(content: &ToolResultContent) -> Value {
    match content {
        ToolResultContent::Text(s) => Value::String(s.clone()),
        ToolResultContent::Blocks(blocks) => {
            // for chat completions, tool results must be strings
            let parts: Vec<String> = blocks
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => text.clone(),
                    ContentBlock::Image { .. } => "[image]".into(),
                })
                .collect();
            Value::String(parts.join("\n"))
        }
    }
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
                tool_type: "function".into(),
                function: ChatToolDef {
                    name: (*name).to_string(),
                    description: (*description).to_string(),
                    parameters: input_schema.clone(),
                },
            }),
            ToolDefinition::Dynamic {
                name,
                description,
                input_schema,
            } => Some(ChatTool {
                tool_type: "function".into(),
                function: ChatToolDef {
                    name: name.clone(),
                    description: description.clone(),
                    parameters: input_schema.clone(),
                },
            }),
            ToolDefinition::BuiltIn { kind } => match kind {
                // expose the text editor as a regular function tool for openrouter models
                BuiltInKind::AnthropicTextEditor => {
                    let (name, description, parameters) = text_editor_function_schema();
                    Some(ChatTool {
                        tool_type: "function".into(),
                        function: ChatToolDef {
                            name: name.to_string(),
                            description: description.to_string(),
                            parameters,
                        },
                    })
                }
                // openai-specific built-ins can't be used through chat completions
                _ => None,
            },
        })
        .collect()
}

fn parse_finish_reason(reason: Option<&str>) -> StopReason {
    match reason {
        Some("stop") => StopReason::EndTurn,
        Some("length") => StopReason::MaxTokens,
        Some("tool_calls") => StopReason::ToolUse,
        _ => StopReason::EndTurn,
    }
}

impl Provider for OpenRouterProvider {
    #[tracing::instrument(skip_all, fields(model = %self.model))]
    async fn complete(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ProviderResponse, Error> {
        let chat_messages = convert_messages(system_prompt, messages);
        let chat_tools = convert_tools(tools);

        let request = ApiRequest {
            model: &self.model,
            messages: chat_messages,
            tools: chat_tools,
            max_tokens: self.max_tokens,
            provider: ProviderPrefs {
                zdr: true,
                data_collection: "deny",
            },
        };

        let response = self
            .client
            .post(API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("X-Title", "ava")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error: ApiError = response.json().await.unwrap_or(ApiError {
                error: ApiErrorDetail {
                    message: format!("HTTP {status}"),
                    code: None,
                },
            });
            let msg = &error.error.message;
            if msg.contains("context length") || msg.contains("too many tokens") {
                return Err(Error::ContextOverflow);
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(Error::RateLimited(error.error.message));
            }
            if status == reqwest::StatusCode::PAYMENT_REQUIRED
                || msg.to_lowercase().contains("credit")
                || msg.to_lowercase().contains("insufficient")
            {
                return Err(Error::BudgetExhausted(error.error.message));
            }
            return Err(Error::Provider(error.error.message));
        }

        let api_response: ApiResponse = response.json().await?;

        let choice = api_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| Error::Provider("empty response from openrouter".into()))?;

        let content = choice.message.content.unwrap_or_default();

        let tool_calls: Vec<ToolCall> = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| {
                let input: Value = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or(Value::Object(serde_json::Map::new()));
                ToolCall {
                    id: tc.id,
                    name: tc.function.name,
                    input,
                }
            })
            .collect();

        let stop_reason = if !tool_calls.is_empty() {
            StopReason::ToolUse
        } else {
            parse_finish_reason(choice.finish_reason.as_deref())
        };

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

    fn cache_ttl(&self) -> Duration {
        // no cache_control is sent on openrouter requests; treat every
        // resume as cold so callers can apply cost-aware policies.
        Duration::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_user_message() {
        let messages = vec![Message::user("hello")];
        let result = convert_messages("system", &messages);

        // system + user
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "system");
        assert_eq!(result[1].role, "user");
    }

    #[test]
    fn test_convert_assistant_with_tool_calls() {
        let messages = vec![Message::assistant_with_content(vec![
            MessageContent::text("thinking..."),
            MessageContent::tool_use("call_1", "remember", json!({"key": "v"})),
        ])];
        let result = convert_messages("sys", &messages);

        // system + assistant (with text + tool_calls in one message)
        assert_eq!(result.len(), 2);
        assert_eq!(result[1].role, "assistant");
        assert_eq!(
            result[1].content.as_ref().unwrap().as_str().unwrap(),
            "thinking..."
        );
        let tc = result[1].tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].function.name, "remember");
    }

    #[test]
    fn test_convert_tool_result() {
        let messages = vec![Message::user_with_content(vec![
            MessageContent::tool_result("call_1", "done"),
        ])];
        let result = convert_messages("sys", &messages);

        // system + tool result
        assert_eq!(result.len(), 2);
        assert_eq!(result[1].role, "tool");
        assert_eq!(result[1].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(
            result[1].content.as_ref().unwrap().as_str().unwrap(),
            "done"
        );
    }

    #[test]
    fn test_convert_tools_includes_text_editor_as_function() {
        let definitions = vec![
            ToolDefinition::Custom {
                name: "remember",
                description: "remember something",
                input_schema: json!({"type": "object"}),
            },
            ToolDefinition::BuiltIn {
                kind: crate::tool::BuiltInKind::AnthropicTextEditor,
            },
            ToolDefinition::BuiltIn {
                kind: crate::tool::BuiltInKind::OpenAiApplyPatch,
            },
        ];
        let tools = convert_tools(&definitions);
        // remember + text_editor (as function), apply_patch is skipped
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].function.name, "remember");
        assert_eq!(tools[1].function.name, "str_replace_based_edit_tool");
    }

    #[test]
    fn test_parse_finish_reason() {
        assert_eq!(parse_finish_reason(Some("stop")), StopReason::EndTurn);
        assert_eq!(parse_finish_reason(Some("length")), StopReason::MaxTokens);
        assert_eq!(parse_finish_reason(Some("tool_calls")), StopReason::ToolUse);
        assert_eq!(parse_finish_reason(None), StopReason::EndTurn);
    }

    #[test]
    fn test_parse_response() {
        let json = r#"{
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "hello there"
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        }"#;

        let response: ApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.choices.len(), 1);
        assert_eq!(
            response.choices[0].message.content.as_deref(),
            Some("hello there")
        );
        assert_eq!(response.choices[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_parse_tool_call_response() {
        let json = r#"{
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "remember",
                            "arguments": "{\"key\":\"name\",\"value\":\"alex\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }"#;

        let response: ApiResponse = serde_json::from_str(json).unwrap();
        let tc = response.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].id, "call_abc");
        assert_eq!(tc[0].function.name, "remember");
    }

    #[test]
    fn test_system_message_has_cache_control() {
        let messages = vec![Message::user("hi")];
        let result = convert_messages("be helpful", &messages);
        let sys = &result[0];
        let content = sys.content.as_ref().unwrap();
        let arr = content.as_array().unwrap();
        assert_eq!(arr[0]["cache_control"]["type"], "ephemeral");
        assert_eq!(arr[0]["text"], "be helpful");
    }

    #[test]
    fn test_request_carries_zdr_and_deny_data_collection() {
        let req = ApiRequest {
            model: "anthropic/claude-sonnet-4-6",
            messages: vec![],
            tools: vec![],
            max_tokens: 1024,
            provider: ProviderPrefs {
                zdr: true,
                data_collection: "deny",
            },
        };
        let body = serde_json::to_value(&req).unwrap();
        assert_eq!(body["provider"]["zdr"], true);
        assert_eq!(body["provider"]["data_collection"], "deny");
    }

    #[test]
    fn test_last_user_message_has_cache_control() {
        let messages = vec![Message::user("first"), Message::user("second")];
        let result = convert_messages("sys", &messages);
        // system, first user, second user (last)
        assert_eq!(result.len(), 3);
        // first user: plain string
        assert!(result[1].content.as_ref().unwrap().is_string());
        // last user: array with cache_control
        let last_content = result[2].content.as_ref().unwrap();
        assert!(last_content.is_array());
        assert_eq!(last_content[0]["cache_control"]["type"], "ephemeral");
    }
}
