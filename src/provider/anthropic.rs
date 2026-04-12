use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::Error;
use crate::message::Message;
use crate::provider::{Provider, ProviderResponse, StopReason, ToolCall, Usage};
use crate::tool::ToolDefinition;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
pub const ALLOWED_MODELS: &[&str] = &["claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5"];
const DEFAULT_MAX_TOKENS: u32 = 8192;

pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    model: String,
    max_tokens: u32,
}

impl AnthropicProvider {
    pub fn new(client: Client, api_key: String) -> Self {
        Self {
            client,
            api_key,
            model: DEFAULT_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    pub fn from_env(client: Client) -> Result<Self, Error> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| Error::MissingApiKey("ANTHROPIC_API_KEY"))?;
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
            "claude-opus-4-6" | "claude-sonnet-4-6" => 1_000_000,
            _ => 200_000,
        }
    }
}

#[derive(Debug, Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: serde_json::Value,
    messages: serde_json::Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
    stop_reason: StopReason,
    usage: ApiUsage,
}

#[derive(Debug, Deserialize)]
struct ApiUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Deserialize)]
struct ApiError {
    error: ApiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    message: String,
}

impl Provider for AnthropicProvider {
    #[tracing::instrument(skip_all, fields(model = %self.model))]
    async fn complete(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ProviderResponse, Error> {
        // system prompt as array with cache_control breakpoint
        let system = json!([{
            "type": "text",
            "text": system_prompt,
            "cache_control": {"type": "ephemeral"}
        }]);

        // serialize messages and add cache_control to the last content block
        // of the last message for incremental conversation caching
        let mut messages_value = serde_json::to_value(messages)
            .map_err(|e| Error::Provider(format!("failed to serialize messages: {e}")))?;
        if let Some(last_msg) = messages_value.as_array_mut().and_then(|a| a.last_mut())
            && let Some(last_block) = last_msg
                .get_mut("content")
                .and_then(|c| c.as_array_mut())
                .and_then(|a| a.last_mut())
        {
            last_block["cache_control"] = json!({"type": "ephemeral"});
        }

        let tools_json: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| match t {
                ToolDefinition::Custom {
                    name,
                    description,
                    input_schema,
                } => json!({
                    "name": name,
                    "description": description,
                    "input_schema": input_schema,
                }),
                ToolDefinition::BuiltIn { tool_type, name } => json!({
                    "type": tool_type,
                    "name": name,
                }),
                ToolDefinition::Dynamic {
                    name,
                    description,
                    input_schema,
                } => json!({
                    "name": name,
                    "description": description,
                    "input_schema": input_schema,
                }),
            })
            .collect();

        let request = ApiRequest {
            model: &self.model,
            max_tokens: self.max_tokens,
            system,
            messages: messages_value,
            tools: tools_json,
        };

        let response = self
            .client
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error: ApiError = response.json().await?;
            let msg = &error.error.message;
            if msg.contains("prompt is too long") || msg.contains("too many tokens") {
                return Err(Error::ContextOverflow);
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(Error::RateLimited(error.error.message));
            }
            if status == reqwest::StatusCode::BAD_REQUEST
                && msg.to_lowercase().contains("credit balance")
            {
                return Err(Error::BudgetExhausted(error.error.message));
            }
            return Err(Error::Provider(error.error.message));
        }

        let api_response: ApiResponse = response.json().await?;

        let mut content = String::new();
        let mut tool_calls = Vec::new();

