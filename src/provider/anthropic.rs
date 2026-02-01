use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::message::Message;
use crate::provider::{Provider, ProviderResponse, StopReason, ToolCall};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_MODEL: &str = "claude-sonnet-4-5";
const DEFAULT_MAX_TOKENS: u32 = 8192;

pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    model: String,
    max_tokens: u32,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            model: DEFAULT_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    pub fn from_env() -> Result<Self, Error> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| Error::MissingApiKey("ANTHROPIC_API_KEY"))?;
        Ok(Self::new(api_key))
    }
}

#[derive(Debug, Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: &'a [Message],
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
    stop_reason: StopReason,
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
    async fn complete(&self, messages: &[Message]) -> Result<ProviderResponse, Error> {
        let request = ApiRequest {
            model: &self.model,
            max_tokens: self.max_tokens,
            messages,
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

        if !response.status().is_success() {
            let error: ApiError = response.json().await?;
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_text_response() {
        let json = r#"{"content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn"}"#;
        let response: ApiResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.content.len(), 1);
        match &response.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("expected text block"),
        }
        assert_eq!(response.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn test_parse_multiple_text_blocks() {
        let json = r#"{"content":[{"type":"text","text":"hello"},{"type":"text","text":"world"}],"stop_reason":"end_turn"}"#;
        let response: ApiResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.content.len(), 2);

        // verify the joining logic works as expected
        let mut content = String::new();
        for block in &response.content {
            match block {
                ContentBlock::Text { text } => {
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str(text);
                }
                _ => {}
            }
        }
        assert_eq!(content, "hello\nworld");
    }

    #[test]
    fn test_parse_tool_use_response() {
        let json = r#"{"content":[{"type":"tool_use","id":"toolu_123","name":"get_weather","input":{"location":"sf"}}],"stop_reason":"tool_use"}"#;
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
    fn test_parse_api_error() {
        let json = r#"{"error":{"message":"invalid api key"}}"#;
        let error: ApiError = serde_json::from_str(json).unwrap();

        assert_eq!(error.error.message, "invalid api key");
    }

    #[test]
    fn test_request_serialization() {
        let messages = vec![Message::user("hello")];
        let request = ApiRequest {
            model: "claude-sonnet-4-5",
            max_tokens: 1024,
            messages: &messages,
        };

        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["model"], "claude-sonnet-4-5");
        assert_eq!(json["max_tokens"], 1024);
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(json["messages"][0]["content"], "hello");
    }
}
