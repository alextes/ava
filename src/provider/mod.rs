mod anthropic;

pub use crate::tool::ToolCall;
pub use anthropic::AnthropicProvider;

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

#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub content: String,
    pub stop_reason: StopReason,
    pub tool_calls: Vec<ToolCall>,
}

pub trait Provider: Send + Sync {
    fn complete(
        &self,
        system_prompt: &str,
        messages: &[Message],
    ) -> impl Future<Output = Result<ProviderResponse, Error>> + Send;
}
