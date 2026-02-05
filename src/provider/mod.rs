mod anthropic;

pub use crate::tool::ToolCall;
pub use anthropic::AnthropicProvider;

use std::future::Future;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::message::Message;

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
        messages: &[Message],
    ) -> impl Future<Output = Result<ProviderResponse, Error>> + Send;
}
