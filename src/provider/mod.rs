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

#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub content: String,
    #[allow(dead_code)]
    pub stop_reason: StopReason,
    pub tool_calls: Vec<ToolCall>,
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
                Ok(Self::Anthropic(p))
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
                Ok(Self::OpenAi(p))
            }
            "openrouter" => {
                let mut p = OpenRouterProvider::from_env(client)?;
                if let Some(m) = model {
                    // no validation — openrouter has hundreds of models, let the API reject bad ones
                    p.set_model(m.to_string());
                }
                Ok(Self::OpenRouter(p))
            }
            _ => Err(Error::Provider(format!("unknown provider: {provider}"))),
        }
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

    #[test]
    fn test_model_id_format_anthropic() {
        let p = AnthropicProvider::new(reqwest::Client::new(), "test-key".into());
        let any = AnyProvider::Anthropic(p);
        assert_eq!(any.model_id(), "anthropic/claude-sonnet-4-6");
    }

    #[test]
    fn test_model_id_format_openai() {
        let p = OpenAiProvider::new(reqwest::Client::new(), "test-key".into());
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
}
