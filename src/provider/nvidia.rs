use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::Error;
use crate::message::{ContentBlock, Message, MessageContent, Role, ToolResultContent};
use crate::provider::{Provider, ProviderResponse, ReasoningEffort, StopReason, ToolCall, Usage};
use crate::tool::{BuiltInKind, ToolDefinition, text_editor_function_schema};

const API_URL: &str = "https://integrate.api.nvidia.com/v1/chat/completions";
const DEFAULT_MODEL: &str = "deepseek-ai/deepseek-v4-pro";
pub const ALLOWED_MODELS: &[&str] = &["deepseek-ai/deepseek-v4-pro"];
const DEFAULT_MAX_TOKENS: u32 = 8192;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

pub struct NvidiaProvider {
    client: Client,
    api_key: String,
    model: String,
    max_tokens: u32,
    reasoning_effort: ReasoningEffort,
}

impl NvidiaProvider {
    pub fn new(client: Client, api_key: String) -> Self {
        Self {
            client,
            api_key,
            model: DEFAULT_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            reasoning_effort: ReasoningEffort::None,
        }
    }

    pub fn from_env(client: Client) -> Result<Self, Error> {
        let api_key =
            std::env::var("NVIDIA_API_KEY").map_err(|_| Error::MissingApiKey("NVIDIA_API_KEY"))?;
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
            "deepseek-ai/deepseek-v4-pro" => 1_048_576,
            _ => 128_000,
        }
    }
}

#[derive(Debug, Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatTool>,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
struct ChatMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
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
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ApiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default)]
    completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct CompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u32>,
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