        for block in api_response.content {
            match block {
                ContentBlock::Text { text } => {
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str(&text);
                }
                ContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall { id, name, input });
                }
            }
        }

        Ok(ProviderResponse {
            content,
            stop_reason: api_response.stop_reason,
            tool_calls,
            usage: Usage {
                input_tokens: api_response.usage.input_tokens,
                output_tokens: api_response.usage.output_tokens,
                cache_creation_tokens: api_response.usage.cache_creation_input_tokens,
                cache_read_tokens: api_response.usage.cache_read_input_tokens,
                ..Default::default()
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_text_response() {
        let json = r#"{"content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
        let response: ApiResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.content.len(), 1);
        match &response.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected text block"),
        }
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 5);
    }

    #[test]
    fn test_parse_multiple_text_blocks() {
        let json = r#"{"content":[{"type":"text","text":"hello"},{"type":"text","text":"world"}],"stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
        let response: ApiResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.content.len(), 2);

        // verify the joining logic works as expected
        let mut content = String::new();
        for block in &response.content {
            if let ContentBlock::Text { text } = block {
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(text);
            }
        }
        assert_eq!(content, "hello\nworld");
    }

    #[test]
    fn test_parse_tool_use_response() {
        let json = r#"{"content":[{"type":"tool_use","id":"toolu_123","name":"get_weather","input":{"location":"sf"}}],"stop_reason":"tool_use","usage":{"input_tokens":20,"output_tokens":15}}"#;
        let response: ApiResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.content.len(), 1);
        match &response.content[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_123");
                assert_eq!(name, "get_weather");
                assert_eq!(input["location"], "sf");
            }
            _ => panic!("expected tool_use block"),
        }
        assert_eq!(response.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn test_parse_usage_with_cache() {
        let json = r#"{"content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":80,"cache_read_input_tokens":0}}"#;
        let response: ApiResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.usage.input_tokens, 100);
        assert_eq!(response.usage.output_tokens, 50);
        assert_eq!(response.usage.cache_creation_input_tokens, Some(80));
        assert_eq!(response.usage.cache_read_input_tokens, Some(0));
    }

    #[test]
    fn test_parse_api_error() {
        let json = r#"{"error":{"message":"invalid api key"}}"#;
        let error: ApiError = serde_json::from_str(json).unwrap();

        assert_eq!(error.error.message, "invalid api key");
    }

    #[test]
    fn test_request_serialization() {
        let messages = vec![Message::user("hello")];
        let tools = crate::tool::tool_definitions(false);

        let system = json!([{
            "type": "text",
            "text": "test system prompt",
            "cache_control": {"type": "ephemeral"}
        }]);

        let mut messages_value = serde_json::to_value(&messages).unwrap();
        if let Some(last_msg) = messages_value.as_array_mut().and_then(|a| a.last_mut())
            && let Some(last_block) = last_msg
                .get_mut("content")
                .and_then(|c| c.as_array_mut())
                .and_then(|a| a.last_mut())
        {
            last_block["cache_control"] = json!({"type": "ephemeral"});
        }

        let tools_json: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| match t {
                ToolDefinition::Custom {
                    name,
                    description,
                    input_schema,
                } => json!({
                    "name": name,
                    "description": description,
                    "input_schema": input_schema,
                }),
                ToolDefinition::BuiltIn { tool_type, name } => json!({
                    "type": tool_type,
                    "name": name,
                }),
                ToolDefinition::Dynamic {
                    name,
                    description,
                    input_schema,
                } => json!({
                    "name": name,
                    "description": description,
                    "input_schema": input_schema,
                }),
            })
            .collect();

        let request = ApiRequest {
            model: "claude-sonnet-4-6",
            max_tokens: 1024,
            system,
            messages: messages_value,
            tools: tools_json,
        };

        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["model"], "claude-sonnet-4-6");
        assert_eq!(json["max_tokens"], 1024);
        // system is now an array with cache_control
        assert_eq!(json["system"][0]["text"], "test system prompt");
        assert_eq!(json["system"][0]["cache_control"]["type"], "ephemeral");
        // messages have cache_control on the last block
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(json["messages"][0]["content"][0]["text"], "hello");
        assert_eq!(
            json["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        // custom tool has name, description, input_schema (no type)
        assert_eq!(json["tools"][0]["name"], "remember");
        assert!(json["tools"][0]["description"].is_string());
        assert!(json["tools"][0]["input_schema"].is_object());
        assert!(json["tools"][0].get("type").is_none());

        // built-in tool has type and name (no description, no input_schema)
        let builtin_tool = json["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t.get("type").is_some_and(|v| v.is_string()))
            .expect("should have a built-in tool");
        assert_eq!(builtin_tool["type"], "text_editor_20250728");
        assert_eq!(builtin_tool["name"], "str_replace_based_edit_tool");
        assert!(builtin_tool.get("description").is_none());
        assert!(builtin_tool.get("input_schema").is_none());
    }
}
