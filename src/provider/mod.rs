mod anthropic;
mod openai;

pub use crate::tool::ToolCall;
pub use anthropic::AnthropicProvider;
pub use openai::OpenAiProvider;

use std::future::Future;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::message::Message;

pub const DEFAULT_SYSTEM_PROMPT: &str = "you are ava, a personal ai assistant. be helpful, concise, and friendly. avoid unnecessary verbosity.";

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
        include_tools: bool,
    ) -> impl Future<Output = Result<ProviderResponse, Error>> + Send;
}

// -- AnyProvider enum dispatch --

pub enum AnyProvider {
    Anthropic(AnthropicProvider),
    OpenAi(OpenAiProvider),
    #[cfg(test)]
    Test(TestProvider),
}

impl AnyProvider {
    /// create the default provider from environment variables (anthropic)
    pub fn default_from_env(client: reqwest::Client) -> Result<Self, Error> {
        Ok(Self::Anthropic(AnthropicProvider::from_env(client)?))
    }

    /// returns a `"provider/model"` identifier string (e.g. `"anthropic/claude-sonnet-4-5"`)
    pub fn model_id(&self) -> String {
        match self {
            Self::Anthropic(p) => format!("anthropic/{}", p.model_name()),
            Self::OpenAi(p) => format!("openai/{}", p.model_name()),
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
            _ => Err(Error::Provider(format!("unknown provider: {provider}"))),
        }
    }

    pub fn context_window(&self) -> u32 {
        match self {
            Self::Anthropic(p) => p.context_window(),
            Self::OpenAi(p) => p.context_window(),
            #[cfg(test)]
            Self::Test(_) => 200_000,
        }
    }
}

impl Provider for AnyProvider {
    async fn complete(
        &self,
        system_prompt: &str,
        messages: &[Message],
        include_tools: bool,
    ) -> Result<ProviderResponse, Error> {
        match self {
            Self::Anthropic(p) => p.complete(system_prompt, messages, include_tools).await,
            Self::OpenAi(p) => p.complete(system_prompt, messages, include_tools).await,
            #[cfg(test)]
            Self::Test(p) => p.complete(system_prompt, messages, include_tools).await,
        }
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
        _include_tools: bool,
    ) -> Result<ProviderResponse, Error> {
        (self.handler)(system_prompt, messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_id_format_anthropic() {
        let p = AnthropicProvider::new(reqwest::Client::new(), "test-key".into());
        let any = AnyProvider::Anthropic(p);
        assert_eq!(any.model_id(), "anthropic/claude-sonnet-4-5");
    }

    #[test]
    fn test_model_id_format_openai() {
        let p = OpenAiProvider::new(reqwest::Client::new(), "test-key".into());
        let any = AnyProvider::OpenAi(p);
        assert_eq!(any.model_id(), "openai/gpt-5.2");
    }

    #[test]
    fn test_model_id_format_test() {
        let p = AnyProvider::Test(TestProvider {
            handler: Box::new(|_, _| unreachable!()),
        });
        assert_eq!(p.model_id(), "test/test");
    }
}
