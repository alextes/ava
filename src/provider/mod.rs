mod anthropic;
mod deepseek;
mod gemini;
mod openai;
mod openrouter;

pub use crate::tool::ToolCall;
pub use anthropic::AnthropicProvider;
pub use deepseek::DeepSeekProvider;
pub use gemini::GeminiProvider;
pub use openai::OpenAiProvider;
pub use openrouter::OpenRouterProvider;

use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::error::Error;
use crate::message::Message;
use crate::tool::ToolDefinition;

pub const DEFAULT_SYSTEM_PROMPT: &str = "you are an ai assistant.";

pub const SETUP_SYSTEM_PROMPT: &str = "\
you are an ai assistant that has just been initialized for the first time. \
your first task is to complete initial setup with the user.

**important:** your full tool harness (shell, web search, file editing, etc.) is disabled \
until setup is complete. only the `complete_setup` and `remember` tools are available right now. \
once you finish setup, all tools will become available.

ask the user:
1. **what should they call you?** a name is strongly encouraged — without one, features \
like group chat participation and intelligent replies won't work well because there's no \
name to refer to you by.
2. **any identity traits or behavioral preferences?** for example: tone, personality, \
areas of expertise, communication style. this is entirely optional — defaults are fine.

let the user know that all of these can be updated at any time after setup using the \
`remember` tool with `kind: identity`.

once the user has chosen at least a name, call the `complete_setup` tool to finalize. \
keep this brief — don't over-explain. get the name, optionally traits, and complete setup.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// anthropic: tokens written to cache this request
    pub cache_creation_tokens: Option<u32>,
    /// anthropic: tokens read from cache this request
    pub cache_read_tokens: Option<u32>,
    /// reasoning tokens used by reasoning models when the provider reports
    /// them (subset of output_tokens).
    pub reasoning_tokens: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Low,
    Medium,
    High,
    XHigh,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }

    pub fn from_persisted(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::XHigh),
            _ => None,
        }
    }

    pub fn from_user_input(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" | "off" => Some(Self::None),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::XHigh),
            _ => None,
        }
    }
}

impl fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub content: String,
    #[allow(dead_code)]
    pub stop_reason: StopReason,
    pub tool_calls: Vec<ToolCall>,
    pub hidden_content: Vec<crate::message::MessageContent>,
    pub usage: Usage,
}

pub trait Provider: Send + Sync {
    fn complete(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> impl Future<Output = Result<ProviderResponse, Error>> + Send;

    /// how long the prompt cache is expected to stay warm after the most
    /// recent request. callers use this to detect "cold resume" situations
    /// where the whole conversation will be re-processed as uncached input.
    ///
    /// - anthropic: `Duration::from_secs(300)` (ephemeral, 5 min)
    /// - deepseek: `Duration::from_secs(300)` (implicit prefix cache estimate)
    /// - gemini: `Duration::ZERO` (no explicit cachedContent is created)
    /// - openai: `Duration::from_secs(24 * 3600)` (24h retention hint)
    /// - openrouter: `Duration::from_secs(300)` (implicit prefix cache estimate)
    fn cache_ttl(&self) -> Duration;
}

// -- AnyProvider enum dispatch --

pub enum AnyProvider {
    Anthropic(AnthropicProvider),
    DeepSeek(DeepSeekProvider),
    Gemini(GeminiProvider),
    OpenAi(OpenAiProvider),
    OpenRouter(OpenRouterProvider),
    #[cfg(test)]
    Test(TestProvider),
}

impl AnyProvider {
    /// create the default provider from environment variables.
    /// anthropic wins when available to preserve the historical default.
    pub fn default_from_env(client: reqwest::Client) -> Result<Self, Error> {
        if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            return Ok(Self::Anthropic(AnthropicProvider::from_env(client)?));
        }
        if std::env::var("GEMINI_API_KEY").is_ok() {
            return Ok(Self::Gemini(GeminiProvider::from_env(client)?));
        }
        if std::env::var("OPENAI_API_KEY").is_ok() {
            return Ok(Self::OpenAi(OpenAiProvider::from_env(client)?));
        }
        if std::env::var("DEEPSEEK_API_KEY").is_ok() {
            let mut provider = Self::DeepSeek(DeepSeekProvider::from_env(client)?);
            let effort = Self::default_reasoning_effort(&provider.model_id());
            provider.set_reasoning_effort(effort);
            return Ok(provider);
        }
        if std::env::var("OPENROUTER_API_KEY").is_ok() {
            return Ok(Self::OpenRouter(OpenRouterProvider::from_env(client)?));
        }

