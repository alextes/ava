mod anthropic;
mod openai;
mod openrouter;

pub use crate::tool::ToolCall;
pub use anthropic::AnthropicProvider;
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
    /// openai: reasoning tokens used by reasoning models (subset of output_tokens)
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
    /// - openai: `Duration::from_secs(24 * 3600)` (24h retention hint)
    /// - openrouter: `Duration::ZERO` (no cache control is sent)
    fn cache_ttl(&self) -> Duration;
}

// -- AnyProvider enum dispatch --

pub enum AnyProvider {
    Anthropic(AnthropicProvider),
    OpenAi(OpenAiProvider),
    OpenRouter(OpenRouterProvider),
    #[cfg(test)]
    Test(TestProvider),
}

impl AnyProvider {
    /// create the default provider from environment variables (anthropic)
    pub fn default_from_env(client: reqwest::Client) -> Result<Self, Error> {
        Ok(Self::Anthropic(AnthropicProvider::from_env(client)?))
    }

    /// returns a `"provider/model"` identifier string (e.g. `"anthropic/claude-sonnet-4-6"`)
    pub fn model_id(&self) -> String {
        match self {
            Self::Anthropic(p) => format!("anthropic/{}", p.model_name()),
            Self::OpenAi(p) => format!("openai/{}", p.model_name()),
            Self::OpenRouter(p) => format!("openrouter/{}", p.model_name()),
            #[cfg(test)]
            Self::Test(_) => "test/test".to_string(),
        }
    }

    pub fn reasoning_effort(&self) -> ReasoningEffort {
        match self {
            Self::Anthropic(p) => p.reasoning_effort(),
            Self::OpenAi(p) => p.reasoning_effort(),
            Self::OpenRouter(p) => p.reasoning_effort(),
            #[cfg(test)]
            Self::Test(_) => ReasoningEffort::None,
        }
    }

    pub fn set_reasoning_effort(&mut self, effort: ReasoningEffort) {
        match self {
            Self::Anthropic(p) => p.set_reasoning_effort(effort),
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
            "openrouter/deepseek/deepseek-chat-v3-0324" => ReasoningEffort::None,
            "openai/gpt-5.5" | "openai/gpt-5.4" | "openai/gpt-5-mini" => ReasoningEffort::Medium,
            _ => ReasoningEffort::None,
        }
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
            Self::OpenAi(_) => "openai",
            Self::OpenRouter(_) => "openrouter",
            #[cfg(test)]
            Self::Test(_) => "test",
        }
    }

    pub fn context_window(&self) -> u32 {
        match self {
            Self::Anthropic(p) => p.context_window(),
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
    fn test_model_id_format_openai() {
        let p = OpenAiProvider::new(test_client(), "test-key".into());
        let any = AnyProvider::OpenAi(p);
        assert_eq!(any.model_id(), "openai/gpt-5.5");
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
            AnyProvider::default_reasoning_effort("openrouter/deepseek/deepseek-chat-v3-0324"),
            ReasoningEffort::None
        );
        assert_eq!(
            AnyProvider::default_reasoning_effort("openai/gpt-5.4"),
            ReasoningEffort::Medium
        );
        assert_eq!(
            AnyProvider::default_reasoning_effort("anthropic/claude-sonnet-4-6"),
            ReasoningEffort::None
        );
    }
}
