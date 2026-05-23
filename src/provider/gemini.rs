use std::collections::HashMap;
use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::Error;
use crate::message::{ContentBlock, Message, MessageContent, Role, ToolResultContent};
use crate::provider::{Provider, ProviderResponse, ReasoningEffort, StopReason, ToolCall, Usage};
use crate::tool::{BuiltInKind, ToolDefinition, text_editor_function_schema};

const API_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const DEFAULT_MODEL: &str = "gemini-3.5-flash";
pub const ALLOWED_MODELS: &[&str] = &["gemini-3.5-flash", "gemini-3.1-pro-preview"];
const DEFAULT_MAX_TOKENS: u32 = 8192;

pub struct GeminiProvider {
    client: Client,
    api_key: String,
    model: String,
    max_tokens: u32,
    reasoning_effort: ReasoningEffort,
}

impl GeminiProvider {
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
            std::env::var("GEMINI_API_KEY").map_err(|_| Error::MissingApiKey("GEMINI_API_KEY"))?;
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
            "gemini-3.5-flash" | "gemini-3.1-pro-preview" => 1_048_576,
            _ => 128_000,
        }
    }
}

#[derive(Debug, Serialize)]
struct ApiRequest {
    #[serde(rename = "systemInstruction")]
    system_instruction: GeminiContent,
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<GeminiTool>,
    #[serde(rename = "generationConfig")]
    generation_config: GenerationConfig,
}

#[derive(Debug, Serialize)]
struct GenerationConfig {
    #[serde(rename = "maxOutputTokens")]
    max_output_tokens: u32,
    #[serde(rename = "thinkingConfig", skip_serializing_if = "Option::is_none")]
    thinking_config: Option<ThinkingConfig>,
}