        Err(Error::MissingApiKey(
            "ANTHROPIC_API_KEY, GEMINI_API_KEY, OPENAI_API_KEY, DEEPSEEK_API_KEY, or OPENROUTER_API_KEY",
        ))
    }

    /// returns a `"provider/model"` identifier string (e.g. `"anthropic/claude-sonnet-4-6"`)
    pub fn model_id(&self) -> String {
        match self {
            Self::Anthropic(p) => format!("anthropic/{}", p.model_name()),
            Self::DeepSeek(p) => format!("deepseek/{}", p.model_name()),
            Self::Gemini(p) => format!("gemini/{}", p.model_name()),
            Self::OpenAi(p) => format!("openai/{}", p.model_name()),
            Self::OpenRouter(p) => format!("openrouter/{}", p.model_name()),
            #[cfg(test)]
            Self::Test(_) => "test/test".to_string(),
        }
    }

    pub fn reasoning_effort(&self) -> ReasoningEffort {
        match self {
            Self::Anthropic(p) => p.reasoning_effort(),
            Self::DeepSeek(p) => p.reasoning_effort(),
            Self::Gemini(p) => p.reasoning_effort(),
            Self::OpenAi(p) => p.reasoning_effort(),
            Self::OpenRouter(p) => p.reasoning_effort(),
            #[cfg(test)]
            Self::Test(_) => ReasoningEffort::None,
        }
    }

    pub fn set_reasoning_effort(&mut self, effort: ReasoningEffort) {
        match self {
            Self::Anthropic(p) => p.set_reasoning_effort(effort),
            Self::DeepSeek(p) => p.set_reasoning_effort(effort),
            Self::Gemini(p) => p.set_reasoning_effort(effort),
            Self::OpenAi(p) => p.set_reasoning_effort(effort),
            Self::OpenRouter(p) => p.set_reasoning_effort(effort),
            #[cfg(test)]
            Self::Test(_) => {}
        }
    }

    pub fn default_reasoning_effort(model_id: &str) -> ReasoningEffort {
        match model_id {
            "openrouter/deepseek/deepseek-v4-pro" | "openrouter/deepseek/deepseek-v4-flash" => {
                ReasoningEffort::High
            }
            "deepseek/deepseek-v4-pro" | "deepseek/deepseek-v4-flash" => ReasoningEffort::High,
            "gemini/gemini-3.5-flash" | "gemini/gemini-3.1-pro-preview" => ReasoningEffort::Medium,
            "openai/gpt-5.5" | "openai/gpt-5.4" | "openai/gpt-5-mini" => ReasoningEffort::Medium,
            _ => ReasoningEffort::None,
        }
    }

    pub fn supports_reasoning_effort(model_id: &str, effort: ReasoningEffort) -> bool {
        match effort {
            ReasoningEffort::None
            | ReasoningEffort::Low
            | ReasoningEffort::Medium
            | ReasoningEffort::High => true,
            ReasoningEffort::XHigh => matches!(
                model_id,
                "openrouter/deepseek/deepseek-v4-pro"
                    | "openrouter/deepseek/deepseek-v4-flash"
                    | "deepseek/deepseek-v4-pro"
                    | "deepseek/deepseek-v4-flash"
                    | "gemini/gemini-3.1-pro-preview"
                    | "anthropic/claude-opus-4-7"
            ),
        }
    }

    fn model_belongs_to_first_party_provider(model: &str) -> bool {
        model.starts_with("anthropic/")
            || model.starts_with("gemini/")
            || model.starts_with("google/gemini")
            || model.starts_with("openai/")
            || model.starts_with("deepseek/")
    }