fn convert_messages(system_prompt: &str, messages: &[Message]) -> Vec<ChatMessage> {
    let mut out = Vec::new();

    out.push(ChatMessage {
        role: "system".into(),
        content: Some(Value::String(system_prompt.to_string())),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
    });

    for msg in messages {
        match msg.role {
            Role::User | Role::System => {
                let mut content_parts: Vec<Value> = Vec::new();
                for block in &msg.content {
                    match block {
                        MessageContent::Text { text } => {
                            content_parts.push(json!({"type": "text", "text": text}));
                        }
                        MessageContent::Image { .. } => {
                            content_parts.push(json!({"type": "text", "text": "[image]"}));
                        }
                        MessageContent::ToolResult {
                            tool_use_id,
                            content,
                        } => {
                            if !content_parts.is_empty() {
                                out.push(ChatMessage {
                                    role: "user".into(),
                                    content: Some(flatten_content_parts(std::mem::take(
                                        &mut content_parts,
                                    ))),
                                    reasoning_content: None,
                                    tool_calls: None,
                                    tool_call_id: None,
                                });
                            }
                            out.push(ChatMessage {
                                role: "tool".into(),
                                content: Some(tool_result_to_value(content)),
                                reasoning_content: None,
                                tool_calls: None,
                                tool_call_id: Some(tool_use_id.clone()),
                            });
                        }
                        MessageContent::Thinking { .. } | MessageContent::ToolUse { .. } => {}
                    }
                }
                if !content_parts.is_empty() {
                    out.push(ChatMessage {
                        role: "user".into(),
                        content: Some(flatten_content_parts(content_parts)),
                        reasoning_content: None,
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
            }
            Role::Assistant => {
                let mut reasoning_parts = Vec::new();
                let mut text_parts = Vec::new();
                let mut tool_calls = Vec::new();

                for block in &msg.content {
                    match block {
                        MessageContent::Thinking { thinking, .. } if !thinking.is_empty() => {
                            reasoning_parts.push(thinking.clone());
                        }
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
                        MessageContent::Thinking { .. }
                        | MessageContent::Image { .. }
                        | MessageContent::ToolResult { .. } => {}
                    }
                }

                out.push(ChatMessage {
                    role: "assistant".into(),
                    content: if text_parts.is_empty() {
                        None
                    } else {
                        Some(Value::String(text_parts.join("\n")))
                    },
                    reasoning_content: if reasoning_parts.is_empty() {
                        None
                    } else {
                        Some(reasoning_parts.join("\n"))
                    },
                    tool_calls: if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    },
                    tool_call_id: None,
                });
            }
        }
    }

    out
}

fn flatten_content_parts(parts: Vec<Value>) -> Value {
    if parts.len() == 1
        && parts[0].get("type") == Some(&Value::String("text".into()))
        && let Some(text) = parts[0].get("text")
    {
        return text.clone();
    }

    Value::Array(parts)
}

fn tool_result_to_value(content: &ToolResultContent) -> Value {
    match content {
        ToolResultContent::Text(s) => Value::String(s.clone()),
        ToolResultContent::Blocks(blocks) => {
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

fn reasoning_effort(effort: ReasoningEffort) -> Option<&'static str> {
    match effort {
        ReasoningEffort::None => None,
        ReasoningEffort::XHigh => Some("max"),
        ReasoningEffort::Low | ReasoningEffort::Medium | ReasoningEffort::High => Some("high"),
    }
}

fn error_from_response(status: reqwest::StatusCode, error: ApiError) -> Error {
    let msg = &error.error.message;
    if msg.contains("context length") || msg.contains("too many tokens") {
        return Error::ContextOverflow;
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Error::RateLimited(error.error.message);
    }
    if status == reqwest::StatusCode::PAYMENT_REQUIRED
        || msg.to_lowercase().contains("credit")
        || msg.to_lowercase().contains("insufficient")
        || msg.to_lowercase().contains("quota")
    {
        return Error::BudgetExhausted(error.error.message);
    }
    Error::Provider(error.error.message)
}

impl Provider for NvidiaProvider {
    #[tracing::instrument(skip_all, fields(model = %self.model))]
    async fn complete(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ProviderResponse, Error> {
        let request = ApiRequest {
            model: &self.model,
            messages: convert_messages(system_prompt, messages),
            tools: convert_tools(tools),
            max_tokens: self.max_tokens,
            stream: false,
            reasoning_effort: reasoning_effort(self.reasoning_effort),
        };

        let response = self
            .client
            .post(API_URL)
            .timeout(REQUEST_TIMEOUT)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
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
            return Err(error_from_response(status, error));
        }

        let api_response: ApiResponse = response.json().await?;
        let choice = api_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| Error::Provider("empty response from nvidia".into()))?;

        let mut hidden_content = Vec::new();
        if let Some(reasoning_content) = choice.message.reasoning_content
            && !reasoning_content.is_empty()
        {
            hidden_content.push(MessageContent::thinking(reasoning_content, ""));
        }

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
                cache_read_tokens: u.prompt_tokens_details.and_then(|d| d.cached_tokens),
                reasoning_tokens: u.completion_tokens_details.and_then(|d| d.reasoning_tokens),
                ..Default::default()
            })
            .unwrap_or_default();

        Ok(ProviderResponse {
            content: choice.message.content.unwrap_or_default(),
            stop_reason,
            tool_calls,
            hidden_content,
            usage,
        })
    }

    fn cache_ttl(&self) -> Duration {
        Duration::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allowed_models() {
        assert_eq!(DEFAULT_MODEL, "deepseek-ai/deepseek-v4-pro");
        assert_eq!(ALLOWED_MODELS, &["deepseek-ai/deepseek-v4-pro"]);
    }

    #[test]
    fn test_new_uses_default_model() {
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test HTTP client");
        let provider = NvidiaProvider::new(client, "test-key".into());
        assert_eq!(provider.model_name(), "deepseek-ai/deepseek-v4-pro");
        assert_eq!(provider.reasoning_effort(), ReasoningEffort::None);
    }

    #[test]
    fn test_request_serializes_stream_false() {
        let req = ApiRequest {
            model: "deepseek-ai/deepseek-v4-pro",
            messages: vec![ChatMessage {
                role: "user".into(),
                content: Some(Value::String("hello".into())),
                reasoning_content: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            tools: vec![],
            max_tokens: 1024,
            stream: false,
            reasoning_effort: reasoning_effort(ReasoningEffort::High),
        };
        let body = serde_json::to_value(&req).unwrap();
        assert_eq!(body["model"], "deepseek-ai/deepseek-v4-pro");
        assert_eq!(body["stream"], false);
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["reasoning_effort"], "high");
    }

    #[test]
    fn test_convert_tools_includes_function_tools() {
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
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].function.name, "remember");
        assert_eq!(tools[1].function.name, "str_replace_based_edit_tool");
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
    fn test_parse_response_with_reasoning_content() {
        let json = r#"{
            "choices": [{
                "message": {
                    "role": "assistant",
                    "reasoning_content": "private chain",
                    "content": "ok"
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        }"#;

        let response: ApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            response.choices[0].message.reasoning_content.as_deref(),
            Some("private chain")
        );
    }

    #[test]
    fn test_parse_response_with_usage_details() {
        let json = r#"{
            "choices": [{
                "message": {"role": "assistant", "content": "ok"},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 1024,
                "completion_tokens": 80,
                "prompt_tokens_details": {"cached_tokens": 768},
                "completion_tokens_details": {"reasoning_tokens": 64}
            }
        }"#;

        let response: ApiResponse = serde_json::from_str(json).unwrap();
        let usage = response.usage.expect("usage present");
        assert_eq!(usage.prompt_tokens, 1024);
        assert_eq!(
            usage.prompt_tokens_details.and_then(|d| d.cached_tokens),
            Some(768)
        );
        assert_eq!(
            usage
                .completion_tokens_details
                .and_then(|d| d.reasoning_tokens),
            Some(64)
        );
    }

    #[test]
    fn test_error_mapping() {
        let rate_limited = error_from_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            ApiError {
                error: ApiErrorDetail {
                    message: "slow down".into(),
                    code: None,
                },
            },
        );
        assert!(matches!(rate_limited, Error::RateLimited(_)));

        let quota = error_from_response(
            reqwest::StatusCode::BAD_REQUEST,
            ApiError {
                error: ApiErrorDetail {
                    message: "quota exceeded".into(),
                    code: None,
                },
            },
        );
        assert!(matches!(quota, Error::BudgetExhausted(_)));

        let context = error_from_response(
            reqwest::StatusCode::BAD_REQUEST,
            ApiError {
                error: ApiErrorDetail {
                    message: "context length exceeded".into(),
                    code: None,
                },
            },
        );
        assert!(matches!(context, Error::ContextOverflow));
    }
}