#[derive(Debug, Serialize)]
struct ThinkingConfig {
    #[serde(rename = "thinkingBudget")]
    thinking_budget: i32,
    #[serde(rename = "includeThoughts")]
    include_thoughts: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inline_data: Option<InlineData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_call: Option<FunctionCallPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_response: Option<FunctionResponsePart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thought_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InlineData {
    mime_type: String,
    data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FunctionCallPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    name: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FunctionResponsePart {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    name: String,
    response: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiTool {
    function_declarations: Vec<FunctionDeclaration>,
}

#[derive(Debug, Serialize)]
struct FunctionDeclaration {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
    usage_metadata: Option<UsageMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    content: Option<GeminiContent>,
    finish_reason: Option<String>,
    finish_message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageMetadata {
    #[serde(default)]
    prompt_token_count: u32,
    #[serde(default)]
    candidates_token_count: u32,
    #[serde(default)]
    cached_content_token_count: Option<u32>,
    #[serde(default)]
    thoughts_token_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    error: ApiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    message: String,
}

fn convert_messages(messages: &[Message]) -> Vec<GeminiContent> {
    let mut out = Vec::new();
    let mut tool_names_by_id = HashMap::new();

    for msg in messages {
        match msg.role {
            Role::User | Role::System => {
                let mut parts = Vec::new();
                for block in &msg.content {
                    match block {
                        MessageContent::Text { text } => {
                            parts.push(GeminiPart::text(text.clone()));
                        }
                        MessageContent::Image { source } => {
                            parts.push(GeminiPart::inline_data(
                                source.media_type.clone(),
                                source.data.clone(),
                            ));
                        }
                        MessageContent::ToolResult {
                            tool_use_id,
                            content,
                        } => {
                            if !parts.is_empty() {
                                out.push(GeminiContent::user(std::mem::take(&mut parts)));
                            }
                            let name = tool_names_by_id
                                .get(tool_use_id)
                                .cloned()
                                .unwrap_or_else(|| tool_use_id.clone());
                            out.push(GeminiContent::user(vec![GeminiPart::function_response(
                                Some(tool_use_id.clone()),
                                name,
                                tool_result_to_response(content),
                            )]));
                        }
                        MessageContent::Thinking { .. } | MessageContent::ToolUse { .. } => {}
                    }
                }
                if !parts.is_empty() {
                    out.push(GeminiContent::user(parts));
                }
            }
            Role::Assistant => {
                let mut parts = Vec::new();
                let mut pending_thought_signature: Option<String> = None;

                for block in &msg.content {
                    match block {
                        MessageContent::Thinking { signature, .. } => {
                            pending_thought_signature = Some(signature.clone());
                        }
                        MessageContent::Text { text } => {
                            let mut part = GeminiPart::text(text.clone());
                            part.thought_signature = pending_thought_signature.take();
                            parts.push(part);
                        }
                        MessageContent::ToolUse { id, name, input } => {
                            tool_names_by_id.insert(id.clone(), name.clone());
                            let mut part = GeminiPart::function_call(
                                Some(id.clone()),
                                name.clone(),
                                input.clone(),
                            );
                            part.thought_signature = pending_thought_signature.take();
                            parts.push(part);
                        }
                        MessageContent::Image { .. } | MessageContent::ToolResult { .. } => {}
                    }
                }

                if !parts.is_empty() {
                    out.push(GeminiContent::model(parts));
                }
            }
        }
    }

    out
}

fn tool_result_to_response(content: &ToolResultContent) -> Value {
    match content {
        ToolResultContent::Text(s) => json!({ "output": s }),
        ToolResultContent::Blocks(blocks) => {
            let text = blocks
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => text.clone(),
                    ContentBlock::Image { .. } => "[image]".into(),
                })
                .collect::<Vec<_>>()
                .join("\n");
            json!({ "output": text })
        }
    }
}

fn convert_tools(definitions: &[ToolDefinition]) -> Vec<GeminiTool> {
    let declarations: Vec<FunctionDeclaration> = definitions
        .iter()
        .filter_map(|def| match def {
            ToolDefinition::Custom {
                name,
                description,
                input_schema,
            } => Some(FunctionDeclaration {
                name: (*name).to_string(),
                description: (*description).to_string(),
                parameters: input_schema.clone(),
            }),
            ToolDefinition::Dynamic {
                name,
                description,
                input_schema,
            } => Some(FunctionDeclaration {
                name: name.clone(),
                description: description.clone(),
                parameters: input_schema.clone(),
            }),
            ToolDefinition::BuiltIn { kind } => match kind {
                BuiltInKind::AnthropicTextEditor => {
                    let (name, description, parameters) = text_editor_function_schema();
                    Some(FunctionDeclaration {
                        name: name.to_string(),
                        description: description.to_string(),
                        parameters,
                    })
                }
                _ => None,
            },
        })
        .collect();

    if declarations.is_empty() {
        Vec::new()
    } else {
        vec![GeminiTool {
            function_declarations: declarations,
        }]
    }
}

fn thinking_config(model: &str, effort: ReasoningEffort) -> Option<ThinkingConfig> {
    let thinking_budget = match effort {
        ReasoningEffort::None if model.contains("pro") => return None,
        ReasoningEffort::None => 0,
        ReasoningEffort::Low => 1024,
        ReasoningEffort::Medium => 8192,
        ReasoningEffort::High => 24_576,
        ReasoningEffort::XHigh if model.contains("pro") => 32_768,
        ReasoningEffort::XHigh => 24_576,
    };

    Some(ThinkingConfig {
        thinking_budget,
        include_thoughts: false,
    })
}

fn parse_finish_reason(reason: Option<&str>, has_tool_calls: bool) -> StopReason {
    if has_tool_calls {
        return StopReason::ToolUse;
    }

    match reason {
        Some("MAX_TOKENS") => StopReason::MaxTokens,
        Some("STOP") | None => StopReason::EndTurn,
        _ => StopReason::EndTurn,
    }
}

impl GeminiContent {
    fn user(parts: Vec<GeminiPart>) -> Self {
        Self {
            role: Some("user".into()),
            parts,
        }
    }

    fn model(parts: Vec<GeminiPart>) -> Self {
        Self {
            role: Some("model".into()),
            parts,
        }
    }

    fn system(text: impl Into<String>) -> Self {
        Self {
            role: None,
            parts: vec![GeminiPart::text(text.into())],
        }
    }
}

impl GeminiPart {
    fn text(text: String) -> Self {
        Self {
            text: Some(text),
            inline_data: None,
            function_call: None,
            function_response: None,
            thought_signature: None,
        }
    }

    fn inline_data(mime_type: String, data: String) -> Self {
        Self {
            text: None,
            inline_data: Some(InlineData { mime_type, data }),
            function_call: None,
            function_response: None,
            thought_signature: None,
        }
    }

    fn function_call(id: Option<String>, name: String, args: Value) -> Self {
        Self {
            text: None,
            inline_data: None,
            function_call: Some(FunctionCallPart { id, name, args }),
            function_response: None,
            thought_signature: None,
        }
    }

    fn function_response(id: Option<String>, name: String, response: Value) -> Self {
        Self {
            text: None,
            inline_data: None,
            function_call: None,
            function_response: Some(FunctionResponsePart { id, name, response }),
            thought_signature: None,
        }
    }
}

impl Provider for GeminiProvider {
    #[tracing::instrument(skip_all, fields(model = %self.model))]
    async fn complete(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ProviderResponse, Error> {
        let request = ApiRequest {
            system_instruction: GeminiContent::system(system_prompt),
            contents: convert_messages(messages),
            tools: convert_tools(tools),
            generation_config: GenerationConfig {
                max_output_tokens: self.max_tokens,
                thinking_config: thinking_config(&self.model, self.reasoning_effort),
            },
        };

        let url = format!("{API_BASE_URL}/{}:generateContent", self.model);
        let response = self
            .client
            .post(url)
            .query(&[("key", &self.api_key)])
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error: ApiError = response.json().await.unwrap_or(ApiError {
                error: ApiErrorDetail {
                    message: format!("HTTP {status}"),
                },
            });
            let msg = &error.error.message;
            if msg.contains("context") || msg.contains("too many tokens") {
                return Err(Error::ContextOverflow);
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(Error::RateLimited(error.error.message));
            }
            if msg.to_lowercase().contains("quota")
                || msg.to_lowercase().contains("billing")
                || msg.to_lowercase().contains("insufficient")
            {
                return Err(Error::BudgetExhausted(error.error.message));
            }
            return Err(Error::Provider(error.error.message));
        }

        let api_response: ApiResponse = response.json().await?;
        let candidate = api_response
            .candidates
            .into_iter()
            .next()
            .ok_or_else(|| Error::Provider("empty response from gemini".into()))?;

        let mut content_parts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut hidden_content = Vec::new();

        if let Some(content) = candidate.content {
            for (idx, part) in content.parts.into_iter().enumerate() {
                let signature = part.thought_signature.clone();
                if let Some(text) = part.text
                    && !text.is_empty()
                {
                    content_parts.push(text);
                }
                if let Some(call) = part.function_call {
                    if let Some(signature) = signature {
                        hidden_content.push(MessageContent::thinking("", signature));
                    }
                    let id = call.id.unwrap_or_else(|| format!("gemini_call_{idx}"));
                    tool_calls.push(ToolCall {
                        id,
                        name: call.name,
                        input: call.args,
                    });
                }
            }
        }

        let stop_reason =
            parse_finish_reason(candidate.finish_reason.as_deref(), !tool_calls.is_empty());
        if tool_calls.is_empty()
            && matches!(
                candidate.finish_reason.as_deref(),
                Some("SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII")
            )
        {
            return Err(Error::Provider(candidate.finish_message.unwrap_or_else(
                || "gemini response blocked by safety filters".into(),
            )));
        }

        let usage = api_response
            .usage_metadata
            .map(|u| Usage {
                input_tokens: u.prompt_token_count,
                output_tokens: u.candidates_token_count,
                cache_read_tokens: u.cached_content_token_count,
                reasoning_tokens: u.thoughts_token_count,
                ..Default::default()
            })
            .unwrap_or_default();

        Ok(ProviderResponse {
            content: content_parts.join("\n"),
            stop_reason,
            tool_calls,
            hidden_content,
            usage,
        })
    }

    fn cache_ttl(&self) -> Duration {
        // no explicit cachedContent resource is created by this proof of concept.
        Duration::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_user_text_and_image() {
        let messages = vec![Message::user_with_content(vec![
            MessageContent::text("look"),
            MessageContent::Image {
                source: crate::message::ImageSource {
                    source_type: "base64".into(),
                    media_type: "image/png".into(),
                    data: "iVBORw0KGgo=".into(),
                },
            },
        ])];

        let result = convert_messages(&messages);
        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["parts"][0]["text"], "look");
        assert_eq!(json["parts"][1]["inlineData"]["mimeType"], "image/png");
    }

    #[test]
    fn test_convert_tool_call_and_response() {
        let messages = vec![
            Message::assistant_with_content(vec![MessageContent::tool_use(
                "call_1",
                "remember",
                json!({"key": "name"}),
            )]),
            Message::user_with_content(vec![MessageContent::tool_result("call_1", "done")]),
        ];

        let result = convert_messages(&messages);
        let first = serde_json::to_value(&result[0]).unwrap();
        let second = serde_json::to_value(&result[1]).unwrap();
        assert_eq!(first["role"], "model");
        assert_eq!(first["parts"][0]["functionCall"]["name"], "remember");
        assert_eq!(second["parts"][0]["functionResponse"]["name"], "remember");
        assert_eq!(
            second["parts"][0]["functionResponse"]["response"]["output"],
            "done"
        );
    }

    #[test]
    fn test_thought_signature_attaches_to_next_tool_call() {
        let messages = vec![Message::assistant_with_content(vec![
            MessageContent::thinking("", "sig_123"),
            MessageContent::tool_use("call_1", "remember", json!({})),
        ])];

        let result = convert_messages(&messages);
        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["parts"][0]["thoughtSignature"], "sig_123");
        assert_eq!(json["parts"][0]["functionCall"]["name"], "remember");
    }

    #[test]
    fn test_convert_tools_groups_function_declarations() {
        let definitions = vec![
            ToolDefinition::Custom {
                name: "remember",
                description: "remember something",
                input_schema: json!({"type": "object"}),
            },
            ToolDefinition::BuiltIn {
                kind: BuiltInKind::AnthropicTextEditor,
            },
            ToolDefinition::BuiltIn {
                kind: BuiltInKind::OpenAiApplyPatch,
            },
        ];

        let tools = convert_tools(&definitions);
        let json = serde_json::to_value(&tools).unwrap();
        assert_eq!(json[0]["functionDeclarations"].as_array().unwrap().len(), 2);
        assert_eq!(json[0]["functionDeclarations"][0]["name"], "remember");
        assert_eq!(
            json[0]["functionDeclarations"][1]["name"],
            "str_replace_based_edit_tool"
        );
    }

    #[test]
    fn test_thinking_config_by_effort() {
        assert!(thinking_config("gemini-3.1-pro-preview", ReasoningEffort::None).is_none());

        let flash_none = serde_json::to_value(
            thinking_config("gemini-3.5-flash", ReasoningEffort::None).unwrap(),
        )
        .unwrap();
        assert_eq!(flash_none["thinkingBudget"], 0);

        let pro_xhigh = serde_json::to_value(
            thinking_config("gemini-3.1-pro-preview", ReasoningEffort::XHigh).unwrap(),
        )
        .unwrap();
        assert_eq!(pro_xhigh["thinkingBudget"], 32768);
    }

    #[test]
    fn test_parse_response_with_tool_call_and_usage() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "functionCall": {
                            "id": "call_abc",
                            "name": "remember",
                            "args": {"key":"name"}
                        },
                        "thoughtSignature": "sig_123"
                    }]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5,
                "thoughtsTokenCount": 3
            }
        }"#;

        let response: ApiResponse = serde_json::from_str(json).unwrap();
        let candidate = response.candidates.into_iter().next().unwrap();
        let part = candidate.content.unwrap().parts.into_iter().next().unwrap();
        assert_eq!(part.thought_signature.as_deref(), Some("sig_123"));
        assert_eq!(part.function_call.unwrap().name, "remember");
        assert_eq!(
            response.usage_metadata.unwrap().thoughts_token_count,
            Some(3)
        );
    }
}