    /// create a provider by name, for the switch_model tool.
    /// if a model is specified, it must be in the provider's allowed list.
    pub fn from_name(
        client: reqwest::Client,
        provider: &str,
        model: Option<&str>,
    ) -> Result<Self, Error> {
        match provider {
            "anthropic" => {
                let mut p = AnthropicProvider::from_env(client)?;
                if let Some(m) = model {
                    if !anthropic::ALLOWED_MODELS.contains(&m) {
                        return Err(Error::Provider(format!(
                            "model {m} not allowed for anthropic. allowed: {}",
                            anthropic::ALLOWED_MODELS.join(", ")
                        )));
                    }
                    p.set_model(m.to_string());
                }
                let mut p = Self::Anthropic(p);
                let effort = Self::default_reasoning_effort(&p.model_id());
                p.set_reasoning_effort(effort);
                Ok(p)
            }
            "gemini" => {
                let mut p = GeminiProvider::from_env(client)?;
                if let Some(m) = model {
                    if !gemini::ALLOWED_MODELS.contains(&m) {
                        return Err(Error::Provider(format!(
                            "model {m} not allowed for gemini. allowed: {}",
                            gemini::ALLOWED_MODELS.join(", ")
                        )));
                    }
                    p.set_model(m.to_string());
                }
                let mut p = Self::Gemini(p);
                let effort = Self::default_reasoning_effort(&p.model_id());
                p.set_reasoning_effort(effort);
                Ok(p)
            }
            "deepseek" => {
                let mut p = DeepSeekProvider::from_env(client)?;
                if let Some(m) = model {
                    if !deepseek::ALLOWED_MODELS.contains(&m) {
                        return Err(Error::Provider(format!(
                            "model {m} not allowed for deepseek. allowed: {}",
                            deepseek::ALLOWED_MODELS.join(", ")
                        )));
                    }
                    p.set_model(m.to_string());
                }
                let mut p = Self::DeepSeek(p);
                let effort = Self::default_reasoning_effort(&p.model_id());
                p.set_reasoning_effort(effort);
                Ok(p)
            }
            "openai" => {
                let mut p = OpenAiProvider::from_env(client)?;
                if let Some(m) = model {
                    if !openai::ALLOWED_MODELS.contains(&m) {
                        return Err(Error::Provider(format!(
                            "model {m} not allowed for openai. allowed: {}",
                            openai::ALLOWED_MODELS.join(", ")
                        )));
                    }
                    p.set_model(m.to_string());
                }
                let mut p = Self::OpenAi(p);
                let effort = Self::default_reasoning_effort(&p.model_id());
                p.set_reasoning_effort(effort);
                Ok(p)
            }
            "openrouter" => {
                if let Some(m) = model
                    && Self::model_belongs_to_first_party_provider(m)
                {
                    return Err(Error::Provider(format!(
                        "model {m} should use its first-party provider, not openrouter"
                    )));
                }
                let mut p = OpenRouterProvider::from_env(client)?;
                if let Some(m) = model {
                    // no validation — openrouter has hundreds of models, let the API reject bad ones
                    p.set_model(m.to_string());
                }
                let mut p = Self::OpenRouter(p);
                let effort = Self::default_reasoning_effort(&p.model_id());
                p.set_reasoning_effort(effort);
                Ok(p)
            }
            _ => Err(Error::Provider(format!("unknown provider: {provider}"))),
        }
    }

    pub fn from_name_with_reasoning(
        client: reqwest::Client,
        provider: &str,
        model: Option<&str>,
        reasoning_effort: ReasoningEffort,
    ) -> Result<Self, Error> {
        let mut provider = Self::from_name(client, provider, model)?;
        provider.set_reasoning_effort(reasoning_effort);
        Ok(provider)
    }

    pub fn provider_name(&self) -> &str {
        match self {
            Self::Anthropic(_) => "anthropic",
            Self::DeepSeek(_) => "deepseek",
            Self::Gemini(_) => "gemini",
            Self::OpenAi(_) => "openai",
            Self::OpenRouter(_) => "openrouter",
            #[cfg(test)]
            Self::Test(_) => "test",
        }
    }

    pub fn context_window(&self) -> u32 {
        match self {
            Self::Anthropic(p) => p.context_window(),
            Self::DeepSeek(p) => p.context_window(),
            Self::Gemini(p) => p.context_window(),
            Self::OpenAi(p) => p.context_window(),
            Self::OpenRouter(p) => p.context_window(),
            #[cfg(test)]
            Self::Test(_) => 200_000,
        }
    }

    /// expected prompt-cache lifetime — see `Provider::cache_ttl`.
    pub fn cache_ttl(&self) -> Duration {
        match self {
            Self::Anthropic(p) => p.cache_ttl(),
            Self::DeepSeek(p) => p.cache_ttl(),
            Self::Gemini(p) => p.cache_ttl(),
            Self::OpenAi(p) => p.cache_ttl(),
            Self::OpenRouter(p) => p.cache_ttl(),
            #[cfg(test)]
            Self::Test(p) => p.cache_ttl(),
        }
    }
}

impl Provider for AnyProvider {
    async fn complete(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ProviderResponse, Error> {
        match self {
            Self::Anthropic(p) => p.complete(system_prompt, messages, tools).await,
            Self::DeepSeek(p) => p.complete(system_prompt, messages, tools).await,
            Self::Gemini(p) => p.complete(system_prompt, messages, tools).await,
            Self::OpenAi(p) => p.complete(system_prompt, messages, tools).await,
            Self::OpenRouter(p) => p.complete(system_prompt, messages, tools).await,
            #[cfg(test)]
            Self::Test(p) => p.complete(system_prompt, messages, tools).await,
        }
    }

    fn cache_ttl(&self) -> Duration {
        AnyProvider::cache_ttl(self)
    }
}

// -- test provider --

#[cfg(test)]
pub struct TestProvider {
    #[allow(clippy::type_complexity)]
    pub handler: Box<dyn Fn(&str, &[Message]) -> Result<ProviderResponse, Error> + Send + Sync>,
}

#[cfg(test)]
impl Provider for TestProvider {
    async fn complete(
        &self,
        system_prompt: &str,
        messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<ProviderResponse, Error> {
        (self.handler)(system_prompt, messages)
    }

    fn cache_ttl(&self) -> Duration {
        Duration::from_secs(300)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test HTTP client")
    }

    #[test]
    fn test_model_id_format_anthropic() {
        let p = AnthropicProvider::new(test_client(), "test-key".into());
        let any = AnyProvider::Anthropic(p);
        assert_eq!(any.model_id(), "anthropic/claude-sonnet-4-6");
    }

    #[test]
    fn test_model_id_format_gemini() {
        let p = GeminiProvider::new(test_client(), "test-key".into());
        let any = AnyProvider::Gemini(p);
        assert_eq!(any.model_id(), "gemini/gemini-3.5-flash");
    }

    #[test]
    fn test_model_id_format_deepseek() {
        let p = DeepSeekProvider::new(test_client(), "test-key".into());
        let any = AnyProvider::DeepSeek(p);
        assert_eq!(any.model_id(), "deepseek/deepseek-v4-flash");
    }

    #[test]
    fn test_model_id_format_openai() {
        let p = OpenAiProvider::new(test_client(), "test-key".into());
        let any = AnyProvider::OpenAi(p);
        assert_eq!(any.model_id(), "openai/gpt-5.5");
    }

    #[test]
    fn test_model_id_format_openrouter_default_is_not_deepseek() {
        let p = OpenRouterProvider::new(test_client(), "test-key".into());
        let any = AnyProvider::OpenRouter(p);
        assert_eq!(any.model_id(), "openrouter/meta-llama/llama-4-maverick");
    }

    #[test]
    fn test_model_id_format_test() {
        let p = AnyProvider::Test(TestProvider {
            handler: Box::new(|_, _| unreachable!()),
        });
        assert_eq!(p.model_id(), "test/test");
    }

    #[test]
    fn test_reasoning_effort_normalization() {
        assert_eq!(
            ReasoningEffort::from_user_input("none"),
            Some(ReasoningEffort::None)
        );
        assert_eq!(
            ReasoningEffort::from_user_input("off"),
            Some(ReasoningEffort::None)
        );
        assert_eq!(
            ReasoningEffort::from_user_input("low"),
            Some(ReasoningEffort::Low)
        );
        assert_eq!(
            ReasoningEffort::from_user_input("medium"),
            Some(ReasoningEffort::Medium)
        );
        assert_eq!(
            ReasoningEffort::from_user_input("high"),
            Some(ReasoningEffort::High)
        );
        assert_eq!(
            ReasoningEffort::from_user_input("xhigh"),
            Some(ReasoningEffort::XHigh)
        );
        assert_eq!(ReasoningEffort::from_user_input("minimal"), None);
        assert_eq!(ReasoningEffort::from_user_input("max"), None);
        assert_eq!(ReasoningEffort::from_user_input("unknown"), None);
    }

    #[test]
    fn test_default_reasoning_effort_by_model() {
        assert_eq!(
            AnyProvider::default_reasoning_effort("openrouter/deepseek/deepseek-v4-pro"),
            ReasoningEffort::High
        );
        assert_eq!(
            AnyProvider::default_reasoning_effort("openrouter/deepseek/deepseek-v4-flash"),
            ReasoningEffort::High
        );
        assert_eq!(
            AnyProvider::default_reasoning_effort("deepseek/deepseek-v4-pro"),
            ReasoningEffort::High
        );
        assert_eq!(
            AnyProvider::default_reasoning_effort("openrouter/deepseek/deepseek-chat"),
            ReasoningEffort::None
        );
        assert_eq!(
            AnyProvider::default_reasoning_effort("openai/gpt-5.4"),
            ReasoningEffort::Medium
        );
        assert_eq!(
            AnyProvider::default_reasoning_effort("gemini/gemini-3.5-flash"),
            ReasoningEffort::Medium
        );
        assert_eq!(
            AnyProvider::default_reasoning_effort("anthropic/claude-sonnet-4-6"),
            ReasoningEffort::None
        );
    }

    #[test]
    fn test_xhigh_support_is_model_specific() {
        assert!(AnyProvider::supports_reasoning_effort(
            "openrouter/deepseek/deepseek-v4-pro",
            ReasoningEffort::XHigh
        ));
        assert!(AnyProvider::supports_reasoning_effort(
            "deepseek/deepseek-v4-pro",
            ReasoningEffort::XHigh
        ));
        assert!(!AnyProvider::supports_reasoning_effort(
            "openai/gpt-5.4",
            ReasoningEffort::XHigh
        ));
        assert!(AnyProvider::supports_reasoning_effort(
            "gemini/gemini-3.1-pro-preview",
            ReasoningEffort::XHigh
        ));
        assert!(!AnyProvider::supports_reasoning_effort(
            "gemini/gemini-3.5-flash",
            ReasoningEffort::XHigh
        ));
        assert!(!AnyProvider::supports_reasoning_effort(
            "anthropic/claude-sonnet-4-6",
            ReasoningEffort::XHigh
        ));
    }

    #[test]
    fn test_openrouter_rejects_first_party_provider_models() {
        let err = AnyProvider::from_name(
            test_client(),
            "openrouter",
            Some("anthropic/claude-sonnet-4-6"),
        )
        .err()
        .expect("openrouter should reject anthropic models");
        assert!(err.to_string().contains("first-party provider"));

        let err = AnyProvider::from_name(test_client(), "openrouter", Some("openai/gpt-5.5"))
            .err()
            .expect("openrouter should reject openai models");
        assert!(err.to_string().contains("first-party provider"));

        let err =
            AnyProvider::from_name(test_client(), "openrouter", Some("gemini/gemini-3.5-flash"))
                .err()
                .expect("openrouter should reject gemini models");
        assert!(err.to_string().contains("first-party provider"));

        let err =
            AnyProvider::from_name(test_client(), "openrouter", Some("google/gemini-3.5-flash"))
                .err()
                .expect("openrouter should reject google gemini models");
        assert!(err.to_string().contains("first-party provider"));

        let err = AnyProvider::from_name(
            test_client(),
            "openrouter",
            Some("deepseek/deepseek-v4-pro"),
        )
        .err()
        .expect("openrouter should reject deepseek models");
        assert!(err.to_string().contains("first-party provider"));
    }
}
